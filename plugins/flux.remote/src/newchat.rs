//! A new chat — agent B's. The user writes in the app's «New chat» tab: Telegram opens a topic with
//! an implicit name and puts the message there ([`on_topic_created`], then [`on_message`]). The
//! bot asks for the project — buttons of the projects Flux knows (open ones first, then recent;
//! «More…» pages) — keeps the first message, and on the choice opens a session in that project
//! with it ([`Host::new_session`](crate::host::Host::new_session)), binds the topic to the
//! session ([`crate::store::Store::add_topic`]), edits the question into the topic's first line
//! («📁 project · mode», kept as the topic's `head`: it follows the session's mode wherever it
//! changed — `/mode`, Flux, after a restart too) and renames the topic when the session has a title
//! ([`on_session`]). More
//! messages before the choice join the first one; photos and files come by [`crate::inbox`] and
//! wait with them ([`take`]). A message in a topic the bot knows no session of (one whose session
//! was closed) is asked the same. When a tracker gives tasks, «From a task» comes between the
//! project and the session ([`crate::task`]: it opens the session by [`open`] or binds the one its
//! action opened by [`bind`]).

use std::collections::BTreeMap;

use flux_plugin_api::remote::{Mode, Project, Session};
use serde_json::Value;

use crate::bot::Bot;
use crate::conversation;
use crate::general::folder_name;
use crate::host::{Host, NewSession, trf};
use crate::keys::{self, Kind};
use crate::render;
use crate::store::Topic;
use crate::telegram::{self, Button, CallbackQuery, Keyboard, Message, TgError};
use crate::transport::{Lane, Module, Purpose};

/// Projects on one page of the question.
const PAGE: usize = 6;

#[derive(Default)]
pub struct State {
    /// The chats that wait for their project, by key.
    pending: BTreeMap<u64, Pending>,
    /// The last key given (they start from the clock: a button of the last run means nothing).
    last_key: u64,
    /// The mode each topic's first line shows, by session (unknown after a restart: the next
    /// change of the session edits it).
    shown: BTreeMap<String, Mode>,
}

/// A new chat waiting for its project.
struct Pending {
    thread: i64,
    /// The user's messages so far: the session's first message.
    words: Vec<String>,
    /// Pictures that came with them: paths in the plugin's data folder.
    images: Vec<String>,
    /// The projects offered, as Flux named them when asked.
    projects: Vec<Project>,
    page: usize,
    /// The question's message, once Telegram took it.
    question: Option<i64>,
}

/// A new chat whose project is chosen: what its session starts with.
#[derive(Clone, Debug)]
pub struct Chat {
    pub thread: i64,
    pub project: Project,
    /// The user's messages so far.
    pub words: Vec<String>,
    /// Pictures: paths in the plugin's data folder.
    pub images: Vec<String>,
    /// The bot's message the chat's questions are in: it becomes the topic's first line.
    pub question: Option<i64>,
}

impl Chat {
    /// The user's words as one message.
    pub fn text(&self) -> String {
        self.words.join("\n\n")
    }
}

/// The user's new topic (its service message): nothing to do — its first message comes next.
pub fn on_topic_created(_bot: &mut Bot, _host: &mut dyn Host, _message: &Message) {}

/// A message in a topic without a session (a new chat's first words, or a topic the bot lost).
pub fn on_message(bot: &mut Bot, host: &mut dyn Host, message: &Message) {
    let Some(thread) = message.thread() else {
        return;
    };
    if crate::inbox::receive(bot, host, message) {
        return;
    }
    let words = message.words().trim();
    if words.is_empty() {
        return;
    }
    take(bot, host, thread, words, Vec::new());
}

/// The user's words and pictures in a topic without a session: they join the chat that waits for
/// its project (or its task — [`crate::task::take`]), or start one.
pub fn take(bot: &mut Bot, host: &mut dyn Host, thread: i64, words: &str, images: Vec<String>) {
    let words = words.trim();
    if crate::task::take(bot, host, thread, words, &images) {
        return;
    }
    if let Some(pending) = bot
        .newchat
        .pending
        .values_mut()
        .find(|pending| pending.thread == thread)
    {
        if !words.is_empty() {
            pending.words.push(words.to_string());
        }
        pending.images.extend(images);
        return;
    }
    let mut projects = host.projects().unwrap_or_default();
    // Open ones first, each part as Flux ordered it.
    projects.sort_by_key(|project| !project.open);
    if projects.is_empty() {
        let note = host.tr("Flux knows no projects yet: open one in Flux and write again.");
        send(
            bot,
            host,
            thread,
            &render::escape(&note),
            None,
            Purpose::none(),
        );
        return;
    }
    let key = next_key(bot, host);
    let pending = Pending {
        thread,
        words: if words.is_empty() {
            Vec::new()
        } else {
            vec![words.to_string()]
        },
        images,
        projects,
        page: 0,
        question: None,
    };
    let keyboard = keyboard(host, key, &pending);
    bot.newchat.pending.insert(key, pending);
    let question = render::escape(&host.tr("Which project?"));
    let purpose = Purpose::new(Module::NewChat, format!("ask:{key}"));
    send(bot, host, thread, &question, Some(&keyboard), purpose);
}

/// A project's button, or «More…» (the key names the pending chat — [`crate::keys`]).
pub fn on_press(bot: &mut Bot, host: &mut dyn Host, query: &CallbackQuery, key: u64, choice: &str) {
    bot.answer_press(host, query, None);
    let Some(chat) = bot.chat() else {
        return;
    };
    let pressed = query.message.as_ref().map(|message| message.message_id);
    let Some(pending) = bot.newchat.pending.get_mut(&key) else {
        return;
    };
    if pending.question.is_some() && pending.question != pressed {
        return;
    }
    if choice == "more" {
        pending.page = (pending.page + 1) % pending.projects.len().div_ceil(PAGE).max(1);
        let Some(question) = pending.question else {
            return;
        };
        let thread = pending.thread;
        let keyboard = keyboard(host, key, pending);
        let call = telegram::edit_keyboard(chat, question, Some(&keyboard));
        bot.call(
            host,
            call,
            Lane::Topic(thread),
            Purpose::new(Module::NewChat, "edit"),
        );
        return;
    }
    let Some(project) = choice
        .strip_prefix('p')
        .and_then(|index| index.parse::<usize>().ok())
        .and_then(|index| pending.projects.get(index))
        .cloned()
    else {
        return;
    };
    let chat = Chat {
        thread: pending.thread,
        project,
        words: pending.words.clone(),
        images: pending.images.clone(),
        question: pending.question,
    };
    // «From a task» first, when a tracker gives tasks: the chat is the task step's now.
    if crate::task::offer(bot, host, key, &chat) {
        bot.newchat.pending.remove(&key);
        return;
    }
    let text = chat.text();
    let spec = NewSession {
        project: chat.project.path.clone(),
        prompt: (!text.is_empty()).then_some(text),
        images: chat.images.clone(),
        ..Default::default()
    };
    match open(bot, host, &chat, &spec) {
        Ok(_) => {
            bot.newchat.pending.remove(&key);
        }
        Err(err) => {
            // The buttons stay: the user may try again, or another project.
            let note = trf(host, "Couldn't start the session: {0}", &[&err]);
            send(
                bot,
                host,
                chat.thread,
                &render::escape(&note),
                None,
                Purpose::none(),
            );
        }
    }
}

/// Opens the chat's session and binds its topic to it ([`bind`]): the session's id. «Thinking…»
/// when it has something to answer. Pictures without words go as the session's first message:
/// Flux sends a new session's pictures only with its prompt.
pub fn open(
    bot: &mut Bot,
    host: &mut dyn Host,
    chat: &Chat,
    spec: &NewSession,
) -> Result<String, String> {
    let pictures_alone = spec.prompt.is_none() && !spec.images.is_empty();
    let session = if pictures_alone {
        host.new_session(&NewSession {
            images: Vec::new(),
            ..spec.clone()
        })?
    } else {
        host.new_session(spec)?
    };
    bind(bot, host, chat, &session, spec.mode, spec.title.as_deref());
    if pictures_alone {
        conversation::send(bot, host, chat.thread, &session, "", &spec.images);
    } else if spec.prompt.is_some() {
        conversation::expect_answer(bot, host, &session);
    }
    Ok(session)
}

/// The chat's topic is the session's now: bound to it, renamed when the session has a name, the
/// chat's question edited into the first line «📁 project · mode» (`mode` — the mode it starts in,
/// until Flux tells).
pub fn bind(
    bot: &mut Bot,
    host: &mut dyn Host,
    chat: &Chat,
    session: &str,
    mode: Option<Mode>,
    title: Option<&str>,
) {
    let Some(chat_id) = bot.chat() else {
        return;
    };
    let thread = chat.thread;
    let title = title.map(telegram::topic_name).unwrap_or_default();
    bot.store.add_topic(Topic {
        thread,
        session: session.to_string(),
        project: chat.project.path.clone(),
        title: title.clone(),
        head: chat.question,
    });
    bot.save(host);
    if !title.is_empty() {
        let call = telegram::rename_topic(chat_id, thread, &title);
        bot.call(
            host,
            call,
            Lane::Topic(thread),
            Purpose::new(Module::NewChat, format!("rename:{thread}")),
        );
    }
    let mode = bot
        .sessions
        .get(session)
        .map(|session| session.mode)
        .or(mode)
        .unwrap_or(Mode::Ask);
    let head = head_html(host, &chat.project.name, mode);
    match chat.question {
        Some(question) => {
            let call = telegram::edit_text(chat_id, question, &head, None);
            bot.call(
                host,
                call,
                Lane::Topic(thread),
                Purpose::new(Module::NewChat, "edit"),
            );
            head_shows(bot, session, mode);
        }
        None => send(bot, host, thread, &head, None, Purpose::none()),
    }
}

/// The topic's first line shows this mode now.
pub fn head_shows(bot: &mut Bot, session: &str, mode: Mode) {
    bot.newchat.shown.insert(session.to_string(), mode);
}

/// The topic's first line was sent (by «Продолжить в Telegram»): it follows the mode from now on.
pub fn head_sent(bot: &mut Bot, host: &mut dyn Host, thread: i64, message: i64) {
    if let Some(topic) = bot
        .store
        .topics
        .iter_mut()
        .find(|topic| topic.thread == thread)
    {
        topic.head = Some(message);
        bot.save(host);
    }
}

pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    if let Some(key) = tag
        .strip_prefix("ask:")
        .and_then(|key| key.parse::<u64>().ok())
    {
        match result.and_then(telegram::result::<Message>) {
            Ok(message) => {
                if let Some(pending) = bot.newchat.pending.get_mut(&key) {
                    pending.question = Some(message.message_id);
                }
            }
            Err(err) => {
                host.log(&format!("Couldn't ask for the project: {err:?}"));
                bot.newchat.pending.remove(&key);
            }
        }
        return;
    }
    if let Some(thread) = tag
        .strip_prefix("rename:")
        .and_then(|thread| thread.parse::<i64>().ok())
    {
        if let Err(err) = result
            && err.topic_gone()
        {
            bot.store.forget_topic(thread);
            bot.save(host);
        }
        return;
    }
    if let Err(err) = result
        && !err.not_modified()
    {
        host.log(&format!("A new chat's edit failed: {err:?}"));
    }
}

/// A session changed: a topic's session got its title — the topic is renamed; its mode changed —
/// the first line follows.
pub fn on_session(bot: &mut Bot, host: &mut dyn Host, session: &Session) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let title = telegram::topic_name(&session.title);
    if let Some(topic) = bot
        .store
        .topics
        .iter_mut()
        .find(|topic| topic.session == session.id)
        && !session.title.trim().is_empty()
        && topic.title != title
    {
        topic.title = title.clone();
        let thread = topic.thread;
        bot.save(host);
        let call = telegram::rename_topic(chat, thread, &title);
        bot.call(
            host,
            call,
            Lane::Topic(thread),
            Purpose::new(Module::NewChat, format!("rename:{thread}")),
        );
    }
    // The first line follows the mode, wherever it was changed (`/mode`, Flux).
    let Some((thread, message, project)) = bot
        .store
        .topic_by_session(&session.id)
        .and_then(|topic| Some((topic.thread, topic.head?, topic.project.clone())))
    else {
        return;
    };
    if bot.newchat.shown.get(&session.id) == Some(&session.mode) {
        return;
    }
    head_shows(bot, &session.id, session.mode);
    let html = head_html(host, &folder_name(&project), session.mode);
    let call = telegram::edit_text(chat, message, &html, None);
    bot.call(
        host,
        call,
        Lane::Topic(thread),
        Purpose::new(Module::NewChat, "edit"),
    );
}

/// «📁 <b>project</b> · Ask».
pub fn head_html(host: &dyn Host, project: &str, mode: Mode) -> String {
    format!(
        "📁 <b>{}</b> · {}",
        render::escape(project),
        render::escape(&mode_name(host, mode))
    )
}

/// A permission mode as Flux names it.
pub fn mode_name(host: &dyn Host, mode: Mode) -> String {
    host.tr(match mode {
        Mode::Ask => "Ask",
        Mode::AcceptEdits => "Accept edits",
        Mode::Plan => "Plan",
        Mode::Bypass => "No checks",
        Mode::Other => "Another mode",
    })
}

/// The question's buttons: a page of projects two to a row, «More…» when there are more.
fn keyboard(host: &dyn Host, key: u64, pending: &Pending) -> Keyboard {
    let start = pending.page * PAGE;
    let buttons: Vec<Button> = pending
        .projects
        .iter()
        .enumerate()
        .skip(start)
        .take(PAGE)
        .map(|(index, project)| {
            let name = render::cut(&project.name, 30);
            Button::press(
                &name,
                keys::encode(Kind::NewChat, key, &format!("p{index}")),
            )
        })
        .collect();
    let mut rows: Keyboard = buttons.chunks(2).map(<[Button]>::to_vec).collect();
    if pending.projects.len() > PAGE {
        rows.push(vec![Button::press(
            &host.tr("More…"),
            keys::encode(Kind::NewChat, key, "more"),
        )]);
    }
    rows
}

fn next_key(bot: &mut Bot, host: &dyn Host) -> u64 {
    bot.newchat.last_key = (bot.newchat.last_key + 1).max(host.now_ms() % 1_000_000_000_000);
    bot.newchat.last_key
}

fn send(
    bot: &mut Bot,
    host: &mut dyn Host,
    thread: i64,
    html: &str,
    keyboard: Option<&Keyboard>,
    purpose: Purpose,
) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let call = telegram::send_message(chat, Some(thread), html, keyboard);
    bot.call(host, call, Lane::Topic(thread), purpose);
    conversation::sent_to_topic(bot, host, thread);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::ClaudeEvent;
    use crate::testing::{
        FakeHost, Telegram, bound_bot, press, session, topic_created, user_message,
    };
    use flux_plugin_api::remote::SessionState;

    fn project(name: &str, open: bool) -> Project {
        Project {
            path: format!("/work/{name}"),
            name: name.into(),
            open,
        }
    }

    #[test]
    fn a_new_chat_asks_the_project_and_opens_the_session_in_its_topic() {
        let mut host = FakeHost::default();
        host.projects = vec![project("landing", false), project("flux", true)];
        let mut bot = bound_bot(&mut host);
        let mut tg = Telegram::default();
        bot.on_update(&mut host, topic_created(1, 77, "Fix the parser t"));
        bot.on_update(&mut host, user_message(2, Some(77), "Fix the parser"));
        bot.on_update(&mut host, user_message(3, Some(77), "and its tests"));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls.len(), 1, "one question");
        let ask = &calls[0].1;
        assert_eq!(ask["text"], "Which project?");
        assert_eq!(ask["message_thread_id"], 77);
        let row = &ask["reply_markup"]["inline_keyboard"][0];
        assert_eq!(row[0]["text"], "flux", "open ones first");
        assert_eq!(row[1]["text"], "landing");
        let data = row[1]["callback_data"].as_str().unwrap().to_string();
        assert!(data.len() <= crate::keys::MAX_BYTES);
        bot.on_update(&mut host, press(4, 101, 77, &data));
        assert_eq!(
            host.claude.last().unwrap(),
            "new /work/landing Fix the parser\n\nand its tests"
        );
        let topic = bot.store.topic_by_thread(77).unwrap();
        assert_eq!(topic.session, host.new_session_id);
        assert_eq!(topic.project, "/work/landing");
        assert_eq!(host.storage["state"]["topics"][0]["thread"], 77);
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            calls[0].0, "answerCallbackQuery",
            "the press is answered first"
        );
        assert_eq!(calls[1].0, "editMessageText");
        assert_eq!(calls[1].1["text"], "📁 <b>landing</b> · Ask");
        assert!(calls[1].1.get("reply_markup").is_none(), "the buttons go");
        assert_eq!(calls[2].0, "sendMessageDraft", "«Thinking…» at once");
        // The session's title names the topic; its mode follows in the first line.
        let id = host.new_session_id.clone();
        let mut titled = session(&id, "/work/project", SessionState::Working);
        titled.title = "Fix the parser".into();
        bot.on_claude(&mut host, ClaudeEvent::Session(titled.clone()));
        titled.mode = Mode::AcceptEdits;
        bot.on_claude(&mut host, ClaudeEvent::Session(titled));
        let calls = tg.flush(&mut bot, &mut host);
        let rename = calls
            .iter()
            .filter(|(method, _)| method == "editForumTopic")
            .collect::<Vec<_>>();
        assert_eq!(rename.len(), 1);
        assert_eq!(rename[0].1["name"], "Fix the parser");
        let head = calls
            .iter()
            .find(|(method, _)| method == "editMessageText")
            .unwrap();
        assert_eq!(head.1["text"], "📁 <b>landing</b> · Accept edits");
        // The topic's next message goes to the session.
        bot.on_update(&mut host, user_message(5, Some(77), "Thanks"));
        assert_eq!(host.claude.last().unwrap(), &format!("send {id} Thanks"));
    }

    #[test]
    fn more_pages_and_a_failure_keep_the_question() {
        let mut host = FakeHost::default();
        host.projects = (0..8)
            .map(|index| project(&format!("p{index}"), false))
            .collect();
        let mut bot = bound_bot(&mut host);
        let mut tg = Telegram::default();
        bot.on_update(&mut host, user_message(1, Some(80), "Hi"));
        let calls = tg.flush(&mut bot, &mut host);
        let rows = calls[0].1["reply_markup"]["inline_keyboard"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(rows.len(), 4, "three rows of two, and «More…»");
        assert_eq!(rows[3][0]["text"], "More…");
        let more = rows[3][0]["callback_data"].as_str().unwrap().to_string();
        bot.on_update(&mut host, press(2, 101, 80, &more));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[1].0, "editMessageReplyMarkup");
        let rows = calls[1].1["reply_markup"]["inline_keyboard"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(rows[0][0]["text"], "p6");
        let pick = rows[0][1]["callback_data"].as_str().unwrap().to_string();
        host.fail = Some("Claude isn't available in this country".into());
        bot.on_update(&mut host, press(3, 101, 80, &pick));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            calls[1].1["text"],
            "Couldn't start the session: Claude isn't available in this country"
        );
        assert!(bot.store.topic_by_thread(80).is_none());
        // Pressed again — it goes now.
        bot.on_update(&mut host, press(4, 101, 80, &pick));
        assert_eq!(host.claude.last().unwrap(), "new /work/p7 Hi");
        // A button of another message (an old question) is nothing.
        bot.on_update(&mut host, press(5, 55, 80, &pick));
        assert_eq!(
            host.claude
                .iter()
                .filter(|line| line.starts_with("new"))
                .count(),
            2
        );
    }

    #[test]
    fn no_projects_is_told() {
        let mut host = FakeHost::default();
        let mut bot = bound_bot(&mut host);
        let mut tg = Telegram::default();
        bot.on_update(&mut host, user_message(1, Some(81), "Hi"));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            calls[0].1["text"],
            "Flux knows no projects yet: open one in Flux and write again."
        );
    }
}
