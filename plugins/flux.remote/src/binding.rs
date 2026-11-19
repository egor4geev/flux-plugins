//! The bot's token and its owner. The token is the settings' secret ([`TOKEN`]); `getMe` checks it
//! and the bot's topics; an unbound bot shows a link with a one-time code
//! (`https://t.me/<bot>?start=<code>`, its QR code in the settings' block) and the first
//! `/start <code>` from a phone binds the bot to that person — from then on it answers only them.
//! The connection's state goes to the windows and to the settings' status line ([`STATUS`]).
//!
//! - The code lives [`CODE_MINUTES`]; then a new one takes its place (the block shows it), and
//!   «New Link» makes one at once. An old or wrong code is told once, calmly, in the topic it came
//!   in; after the binding nobody else hears a word.
//! - The link's «Start» comes in a topic of its own (Telegram names it «/start …»): the binding
//!   deletes it and greets in «Общее» ([`crate::general`]); the owner's `/start` later — the same,
//!   with a short note.
//! - «Unbind» forgets the owner, «Общее» and the topics (they are that chat's) and makes a new code;
//!   «Check Again» asks Telegram again (topics turned on, the other program gone).

use flux_plugin_api::settings::Tone;
use serde_json::Value;

use crate::bot::{Bot, StatusLine};
use crate::host::{Host, trf};
use crate::model::{Connection, ToService, ToWindow};
use crate::render;
use crate::store::{Owner, Store};
use crate::telegram::{self, Me, Message, TgError};
use crate::transport::{Lane, Module, Purpose};
use crate::{awake, general};

/// The secret setting with the bot's token.
pub const TOKEN: &str = "token";
/// The settings' status line of the connection.
pub const STATUS: &str = "connection";
/// A binding code lives this long.
pub const CODE_MINUTES: u64 = 15;
/// `getMe` again after a failure.
const RETRY_MS: u32 = 10_000;
/// The code's letters: no look-alikes.
const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Default)]
pub struct State {
    /// The token the transport has.
    token: Option<String>,
    /// The next try of `getMe` after a failure.
    retry: Option<u64>,
    /// The code's end: a new one then.
    code_timer: Option<u64>,
    /// Who heard that their link is old (once each, while this code lives).
    told: Vec<i64>,
}

/// Reads the token and checks the bot (on the start, when the token changes, on «Check Again»).
pub fn start(bot: &mut Bot, host: &mut dyn Host) {
    let token = host.secret(TOKEN);
    bot.binding.token = token.clone();
    bot.me = None;
    for timer in [bot.binding.retry.take(), bot.binding.code_timer.take()]
        .into_iter()
        .flatten()
    {
        host.cancel_timer(timer);
    }
    bot.tg.set_token(host, token.clone());
    if token.is_none() {
        set(bot, host, Connection::NoToken);
        return;
    }
    set(bot, host, Connection::Checking);
    bot.call(
        host,
        telegram::get_me(),
        Lane::Topic(0),
        Purpose::new(Module::Binding, "me"),
    );
}

pub fn settings_changed(bot: &mut Bot, host: &mut dyn Host) {
    if host.secret(TOKEN) != bot.binding.token {
        start(bot, host);
    }
}

pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    if tag != "me" {
        // The `/start` topic's deletion, the note on an old link: nothing more to do.
        if let Err(err) = result {
            host.log(&format!(
                "Telegram refused the binding's call {tag}: {err:?}"
            ));
        }
        return;
    }
    match result.and_then(telegram::result::<Me>) {
        Ok(me) => {
            bot.store.for_bot(me.id);
            let topics = me.has_topics_enabled;
            bot.me = Some(me);
            bot.save(host);
            if topics {
                connect(bot, host);
            } else {
                let name = bot.bot_name();
                set(bot, host, Connection::NoTopics { bot: name });
            }
        }
        Err(TgError::Unauthorized) => set(bot, host, Connection::BadToken),
        Err(err) => {
            set(
                bot,
                host,
                Connection::Offline {
                    bot: String::new(),
                    error: describe(&err),
                },
            );
            bot.binding.retry = Some(host.timer(RETRY_MS));
        }
    }
}

/// The bot works: poll, and serve the owner or wait for one.
fn connect(bot: &mut Bot, host: &mut dyn Host) {
    bot.tg.start_polling(host, bot.store.offset);
    if bot.store.owner.is_none() {
        match bot.store.code.clone() {
            Some((_, until)) if until > host.now_ms() => {
                let left = until - host.now_ms();
                bot.binding.code_timer = Some(host.timer(left.min(u64::from(u32::MAX)) as u32));
            }
            _ => new_code(bot, host),
        }
    }
    let online = online(bot);
    set(bot, host, online);
    if bot.store.owner.is_some() {
        general::ensure(bot, host);
        general::sessions_changed(bot, host);
    }
}

/// What the connection is while Telegram answers.
fn online(bot: &Bot) -> Connection {
    let name = bot.bot_name();
    match (&bot.store.owner, &bot.store.code) {
        (Some(owner), _) => Connection::Ready {
            bot: name,
            owner: owner.name.clone(),
        },
        (None, Some((code, _))) => Connection::Unbound {
            bot: name,
            link: link(bot, code),
        },
        (None, None) => Connection::Unbound {
            bot: name,
            link: String::new(),
        },
    }
}

/// The connection changed: the windows, the status line and the computer's sleep hear of it.
fn set(bot: &mut Bot, host: &mut dyn Host, connection: Connection) {
    bot.set_connection(host, connection);
    awake::update(bot, host);
}

/// A new one-time code; the old one ends.
fn new_code(bot: &mut Bot, host: &mut dyn Host) {
    let code: String = host
        .random(12)
        .iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect();
    bot.store.code = Some((code, host.now_ms() + CODE_MINUTES * 60_000));
    bot.binding.told.clear();
    if let Some(timer) = bot.binding.code_timer.take() {
        host.cancel_timer(timer);
    }
    bot.binding.code_timer = Some(host.timer((CODE_MINUTES * 60_000) as u32));
    bot.save(host);
}

/// The binding link of a code.
pub fn link(bot: &Bot, code: &str) -> String {
    let name = bot
        .me
        .as_ref()
        .map(|me| me.username.as_str())
        .unwrap_or_default();
    format!("https://t.me/{name}?start={code}")
}

/// `/start <code>` and the topic it opens: binds the bot to who sent it when the code is the one
/// waiting. True — the message was the binding's (nobody else reads it).
pub fn on_message(bot: &mut Bot, host: &mut dyn Host, message: &Message) -> bool {
    // The topic the link's «Start» opens: the binding's, deleted with its `/start`.
    if let Some(created) = &message.forum_topic_created {
        return start_command(&created.name).is_some();
    }
    let Some(code) = start_command(message.words()) else {
        return false;
    };
    let Some(from) = message.from.clone() else {
        return true;
    };
    if let Some(owner) = bot.store.owner.clone() {
        // The owner's `/start` again: nothing to bind. Anyone else: silence.
        if from.id == owner.id && message.chat.id == owner.id {
            delete_start_topic(bot, host, owner.id, message);
            let note =
                host.tr("The bot is already connected: to start a session, write in «New chat».");
            general::note(bot, host, &note);
        }
        return true;
    }
    let fits = bot
        .store
        .code
        .as_ref()
        .is_some_and(|(expected, until)| code == expected && host.now_ms() <= *until);
    if !fits || from.is_bot || message.chat.id != from.id {
        old_link(bot, host, from.id, message.thread());
        return true;
    }
    bot.store.owner = Some(Owner {
        id: from.id,
        name: from.display(),
    });
    bot.store.code = None;
    bot.binding.told.clear();
    if let Some(timer) = bot.binding.code_timer.take() {
        host.cancel_timer(timer);
    }
    bot.save(host);
    let online = online(bot);
    set(bot, host, online);
    delete_start_topic(bot, host, from.id, message);
    general::bound(bot, host);
    true
}

/// The code of `/start <code>` (empty without one); none — not `/start`.
fn start_command(text: &str) -> Option<&str> {
    let rest = text.trim().strip_prefix("/start")?;
    // "/start@flux_bot code" — the command named with the bot.
    let rest = match rest.strip_prefix('@') {
        Some(named) => named
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest),
        None if rest.is_empty() || rest.starts_with(char::is_whitespace) => rest,
        None => return None,
    };
    Some(rest.trim())
}

/// The topic `/start` came in goes: «Общее» speaks instead.
fn delete_start_topic(bot: &mut Bot, host: &mut dyn Host, chat: i64, message: &Message) {
    let Some(thread) = message.thread() else {
        return;
    };
    if Some(thread) == bot.store.general || bot.store.topic_by_thread(thread).is_some() {
        return;
    }
    bot.call(
        host,
        telegram::delete_topic(chat, thread),
        Lane::Topic(thread),
        Purpose::new(Module::Binding, "start-topic"),
    );
}

/// An old or wrong link: told once, in the topic it came in.
fn old_link(bot: &mut Bot, host: &mut dyn Host, chat: i64, thread: Option<i64>) {
    if bot.binding.told.contains(&chat) {
        return;
    }
    bot.binding.told.push(chat);
    let text = host.tr("The link is old — open a new one in Flux's settings.");
    bot.call(
        host,
        telegram::send_message(chat, thread, &render::escape(&text), None),
        Lane::Topic(thread.unwrap_or(0)),
        Purpose::new(Module::Binding, "old-link"),
    );
}

/// A poll came back (none) or failed.
pub fn on_polled(bot: &mut Bot, host: &mut dyn Host, error: Option<TgError>) {
    let name = bot.bot_name();
    match error {
        None => {
            if matches!(bot.connection, Connection::Offline { .. }) {
                let online = online(bot);
                set(bot, host, online);
            }
        }
        Some(TgError::Unauthorized) => set(bot, host, Connection::BadToken),
        Some(TgError::Conflict) => set(bot, host, Connection::Conflict { bot: name }),
        Some(err) => set(
            bot,
            host,
            Connection::Offline {
                bot: name,
                error: describe(&err),
            },
        ),
    }
}

pub fn on_window(bot: &mut Bot, host: &mut dyn Host, _window: Option<u64>, message: &ToService) {
    match message {
        ToService::Hello => host.broadcast(&ToWindow::Connection(bot.connection.clone())),
        ToService::NewCode => {
            if bot.me.is_some() && bot.store.owner.is_none() {
                new_code(bot, host);
                let online = online(bot);
                set(bot, host, online);
            }
        }
        ToService::Unbind => unbind(bot, host),
        ToService::CheckAgain => start(bot, host),
    }
}

/// Forgets the owner, «Общее» and the topics (they are that chat's); a new code.
fn unbind(bot: &mut Bot, host: &mut dyn Host) {
    bot.store = Store {
        bot: bot.store.bot,
        offset: bot.store.offset,
        last_draft: bot.store.last_draft,
        ..Default::default()
    };
    general::forget(bot);
    // The old chat's new chats, drafts and buttons go with it (their timers then find nothing).
    bot.newchat = Default::default();
    bot.conversation = Default::default();
    bot.requests = Default::default();
    if bot.me.is_some() {
        new_code(bot, host);
    } else {
        bot.save(host);
    }
    let connection = match bot.connection {
        Connection::Ready { .. } => online(bot),
        ref other => other.clone(),
    };
    set(bot, host, connection);
}

pub fn on_timer(bot: &mut Bot, host: &mut dyn Host, timer: u64) -> bool {
    if bot.binding.code_timer == Some(timer) {
        bot.binding.code_timer = None;
        if bot.me.is_some() && bot.store.owner.is_none() {
            new_code(bot, host);
            let online = online(bot);
            set(bot, host, online);
        }
        return true;
    }
    if bot.binding.retry != Some(timer) {
        return false;
    }
    bot.binding.retry = None;
    bot.call(
        host,
        telegram::get_me(),
        Lane::Topic(0),
        Purpose::new(Module::Binding, "me"),
    );
    true
}

/// The settings' status line of a connection: what the «Binding» block says, in one line.
pub fn status_line(host: &dyn Host, connection: &Connection) -> Option<StatusLine> {
    Some(match connection {
        Connection::NoToken => return None,
        Connection::Checking => (host.tr("Checking the bot…"), Tone::Normal, true),
        Connection::BadToken => (
            host.tr("The token doesn't fit — check it in @BotFather"),
            Tone::Error,
            false,
        ),
        Connection::NoTopics { .. } => (
            host.tr("The bot's topics are off — turn them on in @BotFather"),
            Tone::Warning,
            false,
        ),
        Connection::Unbound { .. } => (
            host.tr("Open the link on your phone or point its camera at the code"),
            Tone::Normal,
            false,
        ),
        Connection::Ready { owner, .. } => {
            (trf(host, "Bound to {0}", &[owner]), Tone::Success, false)
        }
        Connection::Conflict { .. } => (
            host.tr("Another program or another Flux polls this bot"),
            Tone::Error,
            false,
        ),
        Connection::Offline { error, .. } => (
            trf(
                host,
                "No connection to Telegram: {0} — trying again",
                &[error],
            ),
            Tone::Warning,
            false,
        ),
    })
}

fn describe(error: &TgError) -> String {
    match error {
        TgError::Network(text) | TgError::Refused(_, text) => text.clone(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeHost, reply, settle};
    use crate::transport::LATEST_EVERY_MS;
    use serde_json::json;

    fn me(topics: bool) -> Value {
        json!({ "id": 77, "is_bot": true, "first_name": "Flux", "username": "flux_bot", "has_topics_enabled": topics })
    }

    fn user(id: i64, name: &str) -> Value {
        json!({ "id": id, "is_bot": false, "first_name": name, "username": name.to_lowercase() })
    }

    /// `/start <code>` from a user, in the topic the link's «Start» opened.
    fn start_update(from: Value, thread: i64, text: &str) -> telegram::Update {
        let chat = from["id"].clone();
        telegram::Update {
            update_id: 1,
            message: Some(
                serde_json::from_value(json!({
                    "message_id": 3, "message_thread_id": thread, "from": from,
                    "chat": { "id": chat, "type": "private" }, "text": text
                }))
                .unwrap(),
            ),
            ..Default::default()
        }
    }

    /// A bot checked by `getMe`, waiting for its owner: (bot, host, code).
    fn unbound() -> (Bot, FakeHost, String) {
        let mut host = FakeHost::default();
        host.secrets.insert(TOKEN.into(), "1:T".into());
        let mut bot = Bot::new(&host);
        bot.start(&mut host);
        assert_eq!(bot.connection, Connection::Checking);
        let (id, _) = host.last_call("getMe").unwrap();
        reply(&mut bot, &mut host, id, me(true));
        let Connection::Unbound { link, .. } = bot.connection.clone() else {
            panic!("{:?}", bot.connection);
        };
        let code = link.rsplit('=').next().unwrap().to_string();
        (bot, host, code)
    }

    /// The calls of a method so far: their parameters.
    fn calls(host: &FakeHost, method: &str) -> Vec<Value> {
        host.tg_calls()
            .into_iter()
            .filter(|(_, name, _)| name == method)
            .map(|(_, _, params)| params)
            .collect()
    }

    #[test]
    fn a_start_with_the_code_binds_the_owner_and_greets_in_general() {
        let (mut bot, mut host, code) = unbound();
        assert!(
            host.last_call("getUpdates").is_some(),
            "polls while it waits"
        );
        assert_eq!(
            host.statuses[STATUS],
            "Open the link on your phone or point its camera at the code"
        );
        bot.on_update(
            &mut host,
            start_update(user(5, "Egor"), 900, &format!("/start {code}")),
        );
        assert_eq!(bot.store.owner.as_ref().unwrap().id, 5);
        assert!(matches!(bot.connection, Connection::Ready { .. }));
        assert_eq!(host.storage["state"]["owner"]["id"], 5);
        assert_eq!(host.statuses[STATUS], "Bound to Egor (@egor)");
        settle(&mut bot, &mut host);
        assert_eq!(
            calls(&host, "deleteForumTopic"),
            [json!({ "chat_id": 5, "message_thread_id": 900 })]
        );
        assert_eq!(bot.store.general, Some(500));
        let texts: Vec<String> = calls(&host, "sendMessage")
            .iter()
            .map(|params| params["text"].as_str().unwrap().to_string())
            .collect();
        assert!(
            texts[0].contains("only you"),
            "the welcome first: {texts:?}"
        );
        assert!(
            texts[1].contains("Nothing is going on in Flux now"),
            "then the status: {texts:?}"
        );
        assert!(
            calls(&host, "sendMessage")
                .iter()
                .all(|params| params["message_thread_id"] == 500)
        );
        assert!(bot.store.status_message.is_some());
    }

    #[test]
    fn a_wrong_or_old_code_is_told_once_and_strangers_hear_nothing_after() {
        let (mut bot, mut host, code) = unbound();
        bot.on_update(
            &mut host,
            start_update(user(6, "Stranger"), 901, "/start wrong"),
        );
        bot.on_update(
            &mut host,
            start_update(user(6, "Stranger"), 902, "/start wrong"),
        );
        assert!(bot.store.owner.is_none());
        let told = calls(&host, "sendMessage");
        assert_eq!(told.len(), 1, "once");
        assert_eq!(told[0]["message_thread_id"], 901);
        assert_eq!(
            told[0]["text"],
            "The link is old — open a new one in Flux's settings."
        );
        // The code ends: a new one, and the old one is told.
        host.now += CODE_MINUTES * 60_000 + 1;
        let (timer, ms) = *host
            .timers
            .iter()
            .find(|(_, ms)| u64::from(*ms) == CODE_MINUTES * 60_000)
            .unwrap();
        assert_eq!(u64::from(ms), CODE_MINUTES * 60_000);
        host.timers.retain(|(id, _)| *id != timer);
        bot.on_timer(&mut host, timer);
        let Connection::Unbound { link, .. } = bot.connection.clone() else {
            panic!("{:?}", bot.connection);
        };
        assert!(!link.ends_with(&code), "a new code");
        settle(&mut bot, &mut host);
        bot.on_update(
            &mut host,
            start_update(user(5, "Egor"), 903, &format!("/start {code}")),
        );
        assert!(bot.store.owner.is_none(), "the old code doesn't bind");
        assert_eq!(calls(&host, "sendMessage").len(), 2);
        let new = link.rsplit('=').next().unwrap().to_string();
        bot.on_update(
            &mut host,
            start_update(user(5, "Egor"), 904, &format!("/start {new}")),
        );
        assert_eq!(bot.store.owner.as_ref().unwrap().id, 5);
        settle(&mut bot, &mut host);
        let before = host.http.len();
        bot.on_update(
            &mut host,
            start_update(user(6, "Stranger"), 905, &format!("/start {new}")),
        );
        bot.on_update(&mut host, start_update(user(6, "Stranger"), 905, "hello"));
        assert_eq!(host.http.len(), before, "silence");
    }

    #[test]
    fn the_owners_start_again_deletes_its_topic_with_a_note_in_general() {
        let (mut bot, mut host, code) = unbound();
        bot.on_update(
            &mut host,
            start_update(user(5, "Egor"), 900, &format!("/start {code}")),
        );
        settle(&mut bot, &mut host);
        bot.on_update(&mut host, start_update(user(5, "Egor"), 910, "/start"));
        settle(&mut bot, &mut host);
        let deleted = calls(&host, "deleteForumTopic");
        assert_eq!(deleted.last().unwrap()["message_thread_id"], 910);
        let last = calls(&host, "sendMessage").pop().unwrap();
        assert_eq!(last["message_thread_id"], 500);
        assert!(last["text"].as_str().unwrap().contains("already connected"));
        // The topic «/start» opened is the binding's: the new chats never see it.
        let created = telegram::Update {
            update_id: 2,
            message: Some(
                serde_json::from_value(json!({
                    "message_id": 9, "message_thread_id": 911, "from": user(5, "Egor"),
                    "chat": { "id": 5, "type": "private" },
                    "forum_topic_created": { "name": "/start", "is_name_implicit": true }
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        assert!(on_message(
            &mut bot,
            &mut host,
            created.message.as_ref().unwrap()
        ));
    }

    #[test]
    fn unbinding_forgets_the_chat_and_makes_a_new_link() {
        let (mut bot, mut host, code) = unbound();
        bot.on_update(
            &mut host,
            start_update(user(5, "Egor"), 900, &format!("/start {code}")),
        );
        settle(&mut bot, &mut host);
        bot.store.add_topic(crate::store::Topic {
            thread: 700,
            session: "s1".into(),
            ..Default::default()
        });
        bot.on_window(&mut host, Some(1), ToService::Unbind);
        assert!(
            bot.store.owner.is_none() && bot.store.general.is_none() && bot.store.topics.is_empty()
        );
        assert!(bot.store.status_message.is_none());
        let Connection::Unbound { link, .. } = bot.connection.clone() else {
            panic!("{:?}", bot.connection);
        };
        assert!(!link.ends_with(&code));
        assert!(matches!(
            host.windows.last(),
            Some(ToWindow::Connection(Connection::Unbound { .. }))
        ));
        assert!(host.storage["state"]["owner"].is_null());
        // Sessions changing meanwhile don't touch the old chat.
        let before = host.http.len();
        host.now += LATEST_EVERY_MS;
        general::sessions_changed(&mut bot, &mut host);
        assert_eq!(host.http.len(), before);
    }

    #[test]
    fn no_topics_a_bad_token_and_a_conflict_are_told_and_checked_again() {
        let mut host = FakeHost::default();
        host.secrets.insert(TOKEN.into(), "1:T".into());
        let mut bot = Bot::new(&host);
        bot.start(&mut host);
        let (id, _) = host.last_call("getMe").unwrap();
        reply(&mut bot, &mut host, id, me(false));
        assert!(matches!(bot.connection, Connection::NoTopics { .. }));
        assert_eq!(
            host.statuses[STATUS],
            "The bot's topics are off — turn them on in @BotFather"
        );
        bot.on_window(&mut host, Some(1), ToService::CheckAgain);
        let (id, _) = host.last_call("getMe").unwrap();
        reply(&mut bot, &mut host, id, me(true));
        assert!(matches!(bot.connection, Connection::Unbound { .. }));
        let (poll, _) = host.last_call("getUpdates").unwrap();
        crate::testing::reply_error(
            &mut bot,
            &mut host,
            poll,
            409,
            "Conflict: terminated by other getUpdates request",
        );
        assert!(matches!(bot.connection, Connection::Conflict { .. }));
        assert_eq!(
            host.statuses[STATUS],
            "Another program or another Flux polls this bot"
        );
        host.secrets.insert(TOKEN.into(), "2:X".into());
        bot.settings_changed(&mut host);
        let (id, _) = host.last_call("getMe").unwrap();
        crate::testing::reply_error(&mut bot, &mut host, id, 401, "Unauthorized");
        assert_eq!(bot.connection, Connection::BadToken);
        assert_eq!(
            host.statuses[STATUS],
            "The token doesn't fit — check it in @BotFather"
        );
        host.secrets.remove(TOKEN);
        bot.settings_changed(&mut host);
        assert_eq!(bot.connection, Connection::NoToken);
        assert!(!host.statuses.contains_key(STATUS));
    }

    #[test]
    fn offline_is_told_and_getme_tried_again() {
        let mut host = FakeHost::default();
        host.secrets.insert(TOKEN.into(), "1:T".into());
        let mut bot = Bot::new(&host);
        bot.start(&mut host);
        // The transport tries twice more, then the error comes back.
        for _ in 0..crate::transport::RETRY_MS.len() {
            let (id, _) = host.last_call("getMe").unwrap();
            bot.on_http_done(&mut host, id, Some("no route to host".into()));
            let (timer, ms) = host.timers.pop().unwrap();
            host.now += u64::from(ms);
            bot.on_timer(&mut host, timer);
        }
        let (id, _) = host.last_call("getMe").unwrap();
        bot.on_http_done(&mut host, id, Some("no route to host".into()));
        assert_eq!(
            bot.connection,
            Connection::Offline {
                bot: String::new(),
                error: "no route to host".into()
            }
        );
        assert_eq!(
            host.statuses[STATUS],
            "No connection to Telegram: no route to host — trying again"
        );
        let (timer, ms) = host.timers.pop().unwrap();
        assert_eq!(ms, RETRY_MS);
        bot.on_timer(&mut host, timer);
        let (id, _) = host.last_call("getMe").unwrap();
        reply(&mut bot, &mut host, id, me(true));
        assert!(matches!(bot.connection, Connection::Unbound { .. }));
    }

    #[test]
    fn start_commands() {
        assert_eq!(start_command("/start abc"), Some("abc"));
        assert_eq!(start_command("/start@flux_bot abc"), Some("abc"));
        assert_eq!(start_command("/start"), Some(""));
        assert_eq!(start_command("/started"), None);
        assert_eq!(start_command("hello"), None);
    }
}
