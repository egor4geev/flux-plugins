//! Photos and files from the phone, into the session (part 3).
//!
//! A message with a photo (its largest size) or a file: `getFile` → its bytes
//! ([`Transport::download`](crate::transport::Transport::download)) → a file in the plugin's data
//! folder (`inbox/<when>-<n>/<its name>`) → where the topic's words go now: the session
//! ([`conversation::send`]), or the new chat that waits for its project ([`newchat::take`]) — so the
//! late messages «Выполнить» brings ([`crate::late`]) go the same way. Pictures — a photo; a PNG,
//! JPEG, GIF or WebP file up to 10 MB (what Flux sends Claude as a picture) — go as pictures; any
//! other file by its path, a line under the message's words (Claude reads it itself). An album
//! (several messages with one `media_group_id`) goes as one message: a short timer ([`ALBUM_MS`])
//! gathers its parts. A file over 20 MB — the most Telegram gives a bot — is a calm note, and its
//! message isn't sent. The inbox's files older than a week go at start ([`clean`]).
//!
//! A topic keeps the user's order: while a photo or a file of the topic is on its way, the topic's
//! next messages — words, files, albums — wait behind it and go after it, in the order they came
//! (a photo, then «what is on it?»: the picture's message first). Each topic waits for its own
//! files only; a file that didn't come lets the rest go (its note, then the rest).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::bot::Bot;
use crate::conversation;
use crate::host::{Host, trf};
use crate::newchat;
use crate::render;
use crate::telegram::{self, MAX_DOWNLOAD, Message, TgError, TgFile};
use crate::transport::{Lane, Module, Purpose};

/// The inbox's folder in the plugin's data folder.
const INBOX: &str = "inbox";
/// An album's parts come one by one: the last one this long after the first.
pub const ALBUM_MS: u32 = 1_500;
/// Flux sends Claude a picture up to this size; a larger one goes as a file.
const MAX_PICTURE: u64 = 10 * 1024 * 1024;
/// The inbox keeps files this long.
const KEEP_MS: u64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Default)]
pub struct State {
    /// Messages (an album: its messages) with files on their way, and the words that wait behind
    /// them — by number, in the order they came; each topic's go in order.
    parcels: BTreeMap<u64, Parcel>,
    next: u64,
}

/// A message, or an album, with its files on their way — or words waiting behind one (no files).
struct Parcel {
    thread: i64,
    /// The album's id.
    group: Option<String>,
    /// The album still gathers its parts.
    timer: Option<u64>,
    words: Vec<String>,
    files: Vec<Item>,
}

/// A file of a parcel.
struct Item {
    /// The name it is saved by: "photo.jpg", the file's own.
    name: String,
    picture: bool,
    /// Where it was saved; none — on its way.
    path: Option<String>,
    /// It didn't come (a note said so).
    failed: bool,
}

/// A photo or a file of a message.
struct Attached {
    id: String,
    size: Option<u64>,
    name: String,
    picture: bool,
}

/// The user's message in a topic: with a photo or a file, it goes once its files are here; words
/// wait behind the topic's files on their way. False — nothing to wait for: the words go now.
pub fn receive(bot: &mut Bot, host: &mut dyn Host, message: &Message) -> bool {
    let Some(file) = attached(message) else {
        let Some(thread) = message.thread() else {
            return false;
        };
        let words = message.words().trim();
        if words.is_empty()
            || !bot
                .inbox
                .parcels
                .values()
                .any(|parcel| parcel.thread == thread)
        {
            return false;
        }
        let number = next(bot);
        bot.inbox.parcels.insert(
            number,
            Parcel {
                thread,
                group: None,
                timer: None,
                words: vec![words.to_string()],
                files: Vec::new(),
            },
        );
        return true;
    };
    let Some(thread) = message.thread() else {
        return true;
    };
    if file.size.is_some_and(|size| size > MAX_DOWNLOAD) {
        let note = trf(
            host,
            "«{0}» is larger than 20 MB: Telegram doesn't give bots such files. Send a smaller one, or put it in the project.",
            &[&file.name],
        );
        conversation::send_note(bot, host, thread, &render::escape(&note));
        return true;
    }
    let group = message.media_group_id.clone();
    let gathering = group.as_ref().and_then(|group| {
        bot.inbox
            .parcels
            .iter()
            .find(|(_, parcel)| {
                parcel.thread == thread
                    && parcel.group.as_ref() == Some(group)
                    && parcel.timer.is_some()
            })
            .map(|(number, _)| *number)
    });
    let number = match gathering {
        Some(number) => number,
        None => {
            let number = next(bot);
            let timer = group.is_some().then(|| host.timer(ALBUM_MS));
            bot.inbox.parcels.insert(
                number,
                Parcel {
                    thread,
                    group,
                    timer,
                    words: Vec::new(),
                    files: Vec::new(),
                },
            );
            number
        }
    };
    let Some(parcel) = bot.inbox.parcels.get_mut(&number) else {
        return true;
    };
    let words = message.words().trim();
    if !words.is_empty() {
        parcel.words.push(words.to_string());
    }
    parcel.files.push(Item {
        name: file.name,
        picture: file.picture,
        path: None,
        failed: false,
    });
    let index = parcel.files.len() - 1;
    let purpose = Purpose::new(Module::Inbox, format!("get:{number}:{index}"));
    bot.call(
        host,
        telegram::get_file(&file.id),
        Lane::Topic(thread),
        purpose,
    );
    true
}

/// `getFile` answered: the download starts.
pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    let Some((number, index)) = parse(tag, "get:") else {
        return;
    };
    match result.and_then(telegram::result::<TgFile>) {
        Ok(TgFile {
            file_path: Some(path),
            ..
        }) => {
            let purpose = Purpose::new(Module::Inbox, format!("file:{number}:{index}"));
            if !bot.tg.download(host, &path, purpose) {
                failed(bot, host, number, index, "no token");
            }
        }
        Ok(_) => failed(bot, host, number, index, "no file path"),
        Err(err) => failed(bot, host, number, index, &format!("{err:?}")),
    }
}

/// A file's bytes came (or didn't): saved in the inbox.
pub fn on_file(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Vec<u8>, String>) {
    let Some((number, index)) = parse(tag, "file:") else {
        return;
    };
    let Some(name) = bot
        .inbox
        .parcels
        .get(&number)
        .and_then(|parcel| parcel.files.get(index))
        .map(|item| item.name.clone())
    else {
        return;
    };
    let saved = result.and_then(|bytes| save(host, number, index, &name, &bytes));
    match saved {
        Ok(path) => {
            if let Some(item) = bot
                .inbox
                .parcels
                .get_mut(&number)
                .and_then(|parcel| parcel.files.get_mut(index))
            {
                item.path = Some(path);
            }
            deliver_ready(bot, host, number);
        }
        Err(err) => failed(bot, host, number, index, &err),
    }
}

/// An album's gathering ended (true — the timer was ours).
pub fn on_timer(bot: &mut Bot, host: &mut dyn Host, timer: u64) -> bool {
    let Some(number) = bot
        .inbox
        .parcels
        .iter()
        .find(|(_, parcel)| parcel.timer == Some(timer))
        .map(|(number, _)| *number)
    else {
        return false;
    };
    if let Some(parcel) = bot.inbox.parcels.get_mut(&number) {
        parcel.timer = None;
    }
    deliver_ready(bot, host, number);
    true
}

/// The inbox's files older than a week go (at start).
pub fn clean(host: &mut dyn Host) {
    let dir = PathBuf::from(host.data_dir()).join(INBOX);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let oldest = host.now_ms().saturating_sub(KEEP_MS);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let when = name
            .split('-')
            .next()
            .and_then(|when| when.parse::<u64>().ok());
        if when.is_some_and(|when| when < oldest) {
            let path = entry.path();
            let gone = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(err) = gone {
                host.log(&format!("Couldn't remove {}: {err}", path.display()));
            }
        }
    }
}

/// A file that didn't come: a note in the topic; the rest of its message goes on.
fn failed(bot: &mut Bot, host: &mut dyn Host, number: u64, index: usize, error: &str) {
    let Some(parcel) = bot.inbox.parcels.get_mut(&number) else {
        return;
    };
    let thread = parcel.thread;
    let Some(item) = parcel.files.get_mut(index) else {
        return;
    };
    item.failed = true;
    let name = item.name.clone();
    host.log(&format!("Couldn't download «{name}»: {error}"));
    let note = trf(host, "Couldn't download «{0}» from Telegram.", &[&name]);
    conversation::send_note(bot, host, thread, &render::escape(&note));
    deliver_ready(bot, host, number);
}

/// A parcel's number, in the order the messages came.
fn next(bot: &mut Bot) -> u64 {
    bot.inbox.next += 1;
    bot.inbox.next
}

/// Something of parcel `number` came: its topic's parcels go, in order, as long as the first one
/// is ready — every file here (or failed), its album whole.
fn deliver_ready(bot: &mut Bot, host: &mut dyn Host, number: u64) {
    let Some(thread) = bot.inbox.parcels.get(&number).map(|parcel| parcel.thread) else {
        return;
    };
    loop {
        let Some((&first, parcel)) = bot
            .inbox
            .parcels
            .iter()
            .find(|(_, parcel)| parcel.thread == thread)
        else {
            return;
        };
        let ready = parcel.timer.is_none()
            && parcel
                .files
                .iter()
                .all(|item| item.path.is_some() || item.failed);
        if !ready {
            return;
        }
        if let Some(parcel) = bot.inbox.parcels.remove(&first) {
            deliver(bot, host, parcel);
        }
    }
}

/// A ready parcel goes where the topic's words go now — the session, or the new chat waiting for
/// its project.
fn deliver(bot: &mut Bot, host: &mut dyn Host, parcel: Parcel) {
    let saved: Vec<&Item> = parcel
        .files
        .iter()
        .filter(|item| item.path.is_some())
        .collect();
    if !parcel.files.is_empty() && saved.is_empty() {
        // Every file failed: the notes said so, and the words alone would mislead.
        return;
    }
    let images: Vec<String> = saved
        .iter()
        .filter(|item| item.picture)
        .filter_map(|item| item.path.clone())
        .collect();
    let lines: Vec<String> = saved
        .iter()
        .filter(|item| !item.picture)
        .filter_map(|item| item.path.as_deref())
        .map(|path| trf(host, "File: {0}", &[&path]))
        .collect();
    let mut text = parcel.words.join("\n\n");
    if !lines.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&lines.join("\n"));
    }
    let thread = parcel.thread;
    match bot
        .store
        .topic_by_thread(thread)
        .map(|topic| topic.session.clone())
    {
        Some(session) => conversation::send(bot, host, thread, &session, &text, &images),
        None => newchat::take(bot, host, thread, &text, images),
    }
}

/// Saves a file's bytes in the inbox: its absolute path.
fn save(
    host: &dyn Host,
    number: u64,
    index: usize,
    name: &str,
    bytes: &[u8],
) -> Result<String, String> {
    let dir = PathBuf::from(host.data_dir())
        .join(INBOX)
        .join(format!("{}-{number}-{index}", host.now_ms()));
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// The photo (its largest size) or the file a message carries.
fn attached(message: &Message) -> Option<Attached> {
    if let Some(photo) = message.photo.as_ref().and_then(|sizes| {
        sizes.iter().max_by_key(|size| {
            (
                u64::from(size.width) * u64::from(size.height),
                size.file_size,
            )
        })
    }) {
        return Some(Attached {
            id: photo.file_id.clone(),
            size: photo.file_size,
            name: "photo.jpg".into(),
            picture: true,
        });
    }
    let document = message.document.as_ref()?;
    let extension = picture_extension(document.mime_type.as_deref().unwrap_or_default());
    let picture = extension.is_some() && document.file_size.unwrap_or(0) <= MAX_PICTURE;
    let mut name = file_name(document.file_name.as_deref().unwrap_or_default());
    // Flux tells a picture by its extension.
    if let Some(extension) = extension.filter(|_| picture) {
        let own = name
            .rsplit_once('.')
            .map(|(_, own)| own.to_ascii_lowercase())
            .unwrap_or_default();
        if own != extension && !(extension == "jpg" && own == "jpeg") {
            name = format!("{name}.{extension}");
        }
    }
    Some(Attached {
        id: document.file_id.clone(),
        size: document.file_size,
        name,
        picture,
    })
}

/// The extension of a picture Flux sends Claude, by its MIME type.
fn picture_extension(mime: &str) -> Option<&'static str> {
    Some(match mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return None,
    })
}

/// A file's name the file system takes: no folders, no control characters, not too long.
fn file_name(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim_start_matches('.');
    if clean.is_empty() {
        return "file".into();
    }
    if clean.chars().count() <= 100 {
        return clean.to_string();
    }
    // Too long: its start and its extension.
    let extension = clean
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| extension.len() <= 10)
        .unwrap_or_default();
    let start: String = clean.chars().take(80).collect();
    if extension.is_empty() {
        start
    } else {
        format!("{start}.{extension}")
    }
}

fn parse(tag: &str, prefix: &str) -> Option<(u64, usize)> {
    let (number, index) = tag.strip_prefix(prefix)?.split_once(':')?;
    Some((number.parse().ok()?, index.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeHost, Telegram, bot_with_topic, bound_bot, user_message};
    use flux_plugin_api::remote::Project;
    use serde_json::json;

    /// The owner's photo (two sizes) in a topic, with a caption.
    fn photo(
        update: i64,
        thread: i64,
        caption: &str,
        group: Option<&str>,
    ) -> crate::telegram::Update {
        let mut update = user_message(update, Some(thread), "");
        let message = update.message.as_mut().unwrap();
        message.text = None;
        message.caption = (!caption.is_empty()).then(|| caption.to_string());
        message.photo = Some(
            serde_json::from_value(json!([
                { "file_id": format!("photo-small-{}", update.update_id), "width": 90, "height": 60, "file_size": 900 },
                { "file_id": format!("photo-{}", update.update_id), "width": 1280, "height": 960, "file_size": 90_000 },
            ]))
            .unwrap(),
        );
        message.media_group_id = group.map(str::to_string);
        update
    }

    fn document(
        update: i64,
        thread: i64,
        name: &str,
        mime: &str,
        size: u64,
    ) -> crate::telegram::Update {
        let mut update = user_message(update, Some(thread), "");
        let message = update.message.as_mut().unwrap();
        message.text = None;
        message.caption = Some("Look".into());
        message.document = Some(
            serde_json::from_value(json!({
                "file_id": format!("doc-{}", update.update_id), "file_name": name,
                "mime_type": mime, "file_size": size,
            }))
            .unwrap(),
        );
        update
    }

    /// The paths sent to Claude with the last message: "send s1 <text> +<paths>".
    fn sent(host: &FakeHost) -> (String, Vec<String>) {
        let line = host
            .claude
            .iter()
            .rev()
            .find(|line| line.starts_with("send "))
            .expect("a message to Claude")
            .clone();
        match line.rsplit_once(" +") {
            Some((text, paths)) => (
                text.to_string(),
                paths.split(',').map(str::to_string).collect(),
            ),
            None => (line, Vec::new()),
        }
    }

    #[test]
    fn a_photo_is_downloaded_into_the_inbox_and_sent_as_a_picture() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_update(&mut host, photo(1, 77, "What is wrong here?", None));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].0, "getFile");
        assert_eq!(calls[0].1["file_id"], "photo-1", "the largest size");
        assert_eq!(
            calls[1],
            ("GET".into(), json!({ "path": "photos/photo-1.jpg" }))
        );
        let (text, paths) = sent(&host);
        assert_eq!(text, "send s1 What is wrong here?");
        assert_eq!(paths.len(), 1);
        assert!(paths[0].starts_with(&format!("{}/inbox/", host.data)));
        assert!(paths[0].ends_with("/photo.jpg"));
        assert_eq!(
            std::fs::read_to_string(&paths[0]).unwrap(),
            "bytes of photos/photo-1.jpg"
        );
        assert!(
            calls.iter().any(|(method, _)| method == "sendMessageDraft"),
            "«Thinking…» once it went"
        );
    }

    /// Claude's messages so far: the "send …" lines.
    fn sends(host: &FakeHost) -> Vec<String> {
        host.claude
            .iter()
            .filter(|line| line.starts_with("send "))
            .cloned()
            .collect()
    }

    #[test]
    fn words_after_a_photo_wait_for_it_and_go_in_order() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        bot.store.add_topic(crate::store::Topic {
            thread: 78,
            session: "s2".into(),
            project: "/work/project".into(),
            ..Default::default()
        });
        let mut tg = Telegram::default();
        bot.on_update(&mut host, photo(1, 77, "", None));
        bot.on_update(&mut host, user_message(2, Some(77), "What is on it?"));
        bot.on_update(&mut host, user_message(3, Some(78), "Hi"));
        // Another topic's words go at once; these wait for the topic's photo.
        assert_eq!(sends(&host), vec!["send s2 Hi"]);
        tg.flush(&mut bot, &mut host);
        let sent = sends(&host);
        assert_eq!(sent.len(), 3, "{sent:?}");
        assert!(
            sent[1].starts_with("send s1  +") && sent[1].ends_with("/photo.jpg"),
            "the picture first: {sent:?}"
        );
        assert_eq!(sent[2], "send s1 What is on it?");
        // Nothing waits now: the next words go at once.
        bot.on_update(&mut host, user_message(4, Some(77), "Thanks"));
        assert_eq!(host.claude.last().unwrap(), "send s1 Thanks");
    }

    #[test]
    fn a_file_that_doesnt_come_lets_the_words_behind_it_go() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_update(&mut host, photo(1, 77, "Look", None));
        bot.on_update(&mut host, user_message(2, Some(77), "And fix it"));
        assert!(sends(&host).is_empty());
        assert_eq!(
            tg.refuse(&mut bot, &mut host, "Bad Request: invalid file_id"),
            "getFile"
        );
        assert_eq!(sends(&host), vec!["send s1 And fix it"]);
        let calls = tg.flush(&mut bot, &mut host);
        assert!(
            calls
                .iter()
                .any(|(_, params)| params["text"] == "Couldn't download «photo.jpg» from Telegram."),
            "its note: {calls:?}"
        );
        // A file over 20 MB holds nothing back.
        bot.on_update(
            &mut host,
            document(3, 77, "dump.bin", "application/octet-stream", 30 << 20),
        );
        bot.on_update(&mut host, user_message(4, Some(77), "Next"));
        assert_eq!(host.claude.last().unwrap(), "send s1 Next");
    }

    #[test]
    fn an_album_goes_as_one_message() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_update(&mut host, photo(1, 77, "Both screens", Some("album-1")));
        bot.on_update(&mut host, photo(2, 77, "", Some("album-1")));
        // Both downloaded; the album waits for its timer.
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            calls.iter().filter(|(method, _)| method == "GET").count(),
            2
        );
        // `flush` lets the album's short timer pass: one message with both pictures.
        let sends: Vec<&String> = host
            .claude
            .iter()
            .filter(|line| line.starts_with("send "))
            .collect();
        assert_eq!(sends.len(), 1, "{sends:?}");
        let (text, paths) = sent(&host);
        assert_eq!(text, "send s1 Both screens");
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn a_file_goes_by_its_path_and_a_picture_file_as_a_picture() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_update(
            &mut host,
            document(1, 77, "server.log", "text/plain", 5_000),
        );
        tg.flush(&mut bot, &mut host);
        let (text, paths) = sent(&host);
        assert!(paths.is_empty());
        let path = text
            .strip_prefix("send s1 Look\n\nFile: ")
            .unwrap_or_else(|| panic!("{text}"));
        assert!(path.ends_with("/server.log"));
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "bytes of documents/doc-1"
        );
        // A PNG sent as a file without its extension: a picture, named so Flux knows it.
        bot.on_update(&mut host, document(2, 77, "screen", "image/png", 5_000));
        tg.flush(&mut bot, &mut host);
        let (text, paths) = sent(&host);
        assert_eq!(text, "send s1 Look");
        assert!(paths[0].ends_with("/screen.png"));
    }

    #[test]
    fn a_file_over_20_mb_is_a_note_and_nothing_goes() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_update(
            &mut host,
            document(
                1,
                77,
                "dump.bin",
                "application/octet-stream",
                30 * 1024 * 1024,
            ),
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].1["text"],
            "«dump.bin» is larger than 20 MB: Telegram doesn't give bots such files. Send a smaller one, or put it in the project."
        );
        assert!(!host.claude.iter().any(|line| line.starts_with("send ")));
    }

    #[test]
    fn in_a_new_chat_the_files_wait_with_the_words_for_the_project() {
        let mut host = FakeHost::default();
        host.projects = vec![Project {
            path: "/work/flux".into(),
            name: "flux".into(),
            open: true,
        }];
        let mut bot = bound_bot(&mut host);
        let mut tg = Telegram::default();
        bot.on_update(&mut host, photo(1, 80, "Fix this screen", None));
        let calls = tg.flush(&mut bot, &mut host);
        let ask = calls
            .iter()
            .find(|(method, _)| method == "sendMessage")
            .expect("the project's question");
        assert_eq!(ask.1["text"], "Which project?");
        let data = crate::testing::button(&ask.1, "flux");
        bot.on_update(&mut host, crate::testing::press(2, 101, 80, &data));
        // No task source: the session starts at once, with the picture.
        let line = host.claude.last().unwrap();
        assert!(line.starts_with(&format!(
            "new /work/flux Fix this screen +{}/inbox/",
            host.data
        )));
        assert!(line.ends_with("/photo.jpg"));
        assert_eq!(
            bot.store.topic_by_thread(80).unwrap().session,
            host.new_session_id
        );
    }

    #[test]
    fn old_inbox_files_go_at_start() {
        let mut host = FakeHost::default();
        let inbox = PathBuf::from(&host.data).join(INBOX);
        let old = inbox.join(format!("{}-1-0", host.now - KEEP_MS - 1));
        let new = inbox.join(format!("{}-2-0", host.now - 1000));
        for dir in [&old, &new] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("photo.jpg"), b"x").unwrap();
        }
        clean(&mut host);
        assert!(!old.exists());
        assert!(new.join("photo.jpg").exists());
    }
}
