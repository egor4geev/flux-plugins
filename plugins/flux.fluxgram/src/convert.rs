//! TDLib's messages as Flux shows them (part 3): a `message` → the conversation's
//! [`Message`] — the author (in groups), the text with its entities (bold, links, code blocks,
//! quotes, spoilers; TDLib's offsets are UTF-16), the reply's quote, "Forwarded from", the photo
//! (its `minithumbnail` blurred until the file is there), the sticker, the file, the link preview,
//! the chat's own lines ("Anna joined the group"), reactions, the delivery mark, and the actions
//! the message offers (Edit and Delete only where Telegram allows them).
//!
//! Media (part 4), played by Flux: a voice message (its waveform unpacked from Telegram's 5-bit
//! samples), a round video message, a GIF, a video — each with its poster and its file once
//! downloaded; an animated sticker (TGS, WEBM) and an animated emoji as the still thumbnail,
//! with the animation once the helper rendered it into Flux's file of frames (FXA). What plays by
//! itself and what downloads by itself are the plugin's settings ([`MediaOptions`]).
//!
//! Needs no host — the words come through [`Words`], the names and files from the [`Store`] — so
//! it is tested natively on TDLib's JSON. A message in one line (a quote's text) is
//! [`crate::summary::line`]. A chat's search marks its words in the converted messages
//! ([`highlighted`]: in the window that searches, not here).

use base64::Engine;
use flux_plugin_api::conversation::{
    self as conv, Delivery, ImageShape, ImageSpec, Message, MessageQuote, Part, RichSpan,
    VideoKind, rich,
};
use serde_json::Value;

use crate::model::{Words, downloaded};
use crate::store::{ChatKind, Store};
use crate::summary;

/// A photo in the feed: the largest size up to this side.
const FEED_SIDE: i64 = 1280;
/// A link preview's picture: the largest size up to this side.
const PREVIEW_SIDE: i64 = 640;
/// A round video message's side in the feed.
const ROUND_SIDE: u32 = 240;
/// A sticker's side in the feed, in pixels: twice its 160 points — the size its animation is
/// rendered at.
pub const STICKER_SIDE: u32 = 320;
/// GIFs and stickers download by themselves up to this size (larger ones wait for a click).
pub const AUTO_SIZE: u64 = 4 << 20;
/// The user's own text can be edited this long (Telegram's limit), when TDLib doesn't say.
const EDIT_WINDOW: i64 = 48 * 3600;

/// The message actions of a chat's conversation (`chat.rs` names them in its catalogue).
pub const REPLY: &str = "reply";
pub const EDIT: &str = "edit";
/// Delete, for everyone in the chat.
pub const DELETE: &str = "delete";
/// Delete, only for the user (Telegram can't delete it for the others).
pub const DELETE_FOR_ME: &str = "delete-for-me";

/// What a conversion needs besides the message.
pub struct Context<'a> {
    pub store: &'a Store,
    pub words: &'a dyn Words,
    /// Now, in seconds since 1970: the edit window of the user's own messages.
    pub now: i64,
    /// The message this one answers, when the feed has it (loaded, or fetched for the quote).
    pub replied: Option<&'a Value>,
    /// How media plays and downloads, and the animations rendered so far.
    pub media: &'a MediaOptions,
}

/// How the feed shows media (part 4): the plugin's settings, and where the stickers' animations
/// are.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaOptions {
    /// GIFs and round messages play by themselves (without sound) while on screen.
    pub autoplay: bool,
    /// Animated stickers play again and again; else once each time they come into sight.
    pub loop_stickers: bool,
    /// What downloads by itself once it is on screen: photos; voice and round messages; GIFs and
    /// stickers (up to [`AUTO_SIZE`]).
    pub photos: bool,
    pub voice: bool,
    pub gifs: bool,
    /// The folder of the rendered animations: `<cache>/frames`.
    pub frames: String,
    /// The animations there (their paths): the feed checks the folder, the helper renders more.
    pub rendered: std::collections::HashSet<String>,
}

impl Default for MediaOptions {
    fn default() -> Self {
        MediaOptions {
            autoplay: true,
            loop_stickers: true,
            photos: true,
            voice: true,
            gifs: true,
            frames: String::new(),
            rendered: std::collections::HashSet::new(),
        }
    }
}

impl MediaOptions {
    /// The animation file of a sticker's file (by its `remote.unique_id`, the same for every
    /// account) at `size` pixels.
    pub fn frames_of(&self, file: &Value, size: u32) -> Option<String> {
        let unique = file["remote"]["unique_id"]
            .as_str()
            .filter(|unique| !unique.is_empty())?;
        // Telegram's ids are base64url: safe in a file name.
        Some(format!("{}/{unique}-{size}.fxa", self.frames))
    }
}

/// An animated sticker's animation to render (part 4): its file, the file's path once
/// downloaded (the helper's input), the FXA it renders into.
#[derive(Debug, Clone, PartialEq)]
pub struct Render {
    pub file: i64,
    pub input: Option<String>,
    pub output: String,
    pub size: u32,
}

/// A message as Flux shows it, and what the feed follows of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Converted {
    pub message: Message,
    /// The file of each part, by the part's index: what a click on the part downloads.
    pub part_files: Vec<Option<i64>>,
    /// Every file the message shows (pictures, files, the author's photo): a change of one of
    /// them (`updateFile`) puts the message again.
    pub files: Vec<i64>,
    /// The files downloaded once the message is in sight: its pictures, the author's photo —
    /// not documents or videos, which wait for a click.
    pub auto: Vec<i64>,
    /// The photos' full sizes not downloaded yet: after the pictures in sight, at a lower
    /// priority, so that a click opens the viewer on the full one.
    pub full: Vec<i64>,
    /// The message answered in this chat, when the context hasn't it: the feed fetches it.
    pub reply_to: Option<i64>,
    /// Users and chats the message names that the store doesn't know yet.
    pub missing_users: Vec<i64>,
    pub missing_chats: Vec<i64>,
    /// The animated stickers' animations not rendered yet (part 4): the feed asks the helper
    /// once their files are downloaded.
    pub renders: Vec<Render>,
}

impl Converted {
    /// A message without parts yet: its id (the order too), its date.
    fn new(id: i64, date: i64) -> Self {
        Converted {
            message: Message::new(&id.to_string(), id.max(0) as u64, date),
            part_files: Vec::new(),
            files: Vec::new(),
            auto: Vec::new(),
            full: Vec::new(),
            reply_to: None,
            missing_users: Vec::new(),
            missing_chats: Vec::new(),
            renders: Vec::new(),
        }
    }
}

/// The message as Flux shows it, with what the feed follows of it.
pub fn convert(cx: &Context, value: &Value) -> Option<Converted> {
    if value["@type"] != "message" {
        return None;
    }
    let id = value["id"].as_i64()?;
    let chat = value["chat_id"].as_i64().unwrap_or_default();
    let mut out = Converted::new(id, value["date"].as_i64()?);
    let content = &value["content"];
    let kind = content["@type"].as_str().unwrap_or_default();
    if is_service(kind) {
        let line = summary::line(cx.store, value, cx.words);
        out.message = out.message.part(conv::notice(&line));
        out.part_files.push(None);
        return Some(out);
    }
    let outgoing = value["is_outgoing"] == true;
    if outgoing {
        out.message = out.message.outgoing(delivery(cx.store, value, chat, id));
    } else if is_group(cx.store, chat) {
        author(cx, value, &mut out);
    }
    if value["edit_date"].as_i64().unwrap_or_default() > 0 {
        out.message = out.message.edited();
    }
    forwarded(cx, value, &mut out);
    reply(cx, value, chat, &mut out);
    parts(cx, content, &mut out);
    reactions(value, &mut out);
    out.message = out.message.actions(actions(cx, value, chat, outgoing));
    Some(out)
}

// --- Albums ---

/// The album a message is in: TDLib's `media_album_id` (an int64 — a string in its JSON; a
/// pending album's is negative, the server's comes once it is sent); 0 — none.
pub fn album_id(value: &Value) -> i64 {
    let id = &value["media_album_id"];
    id.as_str()
        .and_then(|id| id.parse().ok())
        .or_else(|| id.as_i64())
        .unwrap_or_default()
}

/// The message has a caption (an album's item: the caption of the whole album).
pub fn has_caption(value: &Value) -> bool {
    value["content"]["caption"]["text"]
        .as_str()
        .is_some_and(|text| !text.is_empty())
}

/// An album — the photos and videos (or files) sent together, its items in id order — as one
/// message, as Telegram shows it: the first item's id, author, date, quote and "Forwarded from";
/// every item's picture one after another (Flux draws them as a grid; a click on one names its
/// part, which is that item's file), then the caption (or above them, as Telegram shows it) — the
/// item that has one is usually the first, sometimes the last; that item's reactions and Edit
/// (else the first's); Delete when every item may go; the newest item's delivery (a clock while
/// any is on its way, the red mark when one failed). One item is just the message.
pub fn album(cx: &Context, items: &[&Value]) -> Option<Converted> {
    let (&first, rest) = items.split_first()?;
    if rest.is_empty() {
        return convert(cx, first);
    }
    if first["@type"] != "message" {
        return None;
    }
    let id = first["id"].as_i64()?;
    let chat = first["chat_id"].as_i64().unwrap_or_default();
    let mut out = Converted::new(id, first["date"].as_i64()?);
    let captioned = items
        .iter()
        .copied()
        .find(|item| has_caption(item))
        .unwrap_or(first);
    let outgoing = first["is_outgoing"] == true;
    if outgoing {
        let marks: Vec<Delivery> = items
            .iter()
            .map(|item| delivery(cx.store, item, chat, item["id"].as_i64().unwrap_or(id)))
            .collect();
        let mark = [Delivery::Failed, Delivery::Sending]
            .into_iter()
            .find(|mark| marks.contains(mark))
            .or(marks.last().copied())
            .unwrap_or(Delivery::Sent);
        out.message = out.message.outgoing(mark);
    } else if is_group(cx.store, chat) {
        author(cx, first, &mut out);
    }
    if items
        .iter()
        .any(|item| item["edit_date"].as_i64().unwrap_or_default() > 0)
    {
        out.message = out.message.edited();
    }
    forwarded(cx, first, &mut out);
    reply(cx, first, chat, &mut out);
    let captions: Vec<Part> = items
        .iter()
        .flat_map(|item| formatted(&item["content"]["caption"]))
        .collect();
    let above = captioned["content"]["show_caption_above_media"] == true;
    if above {
        for part in captions.clone() {
            push(&mut out, part, None);
        }
    }
    for item in items {
        let mut content = item["content"].clone();
        if let Some(content) = content.as_object_mut() {
            content.remove("caption");
        }
        parts(cx, &content, &mut out);
    }
    if !above {
        for part in captions {
            push(&mut out, part, None);
        }
    }
    reactions(captioned, &mut out);
    let each: Vec<Vec<&str>> = items
        .iter()
        .map(|item| actions(cx, item, chat, outgoing))
        .collect();
    let mut offered = Vec::new();
    if each[0].contains(&REPLY) {
        offered.push(REPLY);
    }
    if actions(cx, captioned, chat, outgoing).contains(&EDIT) {
        offered.push(EDIT);
    }
    if each.iter().all(|actions| actions.contains(&DELETE)) {
        offered.push(DELETE);
    } else if each
        .iter()
        .all(|actions| actions.contains(&DELETE) || actions.contains(&DELETE_FOR_ME))
    {
        offered.push(DELETE_FOR_ME);
    }
    out.message = out.message.actions(offered);
    Some(out)
}

// --- What the message is ---

/// A line of the chat itself ("Anna joined the group"), shown in the middle.
fn is_service(kind: &str) -> bool {
    kind.starts_with("messageChat")
        || kind.starts_with("messageForumTopic")
        || kind.starts_with("messageVideoChat")
        || kind.starts_with("messageGifted")
        || matches!(
            kind,
            "messagePinMessage"
                | "messageBasicGroupChatCreate"
                | "messageSupergroupChatCreate"
                | "messageContactRegistered"
                | "messageScreenshotTaken"
                | "messageCustomServiceAction"
                | "messageGameScore"
                | "messageProximityAlertTriggered"
                | "messageExpiredPhoto"
                | "messageExpiredVideo"
                | "messageExpiredVideoNote"
                | "messageExpiredVoiceNote"
        )
}

fn is_group(store: &Store, chat: i64) -> bool {
    store.chats.get(&chat).is_some_and(|chat| {
        matches!(
            chat.kind,
            ChatKind::BasicGroup { .. } | ChatKind::Supergroup { .. }
        )
    })
}

/// The mark of the user's own message: a clock, a red mark, ✓ or ✓✓ (read up to the chat's
/// `last_read_outbox`).
fn delivery(store: &Store, value: &Value, chat: i64, id: i64) -> Delivery {
    match value["sending_state"]["@type"].as_str().unwrap_or_default() {
        "messageSendingStatePending" => Delivery::Sending,
        "messageSendingStateFailed" => Delivery::Failed,
        _ if store
            .chats
            .get(&chat)
            .is_some_and(|chat| id <= chat.last_read_outbox)
            || store.is_saved_messages(chat) =>
        {
            Delivery::Read
        }
        _ => Delivery::Sent,
    }
}

/// The author of someone else's message in a group: a user, or a chat (an anonymous admin, a
/// channel's post).
fn author(cx: &Context, value: &Value, out: &mut Converted) {
    let sender = &value["sender_id"];
    if sender["@type"] == "messageSenderChat" {
        let id = sender["chat_id"].as_i64().unwrap_or_default();
        let Some(chat) = cx.store.chats.get(&id) else {
            out.missing_chats.push(id);
            out.message = out.message.clone().author("…", 0, None);
            return;
        };
        let avatar = cx.store.chat_avatar(id);
        if let Some(file) = chat.photo {
            out.files.push(file);
            out.auto.push(file);
        }
        out.message = out
            .message
            .clone()
            .author(&chat.title, chat.color, Some(avatar));
        return;
    }
    let id = sender["user_id"].as_i64().unwrap_or_default();
    let Some(user) = cx.store.users.get(&id) else {
        out.missing_users.push(id);
        let avatar = cx.store.user_avatar(id);
        out.message = out.message.clone().author("…", avatar.color, Some(avatar));
        return;
    };
    let name = match user.name() {
        name if name.is_empty() => cx.words.tr("Deleted Account"),
        name => name,
    };
    if let Some(file) = user.photo {
        out.files.push(file);
        out.auto.push(file);
    }
    out.message = out
        .message
        .clone()
        .author(&name, user.color, Some(cx.store.user_avatar(id)));
}

/// "Forwarded from …": the original sender's name.
fn forwarded(cx: &Context, value: &Value, out: &mut Converted) {
    let origin = &value["forward_info"]["origin"];
    if !origin.is_object() {
        return;
    }
    let name = origin_name(cx, origin, out);
    out.message = out.message.clone().forwarded(&name);
}

/// The name of a `MessageOrigin`: a user, a hidden user, a chat or a channel (with the post's
/// signature).
fn origin_name(cx: &Context, origin: &Value, out: &mut Converted) -> String {
    let chat_title = |id: i64, out: &mut Converted| match cx.store.chats.get(&id) {
        Some(chat) => chat.title.clone(),
        None => {
            out.missing_chats.push(id);
            "…".to_string()
        }
    };
    let signed = |title: String| match origin["author_signature"].as_str() {
        Some(signature) if !signature.is_empty() => format!("{title} ({signature})"),
        _ => title,
    };
    match origin["@type"].as_str().unwrap_or_default() {
        "messageOriginUser" => {
            let id = origin["sender_user_id"].as_i64().unwrap_or_default();
            match cx.store.users.get(&id) {
                Some(user) if !user.name().is_empty() => user.name(),
                Some(_) => cx.words.tr("Deleted Account"),
                None => {
                    out.missing_users.push(id);
                    "…".to_string()
                }
            }
        }
        "messageOriginHiddenUser" => origin["sender_name"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        "messageOriginChat" => {
            let id = origin["sender_chat_id"].as_i64().unwrap_or_default();
            signed(chat_title(id, out))
        }
        "messageOriginChannel" => {
            let id = origin["chat_id"].as_i64().unwrap_or_default();
            signed(chat_title(id, out))
        }
        _ => "…".to_string(),
    }
}

/// The quote of the message answered: its author and a line of it — the message itself when the
/// feed has it, else the quoted piece TDLib gives, else "…" (and the feed fetches it).
fn reply(cx: &Context, value: &Value, chat: i64, out: &mut Converted) {
    let reply = &value["reply_to"];
    if reply["@type"] != "messageReplyToMessage" {
        return;
    }
    let target = reply["message_id"].as_i64().unwrap_or_default();
    let from = reply["chat_id"].as_i64().unwrap_or(chat);
    let quoted_text = reply["quote"]["text"]["text"]
        .as_str()
        .map(summary::first_line)
        .filter(|text| !text.is_empty());
    // A reply to a message of another chat: its origin's name, and what TDLib tells of it.
    if from != chat && from != 0 {
        let author = if reply["origin"].is_object() {
            origin_name(cx, &reply["origin"], out)
        } else {
            match cx.store.chats.get(&from) {
                Some(chat) => chat.title.clone(),
                None => {
                    out.missing_chats.push(from);
                    "…".to_string()
                }
            }
        };
        let text = quoted_text.unwrap_or_else(|| content_line(cx, &reply["content"]));
        out.message = out.message.clone().reply(MessageQuote {
            message: None,
            author,
            color: 0,
            text,
        });
        return;
    }
    let quote = match cx.replied.filter(|replied| replied["id"] == target) {
        Some(replied) => {
            let (author, color) = quote_author(cx, replied);
            let text = quoted_text.unwrap_or_else(|| summary::line(cx.store, replied, cx.words));
            MessageQuote {
                message: Some(target.to_string()),
                author,
                color,
                text,
            }
        }
        None => {
            out.reply_to = Some(target);
            MessageQuote {
                message: Some(target.to_string()),
                author: String::new(),
                color: 0,
                text: quoted_text.unwrap_or_else(|| "…".to_string()),
            }
        }
    };
    out.message = out.message.clone().reply(quote);
}

/// A message's content in a line, when there is one ("…" otherwise).
fn content_line(cx: &Context, content: &Value) -> String {
    if !content.is_object() {
        return "…".to_string();
    }
    let line = summary::line(
        cx.store,
        &serde_json::json!({ "content": content }),
        cx.words,
    );
    if line.is_empty() {
        "…".to_string()
    } else {
        line
    }
}

/// Who wrote a quoted message: "You" for the user's own, the name in its color.
fn quote_author(cx: &Context, message: &Value) -> (String, u8) {
    if message["is_outgoing"] == true {
        return (cx.words.tr("You"), 0);
    }
    let sender = summary::sender(cx.store, message, cx.words);
    let color = if sender.user {
        cx.store.users.get(&sender.id).map_or(0, |user| user.color)
    } else {
        cx.store.chats.get(&sender.id).map_or(0, |chat| chat.color)
    };
    (sender.name, color)
}

// --- The parts ---

fn push(out: &mut Converted, part: Part, file: Option<i64>) {
    out.message.parts.push(part);
    out.part_files.push(file);
}

/// The caption's parts after the media (or before it, when Telegram shows it above).
fn with_caption(out: &mut Converted, content: &Value, media: impl FnOnce(&mut Converted)) {
    let caption = formatted(&content["caption"]);
    let above = content["show_caption_above_media"] == true;
    if above {
        for part in caption.clone() {
            push(out, part, None);
        }
    }
    media(out);
    if !above {
        for part in caption {
            push(out, part, None);
        }
    }
}

fn parts(cx: &Context, content: &Value, out: &mut Converted) {
    let store = cx.store;
    match content["@type"].as_str().unwrap_or_default() {
        "messageText" => {
            let preview = &content["link_preview"];
            let above = preview["show_above_text"] == true;
            if above {
                link_preview(store, preview, out);
            }
            for part in formatted(&content["text"]) {
                push(out, part, None);
            }
            if !above {
                link_preview(store, preview, out);
            }
        }
        "messageAnimatedEmoji" => {
            let emoji = content["emoji"].as_str().unwrap_or_default();
            // Its sticker (TGS) plays as a sticker does; without one — the emoji as text.
            let animated = &content["animated_emoji"]["sticker"];
            if !animated.is_object() || !sticker(cx, animated, out) {
                push(out, conv::text(vec![rich(emoji)]), None);
            }
        }
        "messagePhoto" => with_caption(out, content, |out| {
            let photo = &content["photo"];
            match photo_spec(store, photo, FEED_SIDE, cx.media.photos, out) {
                Some((spec, file)) => {
                    if let Some(full) = spec.full.is_none().then(|| full_size(photo)).flatten()
                        && full != file
                        && cx.media.photos
                    {
                        out.full.push(full);
                    }
                    push(out, conv::image(spec), Some(file))
                }
                None => push(out, conv::placeholder(None, &cx.words.tr("Photo")), None),
            }
        }),
        "messageVideo" => with_caption(out, content, |out| {
            let video = &content["video"];
            let part = video_part(cx, video, &video["video"], VideoKind::Video, false, out);
            push(out, part, video["video"]["id"].as_i64());
        }),
        "messageAnimation" => with_caption(out, content, |out| {
            let animation = &content["animation"];
            let file = &animation["animation"];
            let auto = cx.media.gifs && small(cx.store, file);
            let part = video_part(cx, animation, file, VideoKind::Gif, auto, out);
            push(out, part, file["id"].as_i64());
        }),
        "messageVideoNote" => {
            let note = &content["video_note"];
            let file = &note["video"];
            let mut part = video_part(cx, note, file, VideoKind::Round, cx.media.voice, out);
            if let Part::Video(spec) = &mut part {
                spec.unheard = unheard(content, "is_viewed", out);
            }
            push(out, part, file["id"].as_i64());
        }
        "messageVoiceNote" => with_caption(out, content, |out| {
            let voice = &content["voice_note"];
            let file = &voice["voice"];
            let id = file["id"].as_i64();
            if let Some(id) = id {
                out.files.push(id);
                if cx.media.voice {
                    out.auto.push(id);
                }
            }
            let seconds = voice["duration"].as_u64().unwrap_or_default() as u32;
            let samples = voice["waveform"]
                .as_str()
                .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
                .map(|bytes| waveform(&bytes))
                .unwrap_or_default();
            let path = file_path(store, file);
            let mut spec = conv::voice_spec(path.as_deref(), seconds, samples);
            if path.is_none()
                && let Some(progress) = file_progress(store, file)
            {
                spec = spec.progress(progress);
            }
            if unheard(content, "is_listened", out) {
                spec = spec.unheard();
            }
            push(out, conv::voice(spec), id);
        }),
        "messageAudio" => with_caption(out, content, |out| {
            let audio = &content["audio"];
            let name = summary::line(
                store,
                &serde_json::json!({ "content": {"@type": "messageAudio", "audio": audio} }),
                cx.words,
            );
            file_part(store, &audio["audio"], &name, out);
        }),
        "messageDocument" => with_caption(out, content, |out| {
            let document = &content["document"];
            let name = document["file_name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map_or_else(|| cx.words.tr("File"), str::to_string);
            file_part(store, &document["document"], &name, out);
        }),
        "messageSticker" => {
            if !sticker(cx, &content["sticker"], out) {
                let emoji = content["sticker"]["emoji"].as_str().unwrap_or_default();
                let text = format!("{emoji} {}", cx.words.tr("Sticker"));
                push(out, conv::placeholder(None, text.trim()), None);
            }
        }
        _ => {
            // A poll, a location, a contact, a game, a story…: a line of what it is.
            let line = content_line(cx, content);
            let icon = match content["@type"].as_str().unwrap_or_default() {
                "messageLocation" | "messageVenue" => "icons/location.svg",
                "messagePoll" => "checklist",
                "messageContact" => "icons/phone.svg",
                "messageCall" => "icons/phone.svg",
                "messageStory" => "history",
                _ => "info",
            };
            push(out, conv::placeholder(Some(icon), &line), None);
        }
    }
    if out.message.parts.is_empty() {
        push(out, conv::text(vec![rich("")]), None);
    }
}

/// A sticker: a static WebP as it is; an animated one (TGS, WEBM) as its still thumbnail, with
/// its animation once the helper rendered it (part 4) — the sticker's file downloads by itself
/// with the GIFs' setting, then the feed asks for the render. False — no picture to show (the
/// caller shows its emoji).
fn sticker(cx: &Context, sticker: &Value, out: &mut Converted) -> bool {
    let width = sticker["width"].as_u64().unwrap_or(512) as u32;
    let height = sticker["height"].as_u64().unwrap_or(512) as u32;
    let format = sticker["format"]["@type"].as_str().unwrap_or_default();
    let animated = matches!(format, "stickerFormatTgs" | "stickerFormatWebm");
    let file = if format == "stickerFormatWebp" {
        Some(&sticker["sticker"])
    } else {
        let thumbnail = &sticker["thumbnail"];
        still(&thumbnail["format"]).then_some(&thumbnail["file"])
    };
    let Some(file) = file.filter(|file| file["id"].is_i64()) else {
        return false;
    };
    let id = file["id"].as_i64().unwrap_or_default();
    out.files.push(id);
    out.auto.push(id);
    let mut spec = match file_path(cx.store, file) {
        Some(path) => conv::picture(&path, width, height),
        None => conv::pending_picture(width, height),
    };
    if animated && let Some(path) = animation(cx, &sticker["sticker"], STICKER_SIDE, out) {
        spec = spec.animation(&path, cx.media.loop_stickers);
    }
    push(out, conv::sticker(spec.shape(ImageShape::Square)), Some(id));
    true
}

/// An animated sticker's FXA at `size` pixels, when it is rendered; else what the feed renders
/// once the file is there (the file downloads by itself with the GIFs' setting).
pub fn animation(cx: &Context, file: &Value, size: u32, out: &mut Converted) -> Option<String> {
    let id = file["id"].as_i64()?;
    let output = cx.media.frames_of(file, size)?;
    if cx.media.rendered.contains(&output) {
        return Some(output);
    }
    out.files.push(id);
    if cx.media.gifs && small(cx.store, file) {
        out.auto.push(id);
    }
    out.renders.push(Render {
        file: id,
        input: file_path(cx.store, file),
        output,
        size,
    });
    None
}

/// A video, a GIF or a round message (part 4): its poster (the thumbnail once downloaded, else
/// the blurred minithumbnail; the video file's progress), its file once downloaded, its length.
/// `auto` — its file downloads by itself once on screen; GIFs and rounds play by themselves with
/// the autoplay setting.
fn video_part(
    cx: &Context,
    media: &Value,
    file: &Value,
    kind: VideoKind,
    auto: bool,
    out: &mut Converted,
) -> Part {
    let size = match kind {
        VideoKind::Round => (ROUND_SIDE, ROUND_SIDE),
        _ => size_of(media),
    };
    let id = file["id"].as_i64();
    let poster = media_preview(cx.store, media, size, id, out);
    if let Some(id) = id.filter(|_| auto) {
        out.auto.push(id);
    }
    let seconds = media["duration"].as_u64().unwrap_or_default() as u32;
    let path = file_path(cx.store, file);
    let mut spec = conv::video_spec(kind, poster, path.as_deref(), seconds);
    if cx.media.autoplay && matches!(kind, VideoKind::Gif | VideoKind::Round) {
        spec = spec.autoplay();
    }
    conv::video(spec)
}

/// A file small enough to download by itself ([`AUTO_SIZE`]); an unknown size counts as small.
fn small(store: &Store, file: &Value) -> bool {
    let size = file["id"]
        .as_i64()
        .and_then(|id| store.files.get(&id))
        .map(|known| known.size)
        .filter(|size| *size > 0)
        .or_else(|| file["size"].as_u64().filter(|size| *size > 0))
        .or_else(|| file["expected_size"].as_u64())
        .unwrap_or_default();
    size <= AUTO_SIZE
}

/// Someone else's voice or round message not listened to (watched) yet: TDLib's `is_listened`
/// (`is_viewed`) is false.
fn unheard(content: &Value, key: &str, out: &Converted) -> bool {
    content[key] == false && !out.message.outgoing
}

/// Telegram's waveform of a voice message: 5-bit samples packed LSB-first (as `voiceNote.waveform`
/// carries them, base64 in TDLib's JSON) → values 0–31; as many as whole 5 bits fit.
pub fn waveform(packed: &[u8]) -> Vec<u8> {
    let count = packed.len() * 8 / 5;
    (0..count)
        .map(|index| {
            let bit = index * 5;
            let byte = bit / 8;
            let pair = u16::from(packed[byte])
                | packed.get(byte + 1).map_or(0, |next| u16::from(*next) << 8);
            ((pair >> (bit % 8)) & 0x1f) as u8
        })
        .collect()
}

/// A thumbnail format Flux decodes as a still picture.
fn still(format: &Value) -> bool {
    matches!(
        format["@type"].as_str().unwrap_or_default(),
        "thumbnailFormatJpeg" | "thumbnailFormatPng" | "thumbnailFormatWebp" | "thumbnailFormatGif"
    )
}

/// A file part: its name, size, path once downloaded, progress while it downloads or uploads.
fn file_part(store: &Store, file: &Value, name: &str, out: &mut Converted) {
    let id = file["id"].as_i64();
    let size = id
        .and_then(|id| store.files.get(&id))
        .map(|known| known.size)
        .filter(|size| *size > 0)
        .or_else(|| file["size"].as_u64().filter(|size| *size > 0))
        .or_else(|| file["expected_size"].as_u64())
        .unwrap_or_default();
    let path = file_path(store, file);
    let mut part = conv::file(name, size, path.as_deref());
    if let (Part::File(spec), Some(progress)) = (&mut part, file_progress(store, file)) {
        spec.progress = Some(progress);
    }
    if let Some(id) = id {
        out.files.push(id);
    }
    push(out, part, id);
}

/// The photo's size for the feed (the largest up to `side`), its file: downloaded — the path,
/// else the blurred minithumbnail and the progress. The largest size, once downloaded, is the
/// viewer's.
fn photo_spec(
    store: &Store,
    photo: &Value,
    side: i64,
    auto: bool,
    out: &mut Converted,
) -> Option<(ImageSpec, i64)> {
    let sizes: Vec<&Value> = photo["sizes"]
        .as_array()?
        .iter()
        .filter(|size| {
            size["width"].as_i64().unwrap_or_default() > 0 && size["photo"]["id"].is_i64()
        })
        .collect();
    let longest = |size: &Value| {
        size["width"]
            .as_i64()
            .unwrap_or_default()
            .max(size["height"].as_i64().unwrap_or_default())
    };
    let shown = sizes
        .iter()
        .filter(|size| longest(size) <= side)
        .max_by_key(|size| longest(size))
        .or_else(|| sizes.iter().min_by_key(|size| longest(size)))?;
    let largest = sizes.iter().max_by_key(|size| longest(size))?;
    let file = &shown["photo"];
    let id = file["id"].as_i64()?;
    let (width, height) = (
        shown["width"].as_u64().unwrap_or_default() as u32,
        shown["height"].as_u64().unwrap_or_default() as u32,
    );
    out.files.push(id);
    if auto {
        out.auto.push(id);
    }
    let mut spec = match file_path(store, file) {
        Some(path) => conv::picture(&path, width, height),
        None => pending(store, file, &photo["minithumbnail"], width, height),
    };
    let full = &largest["photo"];
    if let Some(full_id) = full["id"].as_i64().filter(|full_id| *full_id != id) {
        out.files.push(full_id);
        if let Some(path) = file_path(store, full) {
            spec = spec.full(&path);
        }
    }
    Some((spec, id))
}

/// The file of a photo's largest size: the viewer's.
fn full_size(photo: &Value) -> Option<i64> {
    photo["sizes"]
        .as_array()?
        .iter()
        .filter(|size| size["photo"]["id"].is_i64())
        .max_by_key(|size| {
            let side = |key: &str| size[key].as_i64().unwrap_or_default();
            side("width").max(side("height"))
        })?["photo"]["id"]
        .as_i64()
}

/// A picture not there yet: the blurred minithumbnail, the progress of its download.
fn pending(store: &Store, file: &Value, mini: &Value, width: u32, height: u32) -> ImageSpec {
    let mut spec = conv::pending_picture(width, height);
    if let Some(bytes) = minithumbnail(mini) {
        spec = spec.preview(bytes);
    }
    if let Some(progress) = file_progress(store, file) {
        spec = spec.progress(progress);
    }
    spec
}

/// A video's, a GIF's or a round message's poster: its still thumbnail once downloaded, else
/// the minithumbnail; the progress is the video file's when it downloads (the pill and ▶ are
/// Flux's).
fn media_preview(
    store: &Store,
    media: &Value,
    (width, height): (u32, u32),
    video: Option<i64>,
    out: &mut Converted,
) -> ImageSpec {
    if let Some(video) = video {
        out.files.push(video);
    }
    let thumbnail = &media["thumbnail"];
    let thumbnail_file = still(&thumbnail["format"])
        .then_some(&thumbnail["file"])
        .filter(|file| file["id"].is_i64());
    let mut spec = match thumbnail_file {
        Some(file) => {
            let id = file["id"].as_i64().unwrap_or_default();
            out.files.push(id);
            out.auto.push(id);
            match file_path(store, file) {
                Some(path) => conv::picture(&path, width, height),
                None => pending(store, &Value::Null, &media["minithumbnail"], width, height),
            }
        }
        None => pending(store, &Value::Null, &media["minithumbnail"], width, height),
    };
    let progress = video
        .and_then(|video| store.files.get(&video))
        .and_then(|file| file.progress());
    spec.progress = progress;
    spec
}

/// The size of a video or a GIF, in pixels.
fn size_of(media: &Value) -> (u32, u32) {
    (
        media["width"].as_u64().unwrap_or_default() as u32,
        media["height"].as_u64().unwrap_or_default() as u32,
    )
}

/// A link's preview of a text message: the site, the title, the description, the picture.
fn link_preview(store: &Store, preview: &Value, out: &mut Converted) {
    let Some(url) = preview["url"].as_str().filter(|url| !url.is_empty()) else {
        return;
    };
    let text = |key: &str| {
        preview[key]
            .as_str()
            .or_else(|| preview[key]["text"].as_str())
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    // TDLib 1.8.61 has the photo in the preview's `type`; older ones — in the preview itself.
    let photo = [&preview["type"]["photo"], &preview["photo"]]
        .into_iter()
        .find(|photo| photo.is_object());
    let image = photo.and_then(|photo| photo_spec(store, photo, PREVIEW_SIDE, true, out));
    let file = image.as_ref().map(|(_, file)| *file);
    let part = conv::link_preview(
        url,
        text("site_name").as_deref(),
        text("title").as_deref(),
        text("description").as_deref(),
        image.map(|(spec, _)| spec),
    );
    push(out, part, file);
}

/// The path of a file: the store's knowledge first (it follows `updateFile`), else the message's.
fn file_path(store: &Store, file: &Value) -> Option<String> {
    match file["id"].as_i64().and_then(|id| store.files.get(&id)) {
        Some(known) => known.path.clone(),
        None => downloaded(file),
    }
}

fn file_progress(store: &Store, file: &Value) -> Option<f32> {
    match file["id"].as_i64().and_then(|id| store.files.get(&id)) {
        Some(known) => known.progress(),
        None if file["local"]["is_downloading_active"] == true => {
            let size = file["size"].as_u64().filter(|size| *size > 0)?;
            let done = file["local"]["downloaded_size"]
                .as_u64()
                .unwrap_or_default();
            Some((done as f32 / size as f32).clamp(0.0, 1.0))
        }
        None => None,
    }
}

/// The tiny JPEG of a `minithumbnail` (base64 in TDLib's JSON).
fn minithumbnail(mini: &Value) -> Option<Vec<u8>> {
    let data = mini["data"].as_str()?;
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

/// A length: "0:42", "12:05", "1:02:03".
pub fn duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn reactions(value: &Value, out: &mut Converted) {
    let reactions = &value["interaction_info"]["reactions"];
    // TDLib 1.8.61: `messageReactions` with its list; older ones — the list itself.
    let list = reactions["reactions"]
        .as_array()
        .or_else(|| reactions.as_array());
    for reaction in list.into_iter().flatten() {
        let emoji = match reaction["type"]["@type"].as_str().unwrap_or_default() {
            "reactionTypeEmoji" => reaction["type"]["emoji"].as_str().unwrap_or_default(),
            "reactionTypePaid" => "⭐",
            _ => continue,
        };
        let count = reaction["total_count"].as_u64().unwrap_or_default() as u32;
        if emoji.is_empty() || count == 0 {
            continue;
        }
        out.message = out
            .message
            .clone()
            .reaction(emoji, count, reaction["is_chosen"] == true);
    }
}

/// The actions the message offers: Reply where the user may write; Edit and Delete on the
/// user's own messages, as far as Telegram allows — TDLib 1.8.61 dropped `can_be_edited` and
/// `can_be_deleted_*` from messages (they moved to `getMessageProperties`), so when they're
/// missing, Telegram's rules: the text and caption of a sent message within 48 hours (always in
/// Saved Messages), and deleting for everyone.
fn actions(cx: &Context, value: &Value, chat: i64, outgoing: bool) -> Vec<&'static str> {
    let mut actions = Vec::new();
    let sent = !value["sending_state"].is_object();
    if sent && cx.store.can_send(chat) {
        actions.push(REPLY);
    }
    if !outgoing {
        return actions;
    }
    let has_text = matches!(
        value["content"]["@type"].as_str().unwrap_or_default(),
        "messageText"
            | "messagePhoto"
            | "messageVideo"
            | "messageAnimation"
            | "messageDocument"
            | "messageAudio"
            | "messageVoiceNote"
    );
    let editable = match value["can_be_edited"].as_bool() {
        Some(can) => can,
        None => {
            sent && (cx.store.is_saved_messages(chat)
                || cx.now - value["date"].as_i64().unwrap_or_default() < EDIT_WINDOW)
        }
    };
    if editable && has_text {
        actions.push(EDIT);
    }
    let for_all = value["can_be_deleted_for_all_users"].as_bool();
    let for_me = value["can_be_deleted_only_for_self"].as_bool();
    match (for_all, for_me) {
        (Some(true), _) => actions.push(DELETE),
        (_, Some(true)) => actions.push(DELETE_FOR_ME),
        (Some(false), _) | (_, Some(false)) => {}
        // A message still on its way, or one that failed, goes only here.
        (None, None) if !sent || cx.store.is_saved_messages(chat) => actions.push(DELETE_FOR_ME),
        (None, None) => actions.push(DELETE),
    }
    actions
}

// --- Text with entities ---

/// A style an entity gives its piece of text.
#[derive(Debug, Clone, PartialEq)]
enum Style {
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Spoiler,
    Code,
    Link(String),
}

/// A block an entity makes of its text: a code block, a quote.
#[derive(Debug, Clone, PartialEq)]
enum Block {
    Code(Option<String>),
    Quote,
}

/// A `formattedText` as parts: text with its styles and links, code blocks and quotes between.
pub fn formatted(formatted: &Value) -> Vec<Part> {
    let text = formatted["text"].as_str().unwrap_or_default();
    if text.is_empty() {
        return Vec::new();
    }
    let bytes = Utf16::new(text);
    let mut styles: Vec<(usize, usize, Style)> = Vec::new();
    let mut blocks: Vec<(usize, usize, Block)> = Vec::new();
    for entity in formatted["entities"].as_array().into_iter().flatten() {
        let offset = entity["offset"].as_u64().unwrap_or_default() as usize;
        let length = entity["length"].as_u64().unwrap_or_default() as usize;
        let (start, end) = (bytes.at(offset), bytes.at(offset + length));
        if start >= end {
            continue;
        }
        let piece = &text[start..end];
        let kind = &entity["type"];
        let style = match kind["@type"].as_str().unwrap_or_default() {
            "textEntityTypeBold" => Style::Bold,
            "textEntityTypeItalic" => Style::Italic,
            "textEntityTypeUnderline" => Style::Underline,
            "textEntityTypeStrikethrough" => Style::Strikethrough,
            "textEntityTypeSpoiler" => Style::Spoiler,
            "textEntityTypeCode" => Style::Code,
            "textEntityTypeUrl" => Style::Link(if piece.contains("://") {
                piece.to_string()
            } else {
                format!("https://{piece}")
            }),
            "textEntityTypeTextUrl" => match kind["url"].as_str() {
                Some(url) if !url.is_empty() => Style::Link(url.to_string()),
                _ => continue,
            },
            "textEntityTypeEmailAddress" => Style::Link(format!("mailto:{piece}")),
            "textEntityTypeMention" => {
                Style::Link(format!("https://t.me/{}", piece.trim_start_matches('@')))
            }
            "textEntityTypeMentionName" => Style::Link(format!(
                "{USER_LINK}{}",
                kind["user_id"].as_i64().unwrap_or_default()
            )),
            "textEntityTypePhoneNumber" => {
                let digits: String = piece
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '+')
                    .collect();
                Style::Link(format!("tel:{digits}"))
            }
            "textEntityTypeBotCommand" => Style::Link(format!("{COMMAND_LINK}{piece}")),
            "textEntityTypePre" => {
                blocks.push((start, end, Block::Code(None)));
                continue;
            }
            "textEntityTypePreCode" => {
                let language = kind["language"]
                    .as_str()
                    .filter(|language| !language.is_empty())
                    .map(str::to_string);
                blocks.push((start, end, Block::Code(language)));
                continue;
            }
            "textEntityTypeBlockQuote" | "textEntityTypeExpandableBlockQuote" => {
                blocks.push((start, end, Block::Quote));
                continue;
            }
            // Hashtags, cashtags, custom emoji (their emoji is the text), card numbers: as text.
            _ => continue,
        };
        styles.push((start, end, style));
    }
    blocks.sort_by_key(|(start, _, _)| *start);
    let mut parts = Vec::new();
    let mut cursor = 0;
    let mut after_block = false;
    for (start, end, block) in blocks {
        if start < cursor {
            continue;
        }
        let before = spans(
            text,
            trim_lines(text, cursor, start, after_block, true),
            &styles,
        );
        if !before.is_empty() {
            parts.push(conv::text(before));
        }
        match block {
            Block::Code(language) => {
                let code = text[start..end].trim_end_matches('\n');
                parts.push(conv::code(code, language.as_deref()));
            }
            Block::Quote => {
                let range = trim_lines(text, start, end, true, true);
                let quoted = spans(text, range, &styles);
                if !quoted.is_empty() {
                    parts.push(conv::quote(quoted));
                }
            }
        }
        cursor = end;
        after_block = true;
    }
    let tail = spans(
        text,
        trim_lines(text, cursor, text.len(), after_block, after_block),
        &styles,
    );
    if !tail.is_empty() {
        parts.push(conv::text(tail));
    }
    parts
}

/// The text a message's field takes to edit it, as Markdown in TDLib's way (`parseMarkdown`
/// reads it back when it is saved): `**bold**`, `__italic__`, `~~strike~~`, `||spoiler||`,
/// `` `code` ``, `[text](url)` (a mention of someone without a username — `tg://user?id=`),
/// code blocks in ```` ``` ````; the links TDLib finds by itself (addresses, @usernames, mails,
/// numbers, commands), underlines and quotes stay plain text.
pub fn markdown(message: &Message) -> String {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Text(spans) | Part::Quote(spans) => Some(markdown_spans(spans)),
            Part::Code(code) => Some(match code.language.as_deref() {
                Some(language) => format!("```{language}\n{}\n```", code.text),
                None => format!("```\n{}\n```", code.text),
            }),
            _ => None,
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// A mark of Markdown over a piece of text, outermost first.
#[derive(Debug, Clone, PartialEq)]
enum Mark {
    Link(String),
    Bold,
    Italic,
    Strike,
    Spoiler,
    Code,
}

impl Mark {
    fn opening(&self) -> &'static str {
        match self {
            Mark::Link(_) => "[",
            Mark::Bold => "**",
            Mark::Italic => "__",
            Mark::Strike => "~~",
            Mark::Spoiler => "||",
            Mark::Code => "`",
        }
    }

    fn closing(&self) -> String {
        match self {
            Mark::Link(url) => format!("]({url})"),
            other => other.opening().to_string(),
        }
    }
}

/// The spans with their styles as marks: a mark opens where its style starts and closes where it
/// ends (the ones inside it closed and opened again around it).
fn markdown_spans(spans: &[RichSpan]) -> String {
    let mut out = String::new();
    let mut open: Vec<Mark> = Vec::new();
    for span in spans {
        let mut wanted = Vec::new();
        if let Some(link) = span.link.as_deref() {
            match link.strip_prefix(USER_LINK) {
                Some(user) => wanted.push(Mark::Link(format!("tg://user?id={user}"))),
                None if !found_by_tdlib(&span.text, link) => {
                    wanted.push(Mark::Link(link.to_string()))
                }
                None => {}
            }
        }
        for (on, mark) in [
            (span.bold, Mark::Bold),
            (span.italic, Mark::Italic),
            (span.strikethrough, Mark::Strike),
            (span.spoiler, Mark::Spoiler),
            (span.code, Mark::Code),
        ] {
            if on {
                wanted.push(mark);
            }
        }
        let kept = open
            .iter()
            .zip(&wanted)
            .take_while(|(open, wanted)| open == wanted)
            .count();
        while open.len() > kept {
            if let Some(mark) = open.pop() {
                out.push_str(&mark.closing());
            }
        }
        for mark in &wanted[kept..] {
            out.push_str(mark.opening());
            open.push(mark.clone());
        }
        out.push_str(&span.text);
    }
    while let Some(mark) = open.pop() {
        out.push_str(&mark.closing());
    }
    out
}

/// A link TDLib finds in the text by itself: an address, a @username, a mail, a phone number,
/// a bot's command — the text is enough.
fn found_by_tdlib(text: &str, link: &str) -> bool {
    link.starts_with(COMMAND_LINK)
        || link == text
        || link == format!("https://{text}")
        || link == format!("https://t.me/{}", text.trim_start_matches('@'))
        || link == format!("mailto:{text}")
        || link.strip_prefix("tel:").is_some_and(|number| {
            number
                == text
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '+')
                    .collect::<String>()
        })
}

/// A mention of a user without a username: the plugin's link, the person's chat on a click.
pub const USER_LINK: &str = "fluxgram:user/";
/// A bot's command: the plugin's link, the command into the field on a click.
pub const COMMAND_LINK: &str = "fluxgram:command/";

/// The range without the line breaks at its ends next to a block (`lead`, `tail`).
fn trim_lines(
    text: &str,
    mut start: usize,
    mut end: usize,
    lead: bool,
    tail: bool,
) -> (usize, usize) {
    let bytes = text.as_bytes();
    while lead && start < end && matches!(bytes[start], b'\n' | b'\r') {
        start += 1;
    }
    while tail && end > start && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    (start, end)
}

/// The text of `range` cut where the styles start and end: each piece with every style over it.
fn spans(
    text: &str,
    (start, end): (usize, usize),
    styles: &[(usize, usize, Style)],
) -> Vec<RichSpan> {
    if start >= end {
        return Vec::new();
    }
    let mut cuts = vec![start, end];
    for (from, to, _) in styles {
        for cut in [*from, *to] {
            if cut > start && cut < end {
                cuts.push(cut);
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut spans: Vec<RichSpan> = Vec::new();
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let mut span = rich(&text[from..to]);
        for (_, _, style) in styles
            .iter()
            .filter(|(style_from, style_to, _)| *style_from <= from && *style_to >= to)
        {
            span = match style {
                Style::Bold => span.bold(),
                Style::Italic => span.italic(),
                Style::Underline => span.underline(),
                Style::Strikethrough => span.strikethrough(),
                Style::Spoiler => span.spoiler(),
                Style::Code => span.code(),
                Style::Link(url) => span.link(url),
            };
        }
        // Pieces of the same style side by side are one span.
        match spans.last_mut() {
            Some(last) if same_style(last, &span) => last.text.push_str(&span.text),
            _ => spans.push(span),
        }
    }
    spans
}

fn same_style(a: &RichSpan, b: &RichSpan) -> bool {
    RichSpan {
        text: String::new(),
        ..a.clone()
    } == RichSpan {
        text: String::new(),
        ..b.clone()
    }
}

/// UTF-16 offsets (TDLib's) as byte offsets of the text, on character boundaries.
struct Utf16 {
    /// (the UTF-16 offset, the byte offset) of each character, and of the end.
    marks: Vec<(usize, usize)>,
}

impl Utf16 {
    fn new(text: &str) -> Self {
        let mut marks = Vec::with_capacity(text.len() + 1);
        let mut units = 0;
        for (byte, c) in text.char_indices() {
            marks.push((units, byte));
            units += c.len_utf16();
        }
        marks.push((units, text.len()));
        Utf16 { marks }
    }

    /// The byte offset of the UTF-16 one (inside a surrogate pair — its character's start).
    fn at(&self, units: usize) -> usize {
        let index = self.marks.partition_point(|(mark, _)| *mark <= units);
        self.marks[index.saturating_sub(1)].1
    }
}

// --- A chat's search: its words marked in the messages (the window's, while the search is open) ---

/// The words of a chat's search as they are matched: lowercase, without the spaces.
pub fn search_words(query: &str) -> Vec<Vec<char>> {
    query
        .split_whitespace()
        .map(|word| word.chars().flat_map(char::to_lowercase).collect())
        .collect()
}

/// Where the words are in `text`, as byte ranges in order: each word at the start of a word of the
/// text (Telegram's search matches words by their beginnings), in any case; the longest word
/// that fits at a place.
pub fn found_ranges(text: &str, words: &[Vec<char>]) -> Vec<std::ops::Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let byte_at = |index: usize| chars.get(index).map_or(text.len(), |(byte, _)| *byte);
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let after_word = index > 0 && chars[index - 1].1.is_alphanumeric();
        let end = words
            .iter()
            .filter(|word| !word.is_empty())
            // "#tag", "@anna", "c++" may start anywhere a word may.
            .filter(|word| !after_word || !word[0].is_alphanumeric())
            .filter_map(|word| match_at(&chars, index, word))
            .max();
        match end {
            Some(end) => {
                ranges.push(byte_at(index)..byte_at(end));
                index = end;
            }
            None => index += 1,
        }
    }
    ranges
}

/// The index after `word` matched at `start` (lowercase), if it does.
fn match_at(chars: &[(usize, char)], start: usize, word: &[char]) -> Option<usize> {
    let mut matched = 0;
    let mut index = start;
    while matched < word.len() {
        let (_, c) = *chars.get(index)?;
        for lower in c.to_lowercase() {
            if word.get(matched) != Some(&lower) {
                return None;
            }
            matched += 1;
        }
        index += 1;
    }
    Some(index)
}

/// Spans with the words marked (`highlight`): a span is cut where a match begins or ends; a
/// match may run over several spans.
pub fn highlight_spans(spans: &[RichSpan], words: &[Vec<char>]) -> Vec<RichSpan> {
    let text: String = spans.iter().map(|span| span.text.as_str()).collect();
    let ranges = found_ranges(&text, words);
    if ranges.is_empty() {
        return spans.to_vec();
    }
    let piece = |span: &RichSpan, text: &str, highlight: bool| RichSpan {
        text: text.to_string(),
        highlight: highlight || span.highlight,
        ..span.clone()
    };
    let mut out = Vec::new();
    let mut offset = 0;
    for span in spans {
        let end = offset + span.text.len();
        let mut at = offset;
        for range in ranges.iter().filter(|r| r.start < end && r.end > offset) {
            let (start, stop) = (range.start.max(offset), range.end.min(end));
            if start > at {
                out.push(piece(span, &span.text[at - offset..start - offset], false));
            }
            out.push(piece(span, &span.text[start - offset..stop - offset], true));
            at = stop;
        }
        if at < end {
            out.push(piece(span, &span.text[at - offset..], false));
        }
        offset = end;
    }
    out
}

/// The message with the search's words marked in its text and quotes; none — they aren't there.
pub fn highlighted(message: &Message, words: &[Vec<char>]) -> Option<Message> {
    if words.is_empty() {
        return None;
    }
    let mut marked = message.clone();
    let mut any = false;
    for part in &mut marked.parts {
        if let Part::Text(spans) | Part::Quote(spans) = part {
            let lit = highlight_spans(spans, words);
            if lit != *spans {
                any = true;
                *spans = lit;
            }
        }
    }
    any.then_some(marked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::English;
    use serde_json::json;

    const ME: i64 = 777;
    const GROUP: i64 = -1000000001001;

    fn file(id: i64, path: &str) -> Value {
        json!({"@type": "file", "id": id, "size": 1229, "expected_size": 1229,
            "local": {"path": path, "is_downloading_active": false,
                "is_downloading_completed": !path.is_empty(), "downloaded_size": 0},
            "remote": {"is_uploading_active": false, "uploaded_size": 0,
                "unique_id": format!("u{id}")}})
    }

    fn store() -> Store {
        let mut store = Store {
            me: Some(ME),
            ..Store::default()
        };
        store.apply(&json!({"@type": "user", "id": 103, "first_name": "Petya",
            "last_name": "Smirnov", "accent_color_id": 0}));
        store.apply(&json!({"@type": "user", "id": 101, "first_name": "Anna",
            "last_name": "Smirnova", "accent_color_id": 6,
            "profile_photo": {"small": file(1101, "")}}));
        store.apply(
            &json!({"@type": "chat", "id": GROUP, "title": "Backend · team",
            "type": {"@type": "chatTypeSupergroup", "supergroup_id": 1001, "is_channel": false},
            "last_read_outbox_message_id": 200, "permissions": {"can_send_basic_messages": true}}),
        );
        store.apply(
            &json!({"@type": "chat", "id": 101, "title": "Anna Smirnova",
            "type": {"@type": "chatTypePrivate", "user_id": 101}}),
        );
        store.apply(
            &json!({"@type": "chat", "id": ME, "title": "Saved Messages",
            "type": {"@type": "chatTypePrivate", "user_id": ME}}),
        );
        store.apply(
            &json!({"@type": "supergroup", "id": 1001, "member_count": 5,
            "status": {"@type": "chatMemberStatusMember"}}),
        );
        store
    }

    fn message(id: i64, chat: i64, sender: i64, content: Value) -> Value {
        json!({"@type": "message", "id": id, "chat_id": chat,
            "sender_id": {"@type": "messageSenderUser", "user_id": sender},
            "is_outgoing": sender == ME, "date": 1_763_380_000, "edit_date": 0,
            "content": content})
    }

    fn text(text: &str, entities: Value) -> Value {
        json!({"@type": "messageText", "text": {"@type": "formattedText", "text": text,
            "entities": entities}})
    }

    fn entity(offset: usize, length: usize, kind: Value) -> Value {
        json!({"@type": "textEntity", "offset": offset, "length": length, "type": kind})
    }

    fn convert_with(store: &Store, value: &Value, replied: Option<&Value>) -> Converted {
        convert_media(store, value, replied, &MediaOptions::default())
    }

    fn convert_media(
        store: &Store,
        value: &Value,
        replied: Option<&Value>,
        media: &MediaOptions,
    ) -> Converted {
        let cx = Context {
            store,
            words: &English,
            now: 1_763_380_100,
            replied,
            media,
        };
        convert(&cx, value).unwrap()
    }

    fn spans_of(part: &Part) -> &[RichSpan] {
        match part {
            Part::Text(spans) | Part::Quote(spans) | Part::Notice(spans) => spans,
            other => panic!("not text: {other:?}"),
        }
    }

    #[test]
    fn reads_utf16_entities_nested_and_overlapping() {
        // "Привет 👋 " is 10 UTF-16 units (the emoji is two).
        let value = text(
            "Привет 👋 bold italic end",
            json!([
                entity(10, 11, json!({"@type": "textEntityTypeBold"})),
                entity(15, 10, json!({"@type": "textEntityTypeItalic"})),
                entity(7, 2, json!({"@type": "textEntityTypeSpoiler"})),
            ]),
        );
        let parts = formatted(&value["text"]);
        assert_eq!(parts.len(), 1);
        let spans = spans_of(&parts[0]);
        let pieces: Vec<(&str, bool, bool, bool)> = spans
            .iter()
            .map(|s| (s.text.as_str(), s.bold, s.italic, s.spoiler))
            .collect();
        assert_eq!(
            pieces,
            [
                ("Привет ", false, false, false),
                ("👋", false, false, true),
                (" ", false, false, false),
                ("bold ", true, false, false),
                ("italic", true, true, false),
                (" end", false, true, false),
            ]
        );
    }

    #[test]
    fn makes_links() {
        let value = text(
            "see example.com, @anna, Petya, mail a@b.c, /start, +7 900 #tag",
            json!([
                entity(4, 11, json!({"@type": "textEntityTypeUrl"})),
                entity(17, 5, json!({"@type": "textEntityTypeMention"})),
                entity(
                    24,
                    5,
                    json!({"@type": "textEntityTypeMentionName", "user_id": 103})
                ),
                entity(36, 5, json!({"@type": "textEntityTypeEmailAddress"})),
                entity(43, 6, json!({"@type": "textEntityTypeBotCommand"})),
                entity(51, 6, json!({"@type": "textEntityTypePhoneNumber"})),
                entity(58, 4, json!({"@type": "textEntityTypeHashtag"})),
            ]),
        );
        let parts = formatted(&value["text"]);
        let links: Vec<(&str, &str)> = spans_of(&parts[0])
            .iter()
            .filter_map(|s| Some((s.text.as_str(), s.link.as_deref()?)))
            .collect();
        assert_eq!(
            links,
            [
                ("example.com", "https://example.com"),
                ("@anna", "https://t.me/anna"),
                ("Petya", "fluxgram:user/103"),
                ("a@b.c", "mailto:a@b.c"),
                ("/start", "fluxgram:command//start"),
                ("+7 900", "tel:+7900"),
            ]
        );
    }

    #[test]
    fn splits_code_blocks_and_quotes() {
        let source = "Here is the fix:\nfn main() {}\nAnd a quote:\nMeasure first\nDone";
        let value = text(
            source,
            json!([
                entity(
                    17,
                    13,
                    json!({"@type": "textEntityTypePreCode", "language": "rust"})
                ),
                entity(43, 14, json!({"@type": "textEntityTypeBlockQuote"})),
                entity(43, 7, json!({"@type": "textEntityTypeBold"})),
            ]),
        );
        let parts = formatted(&value["text"]);
        assert_eq!(parts.len(), 5, "{parts:?}");
        assert_eq!(spans_of(&parts[0])[0].text, "Here is the fix:");
        match &parts[1] {
            Part::Code(code) => {
                assert_eq!(code.text, "fn main() {}");
                assert_eq!(code.language.as_deref(), Some("rust"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(spans_of(&parts[2])[0].text, "And a quote:");
        let quote = spans_of(&parts[3]);
        assert_eq!((quote[0].text.as_str(), quote[0].bold), ("Measure", true));
        assert_eq!(quote[1].text, " first");
        assert_eq!(spans_of(&parts[4])[0].text, "Done");
    }

    #[test]
    fn names_authors_in_groups_only() {
        let store = store();
        let incoming = convert_with(
            &store,
            &message(100, GROUP, 101, text("Hi", json!([]))),
            None,
        );
        let author = incoming.message.author.clone().unwrap();
        assert_eq!((author.name.as_str(), author.color), ("Anna Smirnova", 6));
        assert!(author.avatar.is_some());
        // Her photo isn't downloaded: the feed follows it and downloads it in sight.
        assert_eq!(incoming.auto, [1101]);
        let private = convert_with(&store, &message(5, 101, 101, text("Hi", json!([]))), None);
        assert!(private.message.author.is_none());
        let stranger = convert_with(&store, &message(6, GROUP, 999, text("Hi", json!([]))), None);
        assert_eq!(stranger.missing_users, [999]);
        assert_eq!(private.message.actions, Some(vec![REPLY.into()]));
    }

    #[test]
    fn marks_delivery_and_offers_actions() {
        let store = store();
        let read = convert_with(
            &store,
            &message(200, GROUP, ME, text("Mine", json!([]))),
            None,
        );
        assert_eq!(read.message.delivery, Some(Delivery::Read));
        assert_eq!(
            read.message.actions,
            Some(vec![REPLY.into(), EDIT.into(), DELETE.into()])
        );
        let sent = convert_with(
            &store,
            &message(201, GROUP, ME, text("Mine", json!([]))),
            None,
        );
        assert_eq!(sent.message.delivery, Some(Delivery::Sent));
        let mut pending = message(202, GROUP, ME, text("Mine", json!([])));
        pending["sending_state"] = json!({"@type": "messageSendingStatePending"});
        let pending = convert_with(&store, &pending, None);
        assert_eq!(pending.message.delivery, Some(Delivery::Sending));
        assert_eq!(pending.message.actions, Some(vec![DELETE_FOR_ME.into()]));
        // Older than two days: no Edit; TDLib's own word wins when it gives one.
        let mut old = message(150, GROUP, ME, text("Old", json!([])));
        old["date"] = json!(1_763_380_000 - 3 * 86_400);
        let actions = convert_with(&store, &old, None).message.actions.unwrap();
        assert!(!actions.contains(&EDIT.to_string()));
        old["can_be_edited"] = json!(true);
        old["can_be_deleted_for_all_users"] = json!(false);
        old["can_be_deleted_only_for_self"] = json!(true);
        let actions = convert_with(&store, &old, None).message.actions.unwrap();
        assert_eq!(actions, [REPLY, EDIT, DELETE_FOR_ME]);
        // Someone else's message: Reply only.
        let theirs = convert_with(&store, &message(9, GROUP, 103, text("x", json!([]))), None);
        assert_eq!(theirs.message.actions, Some(vec![REPLY.into()]));
    }

    #[test]
    fn quotes_replies() {
        let store = store();
        let original = message(
            90,
            GROUP,
            103,
            text("Release on Friday?\nOr not", json!([])),
        );
        let mut answer = message(100, GROUP, 101, text("Ok", json!([])));
        answer["reply_to"] = json!({"@type": "messageReplyToMessage", "chat_id": GROUP,
            "message_id": 90, "quote": null});
        let quoted = convert_with(&store, &answer, Some(&original));
        let quote = quoted.message.reply.clone().unwrap();
        assert_eq!(quote.author, "Petya Smirnov");
        assert_eq!(quote.text, "Release on Friday?");
        assert_eq!(quote.message.as_deref(), Some("90"));
        assert_eq!(quoted.reply_to, None);
        // Not loaded: "…", and the feed fetches it.
        let waiting = convert_with(&store, &answer, None);
        assert_eq!(waiting.reply_to, Some(90));
        assert_eq!(waiting.message.reply.unwrap().text, "…");
        // The user's own message quoted: "You".
        let mine = message(91, GROUP, ME, text("Mine", json!([])));
        answer["reply_to"]["message_id"] = json!(91);
        let quote = convert_with(&store, &answer, Some(&mine))
            .message
            .reply
            .unwrap();
        assert_eq!(quote.author, "You");
        // Another chat's message: its origin's name, TDLib's quote.
        answer["reply_to"] = json!({"@type": "messageReplyToMessage", "chat_id": 101,
            "message_id": 5, "origin": {"@type": "messageOriginUser", "sender_user_id": 101},
            "quote": {"text": {"text": "the piece"}}});
        let quote = convert_with(&store, &answer, None).message.reply.unwrap();
        assert_eq!(
            (quote.author.as_str(), quote.text.as_str()),
            ("Anna Smirnova", "the piece")
        );
        assert_eq!(quote.message, None);
    }

    #[test]
    fn forwards_and_edits() {
        let store = store();
        let mut value = message(100, GROUP, 103, text("Worth repeating", json!([])));
        value["forward_info"] = json!({"@type": "messageForwardInfo", "date": 1,
            "origin": {"@type": "messageOriginHiddenUser", "sender_name": "Ivan"}});
        value["edit_date"] = json!(1_763_380_050);
        let converted = convert_with(&store, &value, None);
        assert_eq!(converted.message.forwarded.as_deref(), Some("Ivan"));
        assert!(converted.message.edited);
        value["forward_info"]["origin"] =
            json!({"@type": "messageOriginChannel", "chat_id": -1000000001002i64, "message_id": 1});
        let converted = convert_with(&store, &value, None);
        assert_eq!(converted.missing_chats, [-1000000001002i64]);
    }

    #[test]
    fn shows_photos_with_their_preview_and_caption() {
        let mut store = store();
        let mini = base64::engine::general_purpose::STANDARD.encode([0xff, 0xd8, 0xff, 0xd9]);
        let photo = json!({"@type": "messagePhoto", "caption": {"text": "The map", "entities": []},
            "photo": {"minithumbnail": {"width": 40, "height": 30, "data": mini},
            "sizes": [
                {"type": "m", "width": 320, "height": 240, "photo": file(31, "")},
                {"type": "x", "width": 800, "height": 600, "photo": file(32, "")},
                {"type": "w", "width": 2560, "height": 1920, "photo": file(33, "")}]}});
        let value = message(100, GROUP, 103, photo);
        let converted = convert_with(&store, &value, None);
        let Part::Image(spec) = &converted.message.parts[0] else {
            panic!("{:?}", converted.message.parts)
        };
        assert_eq!(
            (spec.width, spec.height, spec.path.clone()),
            (800, 600, None)
        );
        assert_eq!(spec.preview.as_deref(), Some(&[0xff, 0xd8, 0xff, 0xd9][..]));
        assert_eq!(converted.part_files, [Some(32), None]);
        assert_eq!(spans_of(&converted.message.parts[1])[0].text, "The map");
        assert!(converted.auto.contains(&32) && !converted.auto.contains(&33));
        // The full size: after the pictures in sight, until it is there.
        assert_eq!(converted.full, [33]);
        // Downloading, then there: the progress, then the path.
        let mut downloading = file(32, "");
        downloading["local"]["is_downloading_active"] = json!(true);
        downloading["local"]["downloaded_size"] = json!(600);
        downloading["size"] = json!(1200);
        store.apply(&json!({"@type": "updateFile", "file": downloading}));
        let converted = convert_with(&store, &value, None);
        let Part::Image(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(spec.progress, Some(0.5));
        store.apply(&json!({"@type": "updateFile", "file": file(32, "/files/32.jpg")}));
        let converted = convert_with(&store, &value, None);
        let Part::Image(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(spec.path.as_deref(), Some("/files/32.jpg"));
        assert_eq!(spec.preview, None);
    }

    #[test]
    fn unpacks_waveforms() {
        // 31 in bits 0–4, 1 in bits 5–9: 63 and 0; the third sample is what is left (0).
        assert_eq!(waveform(&[63, 0]), [31, 1, 0]);
        assert_eq!(waveform(&[0xff, 0xff]), [31, 31, 31]);
        assert!(waveform(&[]).is_empty());
        // Packed LSB-first, as Telegram does: any samples come back as they were.
        let samples: Vec<u8> = (0..100).map(|i| (i * 7 % 32) as u8).collect();
        let mut packed = vec![0u8; samples.len() * 5 / 8 + 1];
        for (index, sample) in samples.iter().enumerate() {
            let bit = index * 5;
            let pair = u16::from(*sample) << (bit % 8);
            packed[bit / 8] |= pair as u8;
            if bit / 8 + 1 < packed.len() {
                packed[bit / 8 + 1] |= (pair >> 8) as u8;
            }
        }
        assert_eq!(waveform(&packed)[..samples.len()], samples[..]);
    }

    #[test]
    fn shows_media_to_play() {
        let mut store = store();
        let media = MediaOptions {
            frames: "/cache/frames".into(),
            ..MediaOptions::default()
        };
        let video = json!({"@type": "messageVideo", "caption": {"text": ""},
            "video": {"duration": 65, "width": 1280, "height": 720,
            "thumbnail": {"format": {"@type": "thumbnailFormatJpeg"}, "width": 320, "height": 180,
                "file": file(41, "")}, "video": file(42, "")}});
        let converted = convert_media(&store, &message(1, GROUP, 103, video), None, &media);
        let Part::Video(spec) = &converted.message.parts[0] else {
            panic!("{:?}", converted.message.parts)
        };
        assert_eq!(
            (spec.kind, spec.duration, spec.path.clone(), spec.autoplay),
            (VideoKind::Video, 65, None, false)
        );
        // The pill and ▶ are Flux's: the poster has neither.
        assert_eq!((spec.poster.label.clone(), spec.poster.play), (None, false));
        assert_eq!((spec.poster.width, spec.poster.height), (1280, 720));
        assert_eq!(converted.part_files, [Some(42)]);
        assert_eq!(converted.auto, [41], "the video waits for a click");

        // A round message: square, plays by itself, unheard until watched; its file downloads
        // by itself (with voice messages).
        let round = json!({"@type": "messageVideoNote", "is_viewed": false, "video_note": {
            "duration": 12, "length": 384, "thumbnail": null,
            "minithumbnail": {"data": "AAAA"}, "video": file(43, "")}});
        let converted = convert_media(&store, &message(2, GROUP, 103, round.clone()), None, &media);
        let Part::Video(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(
            (spec.kind, spec.autoplay, spec.unheard),
            (VideoKind::Round, true, true)
        );
        assert_eq!((spec.poster.width, spec.poster.height), (240, 240));
        assert!(spec.poster.preview.is_some());
        assert_eq!(converted.auto, [43]);
        // Autoplay off, voice downloads off: ▶ on the poster, a click downloads it.
        let quiet = MediaOptions {
            autoplay: false,
            voice: false,
            ..media.clone()
        };
        let converted = convert_media(&store, &message(2, GROUP, 103, round), None, &quiet);
        let Part::Video(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert!(!spec.autoplay);
        assert!(converted.auto.is_empty());

        // A GIF: plays by itself; the user's own round isn't "unheard".
        let gif = json!({"@type": "messageAnimation", "caption": {"text": "lol"},
            "animation": {"duration": 3, "width": 480, "height": 270, "thumbnail": null,
            "animation": file(44, "/f/44.mp4")}});
        let converted = convert_media(&store, &message(3, GROUP, ME, gif), None, &media);
        let Part::Video(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(
            (spec.kind, spec.autoplay, spec.path.as_deref()),
            (VideoKind::Gif, true, Some("/f/44.mp4"))
        );
        assert_eq!(spans_of(&converted.message.parts[1])[0].text, "lol");

        // A voice message: its waveform, unheard; downloading — its progress, then the path.
        let wave = base64::engine::general_purpose::STANDARD.encode([63, 0]);
        let voice = json!({"@type": "messageVoiceNote", "caption": {"text": ""},
            "is_listened": false, "voice_note": {"duration": 42, "waveform": wave,
            "voice": file(45, "")}});
        let value = message(4, GROUP, 103, voice);
        let converted = convert_media(&store, &value, None, &media);
        let Part::Voice(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(
            (
                spec.duration,
                spec.waveform.clone(),
                spec.unheard,
                spec.path.clone()
            ),
            (42, vec![31, 1, 0], true, None)
        );
        assert_eq!(converted.auto, [45]);
        assert_eq!(converted.part_files, [Some(45)]);
        let mut downloading = file(45, "");
        downloading["local"]["is_downloading_active"] = json!(true);
        downloading["local"]["downloaded_size"] = json!(300);
        downloading["size"] = json!(1200);
        store.apply(&json!({"@type": "updateFile", "file": downloading}));
        let converted = convert_media(&store, &value, None, &media);
        let Part::Voice(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(spec.progress, Some(0.25));
        store.apply(&json!({"@type": "updateFile", "file": file(45, "/f/45.ogg")}));
        let converted = convert_media(&store, &value, None, &media);
        let Part::Voice(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(
            (spec.path.as_deref(), spec.progress),
            (Some("/f/45.ogg"), None)
        );
    }

    #[test]
    fn animates_stickers_once_rendered() {
        let mut store = store();
        let mut media = MediaOptions {
            frames: "/cache/frames".into(),
            ..MediaOptions::default()
        };
        let sticker = json!({"@type": "messageSticker", "sticker": {"width": 512, "height": 512,
            "emoji": "🚀", "format": {"@type": "stickerFormatTgs"},
            "thumbnail": {"format": {"@type": "thumbnailFormatWebp"}, "file": file(46, "")},
            "sticker": file(47, "")}});
        let value = message(5, GROUP, 103, sticker);
        let converted = convert_media(&store, &value, None, &media);
        let Part::Sticker(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(spec.animation, None);
        assert_eq!(converted.part_files, [Some(46)]);
        // The thumbnail and the sticker's file download by themselves; the render waits for
        // the file.
        assert_eq!(converted.auto, [46, 47]);
        let render = &converted.renders[0];
        assert_eq!(
            (
                render.file,
                render.input.clone(),
                render.output.as_str(),
                render.size
            ),
            (47, None, "/cache/frames/u47-320.fxa", 320)
        );
        store.apply(&json!({"@type": "updateFile", "file": file(47, "/f/47.tgs")}));
        let converted = convert_media(&store, &value, None, &media);
        assert_eq!(converted.renders[0].input.as_deref(), Some("/f/47.tgs"));
        // Rendered: the animation over the picture, looped as the setting says.
        media.rendered.insert("/cache/frames/u47-320.fxa".into());
        media.loop_stickers = false;
        let converted = convert_media(&store, &value, None, &media);
        let Part::Sticker(spec) = &converted.message.parts[0] else {
            panic!()
        };
        let animation = spec.animation.clone().unwrap();
        assert_eq!(
            (animation.path.as_str(), animation.looped),
            ("/cache/frames/u47-320.fxa", false)
        );
        assert!(converted.renders.is_empty());
        // An animated emoji: its sticker the same way; without one — the emoji as text.
        let emoji = json!({"@type": "messageAnimatedEmoji", "emoji": "❤",
            "animated_emoji": {"sticker": {"width": 512, "height": 512, "emoji": "❤",
                "format": {"@type": "stickerFormatTgs"},
                "thumbnail": {"format": {"@type": "thumbnailFormatWebp"}, "file": file(48, "")},
                "sticker": file(49, "")}}});
        let converted = convert_media(&store, &message(6, GROUP, 103, emoji), None, &media);
        assert!(matches!(converted.message.parts[0], Part::Sticker(_)));
        assert_eq!(converted.renders[0].output, "/cache/frames/u49-320.fxa");
        let plain = json!({"@type": "messageAnimatedEmoji", "emoji": "❤",
            "animated_emoji": {"sticker": null}});
        let converted = convert_media(&store, &message(7, GROUP, 103, plain), None, &media);
        assert_eq!(spans_of(&converted.message.parts[0])[0].text, "❤");
        // A static sticker as it is; a large GIF waits for a click.
        let webp = json!({"@type": "messageSticker", "sticker": {"width": 512, "height": 512,
            "emoji": "👍", "format": {"@type": "stickerFormatWebp"}, "thumbnail": null,
            "sticker": file(50, "/f/50.webp")}});
        let converted = convert_media(&store, &message(8, GROUP, 103, webp), None, &media);
        let Part::Sticker(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(
            (spec.path.as_deref(), spec.animation.clone()),
            (Some("/f/50.webp"), None)
        );
        let mut large = file(51, "");
        large["size"] = json!(40 << 20);
        let gif = json!({"@type": "messageAnimation", "caption": {"text": ""},
            "animation": {"duration": 3, "width": 480, "height": 270, "thumbnail": null,
            "animation": large}});
        let converted = convert_media(&store, &message(9, GROUP, 103, gif), None, &media);
        assert!(converted.auto.is_empty());
    }

    #[test]
    fn shows_files_and_placeholders() {
        let store = store();
        let document = json!({"@type": "messageDocument", "caption": {"text": ""},
            "document": {"file_name": "staging.env.example", "document": file(45, "/f/s.env")}});
        let converted = convert_with(&store, &message(4, GROUP, 103, document), None);
        let Part::File(spec) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(
            (spec.name.as_str(), spec.size, spec.path.as_deref()),
            ("staging.env.example", 1229, Some("/f/s.env"))
        );
        let poll = json!({"@type": "messagePoll", "poll": {"question": {"text": "Lunch?"}}});
        let converted = convert_with(&store, &message(6, GROUP, 103, poll), None);
        let Part::Placeholder(placeholder) = &converted.message.parts[0] else {
            panic!()
        };
        assert_eq!(placeholder.text, "📊 Lunch?");
        let joined = json!({"@type": "messageChatAddMembers", "member_user_ids": [103]});
        let converted = convert_with(&store, &message(7, GROUP, 101, joined), None);
        assert!(matches!(converted.message.parts[0], Part::Notice(_)));
        assert!(converted.message.author.is_none());
    }

    #[test]
    fn shows_reactions_and_link_previews() {
        let store = store();
        let mut value = text(
            "https://example.com/blog",
            json!([entity(0, 24, json!({"@type": "textEntityTypeUrl"}))]),
        );
        value["link_preview"] = json!({"@type": "linkPreview", "url": "https://example.com/blog",
            "site_name": "Example", "title": "Release notes",
            "description": {"text": "What is new", "entities": []},
            "type": {"@type": "linkPreviewTypeArticle", "photo": {"sizes": [
                {"type": "m", "width": 320, "height": 200, "photo": file(51, "")}]}}});
        let mut value = message(1, GROUP, 103, value);
        value["interaction_info"] = json!({"reactions": {"reactions": [
            {"type": {"@type": "reactionTypeEmoji", "emoji": "🔥"}, "total_count": 2, "is_chosen": true},
            {"type": {"@type": "reactionTypeCustomEmoji", "custom_emoji_id": "1"}, "total_count": 1}]}});
        let converted = convert_with(&store, &value, None);
        let Part::Link(link) = &converted.message.parts[1] else {
            panic!()
        };
        assert_eq!(link.title.as_deref(), Some("Release notes"));
        assert_eq!(link.description.as_deref(), Some("What is new"));
        assert!(link.image.is_some());
        assert_eq!(converted.message.reactions.len(), 1);
        assert_eq!(converted.message.reactions[0].count, 2);
        assert!(converted.message.reactions[0].chosen);
    }

    #[test]
    fn shows_an_album_as_one_message() {
        let store = store();
        let photo = |file_id: i64, caption: &str| {
            json!({"@type": "messagePhoto", "caption": {"text": caption, "entities": []},
                "photo": {"sizes": [{"type": "x", "width": 800, "height": 600,
                "photo": file(file_id, "")}]}})
        };
        let video = json!({"@type": "messageVideo", "caption": {"text": ""},
            "video": {"duration": 65, "width": 1280, "height": 720,
            "thumbnail": {"format": {"@type": "thumbnailFormatJpeg"}, "width": 320, "height": 180,
                "file": file(41, "")}, "video": file(42, "")}});
        let mut items = vec![
            message(10, GROUP, 101, photo(31, "")),
            message(11, GROUP, 101, video),
            message(12, GROUP, 101, photo(33, "The map, at last")),
        ];
        for item in &mut items {
            item["media_album_id"] = json!("13800000000000077");
        }
        items[0]["reply_to"] = json!({"@type": "messageReplyToMessage", "chat_id": GROUP,
            "message_id": 5, "quote": null});
        items[2]["interaction_info"] = json!({"reactions": {"reactions": [
            {"type": {"@type": "reactionTypeEmoji", "emoji": "🔥"}, "total_count": 2,
            "is_chosen": false}]}});
        assert_eq!(album_id(&items[0]), 13_800_000_000_000_077);
        assert_eq!(album_id(&json!({"media_album_id": "0"})), 0);
        assert_eq!(album_id(&json!({"media_album_id": -5})), -5);
        let media = MediaOptions::default();
        let cx = Context {
            store: &store,
            words: &English,
            now: 1_763_380_100,
            replied: None,
            media: &media,
        };
        let refs: Vec<&Value> = items.iter().collect();
        let shown = album(&cx, &refs).unwrap();
        assert_eq!(shown.message.id, "10");
        // The pictures one after another (a grid), the caption — the last item's — after them.
        assert!(
            matches!(
                shown.message.parts[..],
                [
                    Part::Image(_),
                    Part::Video(_),
                    Part::Image(_),
                    Part::Text(_)
                ]
            ),
            "{:?}",
            shown.message.parts
        );
        let Part::Video(clip) = &shown.message.parts[1] else {
            panic!()
        };
        assert_eq!((clip.kind, clip.duration), (VideoKind::Video, 65));
        assert_eq!(shown.part_files, [Some(31), Some(42), Some(33), None]);
        assert_eq!(
            spans_of(&shown.message.parts[3])[0].text,
            "The map, at last"
        );
        assert_eq!(
            shown.message.author.as_ref().map(|a| a.name.as_str()),
            Some("Anna Smirnova")
        );
        // The captioned item's reactions, the first one's quote.
        assert_eq!(shown.message.reactions.len(), 1);
        assert_eq!(shown.reply_to, Some(5));
        assert_eq!(shown.message.actions, Some(vec![REPLY.into()]));
        // The caption above, as Telegram shows it then.
        items[2]["content"]["show_caption_above_media"] = json!(true);
        let refs: Vec<&Value> = items.iter().collect();
        let above = album(&cx, &refs).unwrap();
        assert!(matches!(above.message.parts[0], Part::Text(_)));
        assert_eq!(above.part_files, [None, Some(31), Some(42), Some(33)]);
        // One item is the message itself.
        assert_eq!(album(&cx, &refs[..1]), convert(&cx, refs[0]));

        // The user's own: Edit (the caption), Delete for everyone once every item is sent; the
        // clock while one is on its way.
        let mut mine = vec![
            message(300, GROUP, ME, photo(51, "")),
            message(301, GROUP, ME, photo(52, "")),
        ];
        for item in &mut mine {
            item["media_album_id"] = json!("-5");
        }
        let refs: Vec<&Value> = mine.iter().collect();
        let sent = album(&cx, &refs).unwrap();
        assert_eq!(sent.message.delivery, Some(Delivery::Sent));
        assert_eq!(
            sent.message.actions,
            Some(vec![REPLY.into(), EDIT.into(), DELETE.into()])
        );
        mine[1]["sending_state"] = json!({"@type": "messageSendingStatePending"});
        let refs: Vec<&Value> = mine.iter().collect();
        let sending = album(&cx, &refs).unwrap();
        assert_eq!(sending.message.delivery, Some(Delivery::Sending));
        assert_eq!(
            sending.message.actions,
            Some(vec![REPLY.into(), EDIT.into(), DELETE_FOR_ME.into()])
        );
    }

    #[test]
    fn gives_the_text_to_edit_as_markdown() {
        let message = Message::new("1", 1, 0)
            .part(conv::text(vec![
                rich("Hi "),
                rich("bold").bold(),
                rich(" both").bold().italic(),
                rich(" see "),
                rich("the docs").link("https://example.com/a"),
                rich(" at "),
                rich("site.com").link("https://site.com"),
                rich(", "),
                rich("Anna").link("fluxgram:user/101"),
                rich(", "),
                rich("@anna").link("https://t.me/anna"),
                rich(" "),
                rich("x").code(),
                rich(" "),
                rich("secret").spoiler().strikethrough(),
            ]))
            .part(conv::code("fn main() {}", Some("rust")));
        assert_eq!(
            markdown(&message),
            "Hi **bold__ both__** see [the docs](https://example.com/a) at site.com, \
             [Anna](tg://user?id=101), @anna `x` ~~||secret||~~\n```rust\nfn main() {}\n```"
        );
    }

    #[test]
    fn formats_lengths() {
        assert_eq!(duration(42), "0:42");
        assert_eq!(duration(725), "12:05");
        assert_eq!(duration(3723), "1:02:03");
    }

    #[test]
    fn marks_the_search_in_messages() {
        let words = search_words("Rel  ПЯТНИЦ");
        assert_eq!(words, vec![vec!['r', 'e', 'l'], "пятниц".chars().collect()]);
        // Words by their beginnings, in any case; not inside a word.
        let text = "Release on Пятница? unreliable release";
        let ranges = found_ranges(text, &words);
        let found: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(found, ["Rel", "Пятниц", "rel"]);
        assert!(found_ranges("#release and c++", &search_words("#rel ++")).len() == 2);
        assert!(found_ranges("anything", &search_words("")).is_empty());

        // A match over two spans of different styles; the styles stay.
        let spans = vec![rich("See the re"), rich("lease").bold(), rich(" notes")];
        let lit = highlight_spans(&spans, &search_words("release"));
        let shown: Vec<(&str, bool, bool)> = lit
            .iter()
            .map(|s| (s.text.as_str(), s.highlight, s.bold))
            .collect();
        assert_eq!(
            shown,
            [
                ("See the ", false, false),
                ("re", true, false),
                ("lease", true, true),
                (" notes", false, false)
            ]
        );
        let text: String = lit.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "See the release notes");

        // A message: its text and quotes marked; none when the words aren't there.
        let message = Message::new("1", 1, 0)
            .part(conv::text(vec![rich("Release on Friday")]))
            .part(conv::code("let release = 1;", None));
        let marked = highlighted(&message, &search_words("friday")).expect("marked");
        let Part::Text(spans) = &marked.parts[0] else {
            panic!()
        };
        assert_eq!(spans.len(), 2);
        assert!(spans[1].highlight && spans[1].text == "Friday");
        assert_eq!(marked.parts[1], message.parts[1], "code isn't marked");
        assert!(highlighted(&message, &search_words("monday")).is_none());
        assert!(highlighted(&message, &[]).is_none());
    }
}
