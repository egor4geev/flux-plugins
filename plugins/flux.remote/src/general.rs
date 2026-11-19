//! «Общее» (General): the bot's topic that is the console. Made after the binding with a welcome
//! (what the bot does, and the hint to pin the topic — only the user can), then **the status
//! message**: one message, edited as the sessions change (lane `Latest`, at most every
//! [`LATEST_EVERY_MS`](crate::transport::LATEST_EVERY_MS)), a line per session that has a topic —
//! and while the computer is locked, per session that waits for an answer (its questions come
//! here then, [`crate::requests`]). A message the user writes here is answered with how to start
//! a session. «Общее» gone (`TOPIC_ID_INVALID`) — it is made again.
//!
//! What happens to the computer and to Claude itself is told here too:
//! - the computer goes to sleep — «💤 …» at once, ahead of the queue (Flux holds the sleep about
//!   two seconds on macOS; Windows doesn't wait); on waking the note becomes one line, «💤 slept
//!   23:10–07:45» (an edit: no sound), or, when it never went, «☀️ woke up» without a sound
//!   ([`on_sleep`]);
//! - Claude is blocked in this country — Flux's sentence and «Проверить снова», once while it
//!   lasts (the message is kept, [`crate::store::Store::region_message`]); allowed again — the same
//!   message says Claude works again ([`on_region`], checked on each connection too);
//! - the subscription's limit is reached — one line with when it resets ([`on_limits`]).

use flux_plugin_api::remote::{Mode, RegionState, Session, SessionState, UsageLimits};
use flux_plugin_api::system::LocalTime;
use serde_json::Value;

use crate::bot::Bot;
use crate::host::{Host, trf};
use crate::keys::{self, Kind};
use crate::render::escape;
use crate::telegram::{self, Button, CallbackQuery, ForumTopic, Message, TgError};
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
    /// The computer went to sleep: the note's message and when.
    sleep: Option<Sleep>,
    /// The country's block message is on its way.
    region_sending: bool,
    /// The limit's line was told (until Claude goes again).
    rejected: bool,
}

/// The note of the computer's sleep.
struct Sleep {
    /// When it went to sleep, and woke (ms since 1970).
    since: u64,
    woke: Option<u64>,
    /// The note, once Telegram took it.
    message: Option<i64>,
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

/// The bot was just bound: «Общее» greets the owner (with the status message after, and the
/// country's block if Claude is blocked).
pub fn bound(bot: &mut Bot, host: &mut dyn Host) {
    if bot.store.general.is_some() {
        welcome(bot, host);
        sessions_changed(bot, host);
        check_region(bot, host);
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
                    bot.store.region_message = None;
                    bot.general.shown = None;
                    bot.save(host);
                    welcome(bot, host);
                    sessions_changed(bot, host);
                    check_region(bot, host);
                }
                Ok(_) => {}
                Err(err) => host.log(&format!("Couldn't make «General»: {err:?}")),
            }
        }
        "sleep" => match result.and_then(telegram::result::<Message>) {
            Ok(message) => {
                let woke = bot.general.sleep.as_ref().and_then(|sleep| sleep.woke);
                if let Some(sleep) = bot.general.sleep.as_mut() {
                    sleep.message = Some(message.message_id);
                }
                if let Some(woke) = woke
                    && let Some(sleep) = bot.general.sleep.take()
                {
                    slept(bot, host, &sleep, woke);
                }
            }
            Err(err) => {
                // It never went: on waking, «☀️» says the computer is back.
                if bot
                    .general
                    .sleep
                    .take()
                    .is_some_and(|sleep| sleep.woke.is_some())
                {
                    woke(bot, host);
                }
                failed(bot, host, thread, &err);
            }
        },
        "region" => {
            bot.general.region_sending = false;
            match result.and_then(telegram::result::<Message>) {
                Ok(message) if thread == bot.store.general => {
                    bot.store.region_message = Some(message.message_id);
                    bot.save(host);
                    // Allowed again while it was on its way.
                    if let Ok(RegionState::Allowed) = host.region(false) {
                        on_region(bot, host, &RegionState::Allowed);
                    }
                }
                Ok(_) => {}
                Err(err) => failed(bot, host, thread, &err),
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
        bot.store.region_message = None;
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
    session_line(host, session, &escape(title))
}

/// A session's state after its label (HTML, escaped already): "🔵 <b>Fix</b> · flux — working".
pub fn session_line(host: &dyn Host, session: &Session, title: &str) -> String {
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

/// A press of one of «Общее»'s own buttons: «Проверить снова» under the country's block.
pub fn on_press(
    bot: &mut Bot,
    host: &mut dyn Host,
    query: &CallbackQuery,
    _key: u64,
    choice: &str,
) {
    if choice != "region" {
        bot.answer_press(host, query, None);
        return;
    }
    match host.region(true) {
        Ok(RegionState::Allowed) => {
            bot.answer_press(host, query, None);
            on_region(bot, host, &RegionState::Allowed);
        }
        Ok(RegionState::Checking) => {
            // The answer comes as an event.
            let toast = host.tr("Checking…");
            bot.answer_press(host, query, Some(&toast));
        }
        Ok(RegionState::Blocked(_)) => {
            let toast = host.tr("Nothing has changed yet");
            bot.answer_press(host, query, Some(&toast));
        }
        Err(err) => bot.answer_press(host, query, Some(&err)),
    }
}

// --- The computer's sleep, the country's block, the limit ---

/// The computer goes to sleep (`asleep`) or woke.
pub fn on_sleep(bot: &mut Bot, host: &mut dyn Host, asleep: bool) {
    let (Some(chat), Some(general)) = (bot.chat(), bot.store.general) else {
        return;
    };
    let now = host.now_ms();
    if asleep {
        bot.general.sleep = Some(Sleep {
            since: now,
            woke: None,
            message: None,
        });
        let text = format!("💤 {}", host.tr("The computer is going to sleep"));
        // Ahead of everything: the computer waits only a moment.
        bot.call(
            host,
            telegram::send_message(chat, Some(general), &escape(&text), None),
            Lane::Urgent,
            Purpose::new(Module::General, format!("sleep:{general}")),
        );
        return;
    }
    match bot.general.sleep.as_mut() {
        // The note is on its way: its answer makes it one line.
        Some(sleep) if sleep.message.is_none() => sleep.woke = Some(now),
        Some(_) => {
            if let Some(sleep) = bot.general.sleep.take() {
                slept(bot, host, &sleep, now);
            }
        }
        None => woke(bot, host),
    }
}

/// The sleep's note becomes one line: «💤 The computer slept 23:10–07:45».
fn slept(bot: &mut Bot, host: &mut dyn Host, sleep: &Sleep, woke: u64) {
    let (Some(chat), Some(message)) = (bot.chat(), sleep.message) else {
        return;
    };
    let at = |ms: u64| {
        let seconds = (ms / 1000) as i64;
        LocalTime::at(seconds, host.utc_offset(seconds)).hm()
    };
    let text = format!(
        "💤 {}",
        trf(
            host,
            "The computer slept {0}–{1}",
            &[&at(sleep.since), &at(woke)]
        )
    );
    bot.call(
        host,
        telegram::edit_text(chat, message, &escape(&text), None),
        Lane::Topic(bot.store.general.unwrap_or(0)),
        Purpose::new(Module::General, "edit"),
    );
}

/// «☀️ The computer woke up», without a sound: the sleep's note never went.
fn woke(bot: &mut Bot, host: &mut dyn Host) {
    let (Some(chat), Some(general)) = (bot.chat(), bot.store.general) else {
        return;
    };
    let text = format!("☀️ {}", host.tr("The computer woke up"));
    bot.call(
        host,
        telegram::send_message(chat, Some(general), &escape(&text), None).silent(),
        Lane::Topic(general),
        Purpose::new(Module::General, format!("note:{general}")),
    );
}

/// The country check now (on each connection): a block is told once.
pub fn check_region(bot: &mut Bot, host: &mut dyn Host) {
    match host.region(false) {
        Ok(state) => on_region(bot, host, &state),
        Err(err) => host.log(&format!("No country check: {err}")),
    }
}

/// The country check changed: blocked — Flux's sentence and «Проверить снова» (once while it
/// lasts); allowed again — the same message says so, without the button.
pub fn on_region(bot: &mut Bot, host: &mut dyn Host, state: &RegionState) {
    let (Some(chat), Some(general)) = (bot.chat(), bot.store.general) else {
        return;
    };
    match state {
        RegionState::Blocked((_, sentence)) => {
            if bot.store.region_message.is_some() || bot.general.region_sending {
                return;
            }
            bot.general.region_sending = true;
            let html = format!("⛔ {}", escape(sentence.trim()));
            let keyboard = vec![vec![Button::press(
                &host.tr("Check Again"),
                keys::encode(Kind::General, 0, "region"),
            )]];
            bot.call(
                host,
                telegram::send_message(chat, Some(general), &html, Some(&keyboard)),
                Lane::Topic(general),
                Purpose::new(Module::General, format!("region:{general}")),
            );
        }
        RegionState::Allowed => {
            let Some(message) = bot.store.region_message.take() else {
                return;
            };
            bot.save(host);
            let text = format!("✅ {}", host.tr("Claude works again."));
            bot.call(
                host,
                telegram::edit_text(chat, message, &escape(&text), None),
                Lane::Topic(general),
                Purpose::new(Module::General, format!("edit:{general}")),
            );
        }
        RegionState::Checking => {}
    }
}

/// The usage changed: kept for `/usage`; the limit reached — one line with when it resets.
pub fn on_limits(bot: &mut Bot, host: &mut dyn Host, limits: UsageLimits) {
    let rejected = limits.rejected;
    let resets = [&limits.five_hour, &limits.seven_day]
        .into_iter()
        .flatten()
        .max_by_key(|window| window.percent)
        .map(|window| window.resets_at);
    bot.limits = Some(limits);
    if !rejected || bot.general.rejected {
        bot.general.rejected = rejected;
        return;
    }
    bot.general.rejected = true;
    let now = (host.now_ms() / 1000) as i64;
    let text = match resets {
        Some(at) => format!(
            "⛔ {}",
            trf(
                host,
                "Claude's limit is reached — {0}",
                &[&crate::commands::resets(host, at, now)]
            )
        ),
        None => format!("⛔ {}", host.tr("Claude's limit is reached.")),
    };
    note(bot, host, &text);
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
                head: None,
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

    /// The calls but the status message's.
    fn notes(host: &FakeHost, from: usize) -> Vec<(String, Value)> {
        host.tg_calls()
            .into_iter()
            .skip(from)
            .filter(|(_, method, params)| {
                method != "getUpdates"
                    && !params["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("in Flux"))
            })
            .map(|(_, method, params)| (method, params))
            .collect()
    }

    #[test]
    fn sleep_and_waking_are_one_line_in_general() {
        let (mut bot, mut host) = bound_bot();
        let before = host.http.len();
        let (poll, _) = host.last_call("getUpdates").unwrap();
        bot.on_claude(
            &mut host,
            crate::bot::ClaudeEvent::System(flux_plugin_api::remote::SystemEvent::WillSleep),
        );
        let calls = notes(&host, before);
        assert_eq!(calls[0].1["text"], "💤 The computer is going to sleep");
        assert_eq!(calls[0].1["message_thread_id"], 500);
        settle(&mut bot, &mut host);
        host.now += 8 * 3600 * 1000;
        bot.on_claude(
            &mut host,
            crate::bot::ClaudeEvent::System(flux_plugin_api::remote::SystemEvent::DidWake),
        );
        let calls = notes(&host, before);
        let edit = calls
            .iter()
            .find(|(method, _)| method == "editMessageText")
            .unwrap();
        // 1_795_000_000 is 11:06 UTC (the fake's zone).
        assert_eq!(edit.1["text"], "💤 The computer slept 11:06–19:06");
        assert!(host.cancelled.contains(&poll), "the poll that slept goes");
        assert_ne!(host.last_call("getUpdates").unwrap().0, poll, "a new one");
        settle(&mut bot, &mut host);
        // Woken without a note that went (it failed): «☀️», without a sound.
        bot.on_claude(
            &mut host,
            crate::bot::ClaudeEvent::System(flux_plugin_api::remote::SystemEvent::WillSleep),
        );
        let (note, _) = host.last_call("sendMessage").unwrap();
        bot.on_claude(
            &mut host,
            crate::bot::ClaudeEvent::System(flux_plugin_api::remote::SystemEvent::DidWake),
        );
        reply_error(&mut bot, &mut host, note, 400, "Bad Request: something");
        let (_, woke) = host.last_call("sendMessage").unwrap();
        assert_eq!(woke["text"], "☀️ The computer woke up");
        assert_eq!(woke["disable_notification"], true);
    }

    #[test]
    fn the_countrys_block_is_told_once_and_checked_again() {
        let mut host = FakeHost::default();
        host.secrets.insert("token".into(), "1:T".into());
        host.storage.insert(
            "state".into(),
            serde_json::json!({ "bot": 77, "owner": { "id": 5, "name": "Egor (@egor)" } }),
        );
        let blocked =
            RegionState::Blocked(("RU".into(), "Claude isn't available in Russia.".into()));
        host.region = blocked.clone();
        let mut bot = Bot::new(&host);
        bot.start(&mut host);
        settle(&mut bot, &mut host);
        let block = host
            .tg_calls()
            .into_iter()
            .find(|(_, _, params)| {
                params["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with('⛔'))
            })
            .unwrap();
        assert_eq!(block.2["text"], "⛔ Claude isn't available in Russia.");
        assert_eq!(block.2["message_thread_id"], 500);
        let data = crate::testing::button(&block.2, "Check Again");
        let message = bot.store.region_message.unwrap();
        assert_eq!(host.storage["state"]["region_message"], message);
        // The same block again: nothing new.
        let before = host.http.len();
        bot.on_claude(&mut host, crate::bot::ClaudeEvent::Region(blocked));
        assert_eq!(host.http.len(), before);
        // «Check Again»: still blocked — a toast; allowed — the message says so.
        bot.on_update(&mut host, crate::testing::press(1, message, 500, &data));
        assert_eq!(host.claude.last().unwrap(), "region true");
        assert_eq!(
            host.last_call("answerCallbackQuery").unwrap().1["text"],
            "Nothing has changed yet"
        );
        host.region = RegionState::Allowed;
        bot.on_update(&mut host, crate::testing::press(2, message, 500, &data));
        let (_, edit) = host.last_call("editMessageText").unwrap();
        assert_eq!(edit["message_id"], message);
        assert_eq!(edit["text"], "✅ Claude works again.");
        assert!(edit.get("reply_markup").is_none());
        assert_eq!(bot.store.region_message, None);
    }

    #[test]
    fn a_reached_limit_is_one_line() {
        let (mut bot, mut host) = bound_bot();
        let before = host.http.len();
        let limits = |rejected| UsageLimits {
            five_hour: Some(flux_plugin_api::remote::UsageWindow {
                percent: 100,
                resets_at: 1_795_000_000 + 1800,
            }),
            seven_day: None,
            rejected,
        };
        bot.on_claude(&mut host, crate::bot::ClaudeEvent::Limits(limits(true)));
        bot.on_claude(&mut host, crate::bot::ClaudeEvent::Limits(limits(true)));
        settle(&mut bot, &mut host);
        let calls = notes(&host, before);
        assert_eq!(calls.len(), 1, "once");
        assert_eq!(
            calls[0].1["text"],
            "⛔ Claude's limit is reached — resets at 11:36"
        );
        bot.on_claude(&mut host, crate::bot::ClaudeEvent::Limits(limits(false)));
        bot.on_claude(&mut host, crate::bot::ClaudeEvent::Limits(limits(true)));
        settle(&mut bot, &mut host);
        assert_eq!(notes(&host, before).len(), 2, "reached again");
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("/Users/me/dev/flux/"), "flux");
        assert_eq!(folder_name("C:\\dev\\api"), "api");
    }
}
