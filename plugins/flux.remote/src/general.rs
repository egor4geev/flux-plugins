//! «Общее» (General): the bot's topic that is the console. Made after the binding with a welcome
//! (what the bot does, and the hint to pin the topic — only the user can), then **the status
//! message**: one message, edited as the sessions change (lane `Latest`, at most every
//! [`LATEST_EVERY_MS`](crate::transport::LATEST_EVERY_MS)), a line per session that has a topic —
//! and while the computer is locked, per session that waits for an answer (its questions come
//! here then, [`crate::requests`]). A message the user writes here is answered with how to start
//! a session. «Общее» gone (`TOPIC_ID_INVALID`) — it is made again.

use flux_plugin_api::remote::{Mode, Session, SessionState};
use flux_plugin_api::system::LocalTime;
use serde_json::Value;

use crate::bot::Bot;
use crate::host::{Host, trf};
use crate::render::escape;
use crate::telegram::{self, CallbackQuery, ForumTopic, Message, TgError};
use crate::transport::{Lane, Module, Purpose};

/// The status message's lane: its newest text wins.
const STATUS_LANE: &str = "status";

#[derive(Default)]
pub struct State {
    /// `createForumTopic` is on its way.
    creating: bool,
    /// The status message's `sendMessage` is on its way.
    sending: bool,
    /// The status message's text as sent last (or on its way).
    shown: Option<String>,
}

/// «Общее» exists (made once after the binding, and again when it is gone).
pub fn ensure(bot: &mut Bot, host: &mut dyn Host) {
    if bot.store.general.is_some() || bot.general.creating {
        return;
    }
    let Some(chat) = bot.chat() else {
        return;
    };
    bot.general.creating = true;
    let name = host.tr("General");
    bot.call(
        host,
        telegram::create_topic(chat, &name, telegram::COLORS[1]),
        Lane::Topic(0),
        Purpose::new(Module::General, "created"),
    );
}

/// The bot was just bound: «Общее» greets the owner (with the status message after).
pub fn bound(bot: &mut Bot, host: &mut dyn Host) {
    if bot.store.general.is_some() {
        welcome(bot, host);
        sessions_changed(bot, host);
    } else {
        ensure(bot, host);
    }
}

/// The binding is gone: nothing of «Общее» is kept.
pub fn forget(bot: &mut Bot) {
    bot.general = State::default();
}

/// A short note in «Общее» (the owner's `/start` again).
pub fn note(bot: &mut Bot, host: &mut dyn Host, text: &str) {
    let (Some(chat), Some(general)) = (bot.chat(), bot.store.general) else {
        return;
    };
    bot.call(
        host,
        telegram::send_message(chat, Some(general), &escape(text), None),
        Lane::Topic(general),
        Purpose::new(Module::General, format!("note:{general}")),
    );
}

pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    let (what, thread) = match tag.split_once(':') {
        Some((what, thread)) => (what, thread.parse::<i64>().ok()),
        None => (tag, None),
    };
    match what {
        "created" => {
            bot.general.creating = false;
            match result.and_then(telegram::result::<ForumTopic>) {
                Ok(topic) if bot.chat().is_some() => {
                    bot.store.general = Some(topic.message_thread_id);
                    bot.store.status_message = None;
                    bot.general.shown = None;
                    bot.save(host);
                    welcome(bot, host);
                    sessions_changed(bot, host);
                }
                Ok(_) => {}
                Err(err) => host.log(&format!("Couldn't make «General»: {err:?}")),
            }
        }
        "status" => {
            bot.general.sending = false;
            match result.and_then(telegram::result::<Message>) {
                Ok(message) if thread == bot.store.general => {
                    bot.store.status_message = Some(message.message_id);
                    bot.save(host);
                    // What changed while it was on its way.
                    sessions_changed(bot, host);
                }
                Ok(_) => {}
                Err(err) => failed(bot, host, thread, &err),
            }
        }
        _ => {
            if let Err(err) = result {
                failed(bot, host, thread, &err);
            }
        }
    }
}

/// A call to «Общее» failed: the topic gone — made again; the status message gone — sent again.
fn failed(bot: &mut Bot, host: &mut dyn Host, thread: Option<i64>, err: &TgError) {
    if err.not_modified() || thread != bot.store.general {
        return;
    }
    if err.topic_gone() {
        bot.store.general = None;
        bot.store.status_message = None;
        bot.general.shown = None;
        bot.save(host);
        ensure(bot, host);
    } else if matches!(err, TgError::Refused(_, text) if text.contains("message to edit not found"))
    {
        bot.store.status_message = None;
        bot.general.shown = None;
        sessions_changed(bot, host);
    } else {
        // The next change tries again.
        bot.general.shown = None;
        host.log(&format!("«General» refused a call: {err:?}"));
    }
}

/// What the bot does, in three short lines: after the binding, or when «Общее» is made again.
fn welcome(bot: &mut Bot, host: &mut dyn Host) {
    let text = host.tr(
        "Done: the bot is connected to Flux and answers only you.\nTo start a Claude session, write in «New chat» — the bot asks for the project.\nWhile the computer is locked, the questions of every session come here. Pin this topic to the top — only you can.",
    );
    note(bot, host, &text);
}

/// A session appeared, changed or ended (or the computer was locked): the status message is due —
/// sent once, then edited when its text changes.
pub fn sessions_changed(bot: &mut Bot, host: &mut dyn Host) {
    let (Some(chat), Some(general)) = (bot.chat(), bot.store.general) else {
        return;
    };
    let text = status_text(bot, host);
    if bot.general.shown.as_deref() == Some(text.as_str()) || bot.general.sending {
        return;
    }
    bot.general.shown = Some(text.clone());
    match bot.store.status_message {
        Some(message) => bot.call(
            host,
            telegram::edit_text(chat, message, &text, None),
            Lane::Latest(STATUS_LANE.into()),
            Purpose::new(Module::General, format!("edit:{general}")),
        ),
        None => {
            bot.general.sending = true;
            bot.call(
                host,
                telegram::send_message(chat, Some(general), &text, None),
                Lane::Topic(general),
                Purpose::new(Module::General, format!("status:{general}")),
            );
        }
    }
}

/// The status message: a line per session with a topic and, while the computer is locked, per
/// session that waits for an answer.
fn status_text(bot: &Bot, host: &dyn Host) -> String {
    let mut lines = Vec::new();
    for topic in &bot.store.topics {
        if let Some(session) = bot.sessions.get(&topic.session) {
            let title = if topic.title.is_empty() {
                title_of(session)
            } else {
                topic.title.clone()
            };
            lines.push(status_line(host, session, &title));
        }
    }
    let locked = bot.system.screen_locked;
    if locked {
        for session in bot.sessions.values() {
            if waits(session) && bot.store.topic_by_session(&session.id).is_none() {
                lines.push(status_line(host, session, &title_of(session)));
            }
        }
    }
    if lines.is_empty() {
        lines.push(escape(&host.tr("Nothing is going on in Flux now")));
    } else {
        lines.insert(0, format!("<b>{}</b>", escape(&host.tr("Now in Flux"))));
    }
    if locked {
        lines.push(String::new());
        let note = host.tr("The computer is locked — the questions of every session come here.");
        lines.push(format!("<i>{}</i>", escape(&note)));
    }
    lines.join("\n")
}

/// The session waits for the user's answer.
fn waits(session: &Session) -> bool {
    session.state == SessionState::Waiting || session.request.is_some()
}

/// A session's name: its title, or its project's folder.
pub fn title_of(session: &Session) -> String {
    if !session.title.trim().is_empty() {
        return session.title.trim().to_string();
    }
    folder_name(&session.project)
}

/// "/Users/me/dev/flux" → "flux".
pub fn folder_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_string()
}

/// The session's line in the status message: "🔵 Fix the parser — working" (HTML).
pub fn status_line(host: &dyn Host, session: &Session, title: &str) -> String {
    let title = escape(title);
    if waits(session) {
        format!("🟡 {}", trf(host, "{0} — waiting for an answer", &[&title]))
    } else {
        match session.state {
            SessionState::Starting | SessionState::Working if session.mode == Mode::Plan => {
                format!("🔵 {}", trf(host, "{0} — planning", &[&title]))
            }
            SessionState::Starting | SessionState::Working | SessionState::Waiting => {
                format!("🔵 {}", trf(host, "{0} — working", &[&title]))
            }
            SessionState::Idle | SessionState::Unloaded if session.unseen => {
                let at = LocalTime::at(session.updated, host.utc_offset(session.updated)).hm();
                format!("🟢 {}", trf(host, "{0} — done at {1}", &[&title, &at]))
            }
            SessionState::Idle | SessionState::Unloaded => {
                format!("⚪ {}", trf(host, "{0} — idle", &[&title]))
            }
        }
    }
}

/// A message the user wrote in «Общее»: how to start a session.
pub fn on_message(bot: &mut Bot, host: &mut dyn Host, _message: &Message) {
    let text = host.tr("This topic shows what goes on in Flux. To start a session, write in «New chat» — the bot asks for the project.");
    note(bot, host, &text);
}

/// A press of one of «Общее»'s own buttons (none yet: the requests' are agent B's).
pub fn on_press(
    bot: &mut Bot,
    host: &mut dyn Host,
    query: &CallbackQuery,
    _key: u64,
    _choice: &str,
) {
    bot.answer_press(host, query, None);
}

/// «Общее» has no timers: the transport paces the status message.
pub fn on_timer(_bot: &mut Bot, _host: &mut dyn Host, _timer: u64) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Owner, Topic};
    use crate::testing::{FakeHost, reply, reply_error, session, settle};
    use crate::transport::LATEST_EVERY_MS;

    /// A bot bound to 5 with «Общее» (500) and its status message made.
    fn bound_bot() -> (Bot, FakeHost) {
        let mut host = FakeHost::default();
        host.secrets.insert("token".into(), "1:T".into());
        host.storage.insert(
            "state".into(),
            serde_json::json!({ "bot": 77, "owner": { "id": 5, "name": "Egor (@egor)" } }),
        );
        let mut bot = Bot::new(&host);
        bot.start(&mut host);
        settle(&mut bot, &mut host);
        assert_eq!(bot.store.general, Some(500));
        assert!(bot.store.status_message.is_some());
        (bot, host)
    }

    fn last_text(host: &FakeHost, method: &str) -> String {
        host.last_call(method).unwrap().1["text"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn the_status_has_a_line_per_session_with_a_topic() {
        let (mut bot, mut host) = bound_bot();
        assert_eq!(
            last_text(&host, "sendMessage"),
            "Nothing is going on in Flux now"
        );
        let mut working = session("s1", "/p/flux", SessionState::Working);
        working.title = "Fix <the> parser".into();
        let mut waiting = session("s2", "/p/api", SessionState::Waiting);
        waiting.mode = Mode::AcceptEdits;
        let mut done = session("s3", "/p/web", SessionState::Idle);
        done.unseen = true;
        done.updated = 1_795_000_000 - 1_795_000_000 % 86_400 + 14 * 3600 + 32 * 60;
        let idle = session("s4", "/p/db", SessionState::Unloaded);
        let free = session("s5", "/p/free", SessionState::Waiting);
        for (thread, session) in [(701, &working), (702, &waiting), (703, &done), (704, &idle)] {
            bot.store.add_topic(Topic {
                thread,
                session: session.id.clone(),
                project: session.project.clone(),
                title: String::new(),
            });
        }
        for session in [working, waiting, done, idle, free] {
            bot.sessions.insert(session.id.clone(), session);
        }
        sessions_changed(&mut bot, &mut host);
        assert_eq!(
            last_text(&host, "editMessageText"),
            "<b>Now in Flux</b>\n🔵 Fix &lt;the&gt; parser — working\n🟡 api — waiting for an answer\n🟢 web — done at 14:32\n⚪ db — idle"
        );
        // Locked: the session without a topic that waits comes too, and the note.
        bot.system.screen_locked = true;
        sessions_changed(&mut bot, &mut host);
        let (edit, _) = host.last_call("editMessageText").unwrap();
        reply(&mut bot, &mut host, edit, serde_json::json!(true));
        let (timer, ms) = host.timers.pop().unwrap();
        assert_eq!(
            u64::from(ms),
            LATEST_EVERY_MS,
            "the next edit waits its turn"
        );
        host.now += LATEST_EVERY_MS;
        bot.on_timer(&mut host, timer);
        let text = last_text(&host, "editMessageText");
        assert!(text.contains("🟡 free — waiting for an answer"), "{text}");
        assert!(text.ends_with(
            "<i>The computer is locked — the questions of every session come here.</i>"
        ));
    }

    #[test]
    fn the_same_text_isnt_sent_twice_and_a_change_while_sending_follows() {
        let mut host = FakeHost::default();
        let mut bot = Bot::new(&host);
        bot.store.owner = Some(Owner {
            id: 5,
            name: "Egor".into(),
        });
        bot.store.general = Some(500);
        bot.tg.set_token(&mut host, Some("1:T".into()));
        sessions_changed(&mut bot, &mut host);
        sessions_changed(&mut bot, &mut host);
        assert_eq!(host.http.len(), 1, "one status message");
        bot.store.add_topic(Topic {
            thread: 701,
            session: "s1".into(),
            ..Default::default()
        });
        bot.sessions
            .insert("s1".into(), session("s1", "/p/flux", SessionState::Working));
        sessions_changed(&mut bot, &mut host);
        assert_eq!(host.http.len(), 1, "waits for the message's id");
        let (id, _) = host.last_call("sendMessage").unwrap();
        reply(
            &mut bot,
            &mut host,
            id,
            serde_json::json!({ "message_id": 42, "chat": { "id": 5 } }),
        );
        assert_eq!(bot.store.status_message, Some(42));
        assert!(last_text(&host, "editMessageText").contains("flux — working"));
    }

    #[test]
    fn general_gone_is_made_again_and_a_message_there_gets_the_hint() {
        let (mut bot, mut host) = bound_bot();
        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 9, "message_thread_id": 500, "from": { "id": 5, "first_name": "Egor" },
            "chat": { "id": 5, "type": "private" }, "text": "hi"
        }))
        .unwrap();
        bot.on_update(
            &mut host,
            telegram::Update {
                update_id: 3,
                message: Some(message),
                ..Default::default()
            },
        );
        let (id, params) = host.last_call("sendMessage").unwrap();
        assert!(params["text"].as_str().unwrap().contains("«New chat»"));
        reply_error(
            &mut bot,
            &mut host,
            id,
            400,
            "Bad Request: TOPIC_ID_INVALID",
        );
        assert_eq!(bot.store.general, None);
        let (id, params) = host.last_call("createForumTopic").unwrap();
        assert_eq!(params["name"], "General");
        reply(
            &mut bot,
            &mut host,
            id,
            serde_json::json!({ "message_thread_id": 501, "name": "General" }),
        );
        settle(&mut bot, &mut host);
        assert_eq!(bot.store.general, Some(501));
        let (_, params) = host.last_call("sendMessage").unwrap();
        assert_eq!(params["message_thread_id"], 501);
        assert_eq!(
            params["text"], "Nothing is going on in Flux now",
            "the status again"
        );
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("/Users/me/dev/flux/"), "flux");
        assert_eq!(folder_name("C:\\dev\\api"), "api");
    }
}
