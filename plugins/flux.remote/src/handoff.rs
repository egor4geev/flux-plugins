//! «Continue in Telegram»: the Claude console's action at a session ([`ACTION`]) gives it a topic
//! named by its title (or its project's folder), whose first message is «📁 project · mode», then
//! the last answer, folded, and «write here — it goes to the session»; the session's waiting
//! request follows into the topic ([`crate::requests::repost`]). «Продолжить здесь» under a
//! request in «Общее» does the same ([`key_for`], [`on_press`]). A session that has a topic gets
//! no second one: the topic gets a note instead. The bot not bound yet — a Flux notification says
//! where to connect it.

use serde_json::Value;

use crate::bot::Bot;
use crate::general::{folder_name, title_of};
use crate::host::Host;
use crate::render::{RICH_LIMIT, cut, escape};
use crate::store::Topic;
use crate::telegram::{self, CallbackQuery, ForumTopic, Message, TgError};
use crate::transport::{Lane, Module, Purpose};
use crate::{awake, general, newchat, requests};

/// The manifest's `[[claude-actions]]` id.
pub const ACTION: &str = "continue";
/// The conversation's messages read for the last answer.
const TRANSCRIPT: u32 = 6;

#[derive(Default)]
pub struct State {
    /// The sessions whose topic is being made.
    making: Vec<String>,
}

/// The console's action: a topic for the session.
pub fn continue_in_telegram(bot: &mut Bot, host: &mut dyn Host, session: &str) {
    let Some(chat) = bot.chat() else {
        let text = host.tr("Connect the bot first: Settings → Flux Remote");
        host.notify(&text);
        return;
    };
    if let Some(topic) = bot.store.topic_by_session(session) {
        let thread = topic.thread;
        let text = host.tr("Continuing here");
        bot.call(
            host,
            telegram::send_message(chat, Some(thread), &escape(&text), None),
            Lane::Topic(thread),
            Purpose::new(Module::Handoff, format!("note:{thread}:{session}")),
        );
        return;
    }
    if bot.handoff.making.iter().any(|making| making == session) {
        return;
    }
    if !bot.sessions.contains_key(session)
        && let Ok(sessions) = host.sessions()
    {
        bot.sessions.extend(
            sessions
                .into_iter()
                .map(|session| (session.id.clone(), session)),
        );
    }
    let Some(name) = bot.sessions.get(session).map(title_of) else {
        host.log(&format!("Continue in Telegram: no session {session}"));
        return;
    };
    bot.handoff.making.push(session.to_string());
    bot.call(
        host,
        telegram::create_topic(chat, &name, telegram::COLORS[0]),
        Lane::Topic(0),
        Purpose::new(Module::Handoff, format!("topic:{session}")),
    );
}

/// The topic is made: the session goes there.
fn opened(bot: &mut Bot, host: &mut dyn Host, session: &str, topic: ForumTopic) {
    let (Some(chat), Some(found)) = (bot.chat(), bot.sessions.get(session).cloned()) else {
        return;
    };
    let thread = topic.message_thread_id;
    bot.store.add_topic(Topic {
        thread,
        session: session.to_string(),
        project: found.project.clone(),
        title: topic.name,
        head: None,
    });
    bot.save(host);
    let lane = Lane::Topic(thread);
    let first = newchat::head_html(host, &folder_name(&found.project), found.mode);
    newchat::head_shows(bot, session, found.mode);
    bot.call(
        host,
        telegram::send_message(chat, Some(thread), &first, None),
        lane.clone(),
        Purpose::new(Module::Handoff, format!("first:{thread}")),
    );
    if let Some(answer) = last_answer(host, session) {
        let markdown = format!(
            "{}\n\n<details>\n<summary>{}</summary>\n\n{}\n\n</details>",
            host.tr("Continuing the session from Flux."),
            host.tr("Claude's last answer"),
            cut(&answer, RICH_LIMIT - 1000)
        );
        bot.call(
            host,
            telegram::send_rich(chat, Some(thread), &markdown, None),
            lane.clone(),
            Purpose::new(Module::Handoff, format!("answer:{thread}")),
        );
    }
    let hint = host.tr("Write here — the messages go to this session. It stays in Flux too.");
    bot.call(
        host,
        telegram::send_message(chat, Some(thread), &escape(&hint), None),
        lane,
        Purpose::new(Module::Handoff, format!("hint:{thread}")),
    );
    // The request it waits on comes into the topic (with the words «Другое…» or «Доработать» in
    // «Общее» asked for).
    requests::repost(bot, host, session);
    general::sessions_changed(bot, host);
    awake::update(bot, host);
}

/// Claude's last message in the session (Markdown).
fn last_answer(host: &mut dyn Host, session: &str) -> Option<String> {
    let items = host.transcript(session, TRANSCRIPT).ok()?;
    items
        .into_iter()
        .rev()
        .find(|item| !item.user && !item.text.trim().is_empty())
        .map(|item| item.text)
}

/// The key of a session's «Продолжить здесь» button ([`crate::keys::Kind::Handoff`]): the
/// requests in «Общее» and `/sessions` put it on their buttons, [`on_press`] finds the session by
/// it ([`session_by_key`]). The same session has the same key, across restarts too (a hash of its
/// id).
pub fn key_for(session: &str) -> u64 {
    key_of(session)
}

/// The session a button's key names: one Flux has, or one with a topic.
pub fn session_by_key(bot: &Bot, key: u64) -> Option<String> {
    bot.sessions
        .keys()
        .chain(bot.store.topics.iter().map(|topic| &topic.session))
        .find(|session| key_of(session) == key)
        .cloned()
}

/// FNV-1a of the session's id, 48 bits: short in the button's data.
fn key_of(session: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in session.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash & 0xFFFF_FFFF_FFFF
}

/// «Продолжить здесь» (the key names the session — [`key_for`]).
pub fn on_press(
    bot: &mut Bot,
    host: &mut dyn Host,
    query: &CallbackQuery,
    key: u64,
    _choice: &str,
) {
    match session_by_key(bot, key) {
        Some(session) => {
            bot.answer_press(host, query, None);
            continue_in_telegram(bot, host, &session);
        }
        None => {
            let text = host.tr("This session is closed in Flux");
            bot.answer_press(host, query, Some(&text));
        }
    }
}

pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    let mut parts = tag.splitn(3, ':');
    let (what, first, rest) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    match what {
        "topic" => {
            bot.handoff.making.retain(|making| making != first);
            match result.and_then(telegram::result::<ForumTopic>) {
                Ok(topic) => opened(bot, host, first, topic),
                Err(err) => host.log(&format!("Couldn't make the session's topic: {err:?}")),
            }
        }
        // The topic's first message: it follows the session's mode.
        "first" => match (
            result.and_then(telegram::result::<Message>),
            first.parse::<i64>(),
        ) {
            (Ok(message), Ok(thread)) => newchat::head_sent(bot, host, thread, message.message_id),
            (Err(err), _) => host.log(&format!("Telegram refused the handoff's first: {err:?}")),
            _ => {}
        },
        // The note in a topic that is gone: the session gets a new one.
        "note" => {
            if let Err(err) = result
                && err.topic_gone()
                && let Ok(thread) = first.parse::<i64>()
            {
                bot.store.forget_topic(thread);
                bot.save(host);
                continue_in_telegram(bot, host, rest);
            }
        }
        _ => {
            if let Err(err) = result {
                host.log(&format!("Telegram refused the handoff's {what}: {err:?}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{self, Kind};
    use crate::testing::{FakeHost, reply, reply_error, session, settle};
    use flux_plugin_api::remote::{SessionState, TranscriptItem};
    use serde_json::json;

    fn bound_bot() -> (Bot, FakeHost) {
        let mut host = FakeHost::default();
        host.secrets.insert("token".into(), "1:T".into());
        host.storage.insert(
            "state".into(),
            json!({ "bot": 77, "owner": { "id": 5, "name": "Egor (@egor)" } }),
        );
        let mut bot = Bot::new(&host);
        bot.start(&mut host);
        settle(&mut bot, &mut host);
        (bot, host)
    }

    fn calls(host: &FakeHost, method: &str) -> Vec<Value> {
        host.tg_calls()
            .into_iter()
            .filter(|(_, name, _)| name == method)
            .map(|(_, _, params)| params)
            .collect()
    }

    #[test]
    fn not_bound_is_a_notification() {
        let mut host = FakeHost::default();
        let mut bot = Bot::new(&host);
        continue_in_telegram(&mut bot, &mut host, "s1");
        assert_eq!(
            host.notices,
            ["Connect the bot first: Settings → Flux Remote"]
        );
        assert!(host.http.is_empty());
    }

    #[test]
    fn a_session_gets_a_topic_with_its_last_answer() {
        let (mut bot, mut host) = bound_bot();
        let mut found = session("s1", "/Users/me/dev/api", SessionState::Idle);
        found.mode = flux_plugin_api::remote::Mode::AcceptEdits;
        bot.sessions.insert("s1".into(), found);
        host.transcript = vec![
            TranscriptItem {
                user: true,
                text: "Migrate the DB".into(),
            },
            TranscriptItem {
                user: false,
                text: "Migrations **0042–0044** are done.".into(),
            },
        ];
        bot.on_claude(
            &mut host,
            crate::bot::ClaudeEvent::Action(ACTION.into(), "s1".into()),
        );
        let (_, params) = host.last_call("createForumTopic").unwrap();
        assert_eq!(params["name"], "api", "no title: the project's folder");
        settle(&mut bot, &mut host);
        let topic = bot.store.topic_by_session("s1").unwrap().clone();
        assert_eq!(
            (topic.thread, topic.project.as_str(), topic.title.as_str()),
            (501, "/Users/me/dev/api", "api")
        );
        let in_topic: Vec<String> = host
            .tg_calls()
            .into_iter()
            .filter(|(_, _, params)| params["message_thread_id"] == 501)
            .map(|(_, method, params)| match method.as_str() {
                "sendRichMessage" => format!(
                    "rich: {}",
                    params["rich_message"]["markdown"].as_str().unwrap()
                ),
                _ => params["text"].as_str().unwrap().to_string(),
            })
            .collect();
        assert_eq!(in_topic[0], "📁 <b>api</b> · Accept edits");
        assert!(in_topic[1].starts_with("rich: Continuing the session from Flux.\n\n<details>\n<summary>Claude's last answer</summary>"));
        assert!(in_topic[1].contains("Migrations **0042–0044** are done."));
        assert_eq!(
            in_topic[2],
            "Write here — the messages go to this session. It stays in Flux too."
        );
        let status = host.last_call("editMessageText").unwrap().1;
        assert!(
            status["text"].as_str().unwrap().contains("⚪ api — idle"),
            "«Общее» shows it"
        );
        // Again: a note in the same topic, no second topic.
        continue_in_telegram(&mut bot, &mut host, "s1");
        assert_eq!(
            calls(&host, "createForumTopic").len(),
            2,
            "«Общее»'s and this one"
        );
        let (_, note) = host.last_call("sendMessage").unwrap();
        assert_eq!(
            (note["message_thread_id"].clone(), note["text"].clone()),
            (json!(501), json!("Continuing here"))
        );
    }

    #[test]
    fn a_note_in_a_topic_that_is_gone_makes_a_new_topic() {
        let (mut bot, mut host) = bound_bot();
        bot.sessions
            .insert("s1".into(), session("s1", "/p/web", SessionState::Working));
        bot.store.add_topic(Topic {
            thread: 700,
            session: "s1".into(),
            project: "/p/web".into(),
            title: "web".into(),
            head: None,
        });
        continue_in_telegram(&mut bot, &mut host, "s1");
        let (id, _) = host.last_call("sendMessage").unwrap();
        reply_error(
            &mut bot,
            &mut host,
            id,
            400,
            "Bad Request: TOPIC_ID_INVALID",
        );
        assert!(bot.store.topic_by_thread(700).is_none());
        let (id, params) = host.last_call("createForumTopic").unwrap();
        assert_eq!(params["name"], "web");
        reply(
            &mut bot,
            &mut host,
            id,
            json!({ "message_thread_id": 720, "name": "web" }),
        );
        assert_eq!(bot.store.topic_by_session("s1").unwrap().thread, 720);
        assert!(
            host.claude.contains(&"awake Awake::Always".to_string()),
            "a remote session works: no sleep"
        );
    }

    #[test]
    fn continue_here_finds_the_session_by_its_key() {
        let (mut bot, mut host) = bound_bot();
        let mut found = session(
            "0b5d7c1e-2f4a-4c1b-9d3e-8a7f6e5d4c3b",
            "/p/db",
            SessionState::Waiting,
        );
        found.title = "Migrations".into();
        bot.sessions.insert(found.id.clone(), found.clone());
        let key = key_for(&found.id);
        assert_eq!(key, key_for(&found.id), "the same key each time");
        let data = keys::encode(Kind::Handoff, key, "here");
        assert!(data.len() <= 24, "{data}");
        let query = |data: String| crate::telegram::CallbackQuery {
            id: "q1".into(),
            from: crate::telegram::User {
                id: 5,
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        };
        bot.on_update(
            &mut host,
            crate::telegram::Update {
                update_id: 9,
                callback_query: Some(query(data)),
                ..Default::default()
            },
        );
        let answers = calls(&host, "answerCallbackQuery");
        assert_eq!(
            answers,
            [json!({ "callback_query_id": "q1" })],
            "answered at once"
        );
        assert_eq!(
            host.last_call("createForumTopic").unwrap().1["name"],
            "Migrations"
        );
        // A key of no session: told on the press.
        bot.on_update(
            &mut host,
            crate::telegram::Update {
                update_id: 10,
                callback_query: Some(query(keys::encode(Kind::Handoff, 12345, "here"))),
                ..Default::default()
            },
        );
        assert_eq!(
            host.last_call("answerCallbackQuery").unwrap().1["text"],
            "This session is closed in Flux"
        );
    }
}
