//! Messages that came while Flux was closed — or while the computer slept (the author: the same,
//! and always, however short the wait): a message sent before the bot last began to hear Telegram
//! ([`awake`]: the service's start, the computer's wake; its `date` against the computer's clock,
//! with [`CLOCK_SLACK_S`]). Only the words that make Claude work wait — a session's topic, a new chat; commands
//! answer about now, presses carry no date. What happens is the plugin's setting [`SETTING`]:
//!
//! - «Спрашивать» ([`ASK`], the default): the bot answers in the topic, quoting the first one —
//!   «Пришло, пока Flux был закрыт. Выполнить?» (one question for several in a topic: «Пришло 3
//!   сообщения…»); «Выполнить» routes them as if they came now, joined into one message as a new
//!   chat joins its first ones ([`Bot::route`]); «Не выполнять» — the question becomes a short
//!   result line.
//! - «Выполнять всегда» ([`ALWAYS`]): they go as they come.
//! - «Не выполнять» ([`NEVER`]): one calm note in the topic that they weren't run.
//!
//! The questions are asked once the batch of updates is read ([`flush`]): the messages of a topic
//! that came together get one. The waiting messages are kept in memory (Telegram keeps undelivered
//! updates about a day): after a restart a question's buttons say it is old.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::bot::Bot;
use crate::host::Host;
use crate::keys::{self, Kind};
use crate::render::{self, escape};
use crate::telegram::{self, Button, CallbackQuery, Message, TgError};
use crate::transport::{Lane, Module, Purpose};

/// The setting's key and its values.
pub const SETTING: &str = "late-messages";
pub const ASK: &str = "ask";
pub const ALWAYS: &str = "always";
pub const NEVER: &str = "never";
/// Telegram dates a message by its own clock, the bot reads the computer's: a message sent this
/// close before the moment counts as sent after it.
pub const CLOCK_SLACK_S: i64 = 2;

#[derive(Default)]
pub struct State {
    /// The late messages waiting for the user's word, by topic.
    held: BTreeMap<i64, Held>,
    /// The last key given (from the clock: a button of the last run means nothing).
    last_key: u64,
    /// Since when the bot hears Telegram (seconds since 1970): what was sent before came while
    /// Flux was closed or the computer slept. 0 — not started (nothing is late).
    since: i64,
}

/// A topic's late messages.
struct Held {
    key: u64,
    messages: Vec<Message>,
    /// The question, once Telegram took it.
    question: Option<i64>,
    /// The question is on its way.
    sending: bool,
    /// How many messages the question counts.
    asked: usize,
}

/// The setting's value: «Спрашивать» unless the user chose otherwise.
pub fn setting(host: &dyn Host) -> &'static str {
    match host.setting(SETTING).as_ref().and_then(Value::as_str) {
        Some(ALWAYS) => ALWAYS,
        Some(NEVER) => NEVER,
        _ => ASK,
    }
}

/// The bot hears Telegram from now on: the service started, the computer woke.
pub fn awake(bot: &mut Bot, host: &dyn Host) {
    bot.late.since = (host.now_ms() / 1000) as i64;
}

/// The message was sent before the bot heard Telegram: Flux was closed, or the computer slept.
pub fn is_late(bot: &Bot, message: &Message) -> bool {
    message.date > 0 && message.date < bot.late.since - CLOCK_SLACK_S
}

/// The user's words to a session or a new chat: a late one waits for the user's word, or is
/// noted and dropped (true); anything else goes now (false).
pub fn hold(bot: &mut Bot, host: &mut dyn Host, message: &Message) -> bool {
    let Some(thread) = message.thread() else {
        return false;
    };
    if Some(thread) == bot.store.general || !is_late(bot, message) {
        return false;
    }
    if setting(host) == ALWAYS {
        return false;
    }
    let key = match bot.late.held.get(&thread) {
        Some(held) => held.key,
        None => {
            bot.late.last_key = (bot.late.last_key + 1).max(host.now_ms() % 1_000_000_000_000);
            bot.late.last_key
        }
    };
    bot.late
        .held
        .entry(thread)
        .or_insert_with(|| Held {
            key,
            messages: Vec::new(),
            question: None,
            sending: false,
            asked: 0,
        })
        .messages
        .push(message.clone());
    true
}

/// A batch of updates is read: each topic with new late messages gets its question (or the note).
pub fn flush(bot: &mut Bot, host: &mut dyn Host) {
    let Some(chat) = bot.chat() else {
        bot.late.held.clear();
        return;
    };
    let never = setting(host) == NEVER;
    let due: Vec<i64> = bot
        .late
        .held
        .iter()
        .filter(|(_, held)| held.asked != held.messages.len() && !held.sending)
        .map(|(thread, _)| *thread)
        .collect();
    for thread in due {
        if never {
            let Some(held) = bot.late.held.remove(&thread) else {
                continue;
            };
            let note = counted(
                host,
                held.messages.len(),
                "This came while Flux was closed and wasn't run.",
                "{0} messages came while Flux was closed and weren't run.",
            );
            let call = telegram::send_message(chat, Some(thread), &escape(&note), None);
            bot.call(host, call, Lane::Topic(thread), Purpose::none());
            crate::conversation::sent_to_topic(bot, host, thread);
            continue;
        }
        let Some(held) = bot.late.held.get_mut(&thread) else {
            continue;
        };
        held.asked = held.messages.len();
        let html = escape(&question(host, held.asked));
        let keyboard = vec![vec![
            Button::press(&host.tr("Run"), keys::encode(Kind::Late, held.key, "run")).primary(),
            Button::press(
                &host.tr("Don't run"),
                keys::encode(Kind::Late, held.key, "skip"),
            ),
        ]];
        let call = match held.question {
            Some(question) => telegram::edit_text(chat, question, &html, Some(&keyboard)),
            None => {
                held.sending = true;
                let first = held.messages[0].message_id;
                telegram::send_message(chat, Some(thread), &html, Some(&keyboard)).reply_to(first)
            }
        };
        let tag = format!("ask:{thread}");
        bot.call(
            host,
            call,
            Lane::Topic(thread),
            Purpose::new(Module::Late, tag),
        );
        crate::conversation::sent_to_topic(bot, host, thread);
    }
}

/// «Пришло, пока Flux был закрыт. Выполнить?», «Пришло 3 сообщения…».
fn question(host: &dyn Host, count: usize) -> String {
    counted(
        host,
        count,
        "This came while Flux was closed. Run it?",
        "{0} messages came while Flux was closed. Run them?",
    )
}

/// One message is «it» (no number); several are counted, in the language's plural forms.
fn counted(host: &dyn Host, count: usize, one: &str, several: &str) -> String {
    if count == 1 {
        host.tr(one)
    } else {
        render::count(host, count as u64, one, several)
    }
}

/// «Выполнить» / «Не выполнять» (the key names the topic's question).
pub fn on_press(bot: &mut Bot, host: &mut dyn Host, query: &CallbackQuery, key: u64, choice: &str) {
    let thread = bot
        .late
        .held
        .iter()
        .find(|(_, held)| held.key == key)
        .map(|(thread, _)| *thread);
    let (Some(chat), Some(pressed)) = (bot.chat(), query.message.as_ref()) else {
        bot.answer_press(host, query, None);
        return;
    };
    let lane = Lane::Topic(pressed.thread().unwrap_or(0));
    let Some(held) = thread.and_then(|thread| bot.late.held.remove(&thread)) else {
        // A question of the last run: its messages are gone.
        let toast = host.tr("This question is old");
        bot.answer_press(host, query, Some(&toast));
        let call = telegram::edit_keyboard(chat, pressed.message_id, None);
        bot.call(host, call, lane, Purpose::new(Module::Late, "edit"));
        return;
    };
    bot.answer_press(host, query, None);
    let run = choice == "run";
    let result = if run {
        format!("▶ {}", host.tr("Running"))
    } else {
        format!("✕ {}", host.tr("Not run"))
    };
    let call = telegram::edit_text(chat, pressed.message_id, &escape(&result), None);
    bot.call(host, call, lane, Purpose::new(Module::Late, "edit"));
    if run {
        for message in joined(held.messages) {
            bot.route(host, &message);
        }
    }
}

/// The late messages as they go now: one message with their words, as a new chat joins its first
/// ones — or each as it came when they carry pictures or files.
fn joined(messages: Vec<Message>) -> Vec<Message> {
    if messages.len() < 2
        || messages
            .iter()
            .any(|message| message.photo.is_some() || message.document.is_some())
    {
        return messages;
    }
    let words: Vec<&str> = messages
        .iter()
        .map(|message| message.words().trim())
        .filter(|words| !words.is_empty())
        .collect();
    let mut one = messages[0].clone();
    one.text = Some(words.join("\n\n"));
    one.caption = None;
    vec![one]
}

pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    let Some(thread) = tag
        .strip_prefix("ask:")
        .and_then(|thread| thread.parse::<i64>().ok())
    else {
        if let Err(err) = result
            && !err.not_modified()
        {
            host.log(&format!("A late message's edit failed: {err:?}"));
        }
        return;
    };
    let Some(held) = bot.late.held.get_mut(&thread) else {
        return;
    };
    held.sending = false;
    match result.and_then(telegram::result::<Message>) {
        Ok(message) => {
            held.question = Some(message.message_id);
            // More came while it was on its way: the count follows.
            if held.asked != held.messages.len() {
                flush(bot, host);
            }
        }
        Err(err) => {
            host.log(&format!("Couldn't ask about the late messages: {err:?}"));
            bot.late.held.remove(&thread);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        FakeHost, NOW_S, Telegram, bot_with_topic, button, press, topic_created, user_message,
    };
    use flux_plugin_api::remote::SystemEvent;
    use serde_json::json;

    /// A batch of updates the bot reads when it starts an hour after they were written.
    fn an_hour_later(bot: &mut Bot, host: &mut FakeHost, updates: Vec<telegram::Update>) {
        host.now = (NOW_S as u64 + 3600) * 1000;
        awake(bot, host);
        for update in updates {
            bot.on_update(host, update);
        }
        flush(bot, host);
    }

    fn sends(host: &FakeHost) -> Vec<&String> {
        host.claude
            .iter()
            .filter(|line| line.starts_with("send") || line.starts_with("new"))
            .collect()
    }

    #[test]
    fn ask_holds_them_and_run_sends_them_together() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        // Fresh: it goes at once.
        bot.on_update(&mut host, user_message(1, Some(77), "Hi"));
        assert_eq!(sends(&host), ["send s1 Hi"]);
        an_hour_later(
            &mut bot,
            &mut host,
            vec![
                user_message(2, Some(77), "Fix the parser"),
                user_message(3, Some(77), "and its tests"),
                user_message(4, Some(77), "/usage"),
            ],
        );
        assert_eq!(sends(&host).len(), 1, "nothing sent");
        let calls = tg.flush(&mut bot, &mut host);
        let question = calls
            .iter()
            .find(|(_, params)| params["reply_parameters"].is_object())
            .unwrap()
            .1
            .clone();
        assert_eq!(
            question["text"],
            "2 messages came while Flux was closed. Run them?"
        );
        assert_eq!(question["reply_parameters"]["message_id"], 10_002);
        assert!(
            calls.iter().any(|(_, params)| params["text"]
                .as_str()
                .is_some_and(|text| text.contains("limits"))),
            "a command answers about now"
        );
        bot.on_update(&mut host, press(5, 300, 77, &button(&question, "Run")));
        assert_eq!(
            sends(&host).last().unwrap().as_str(),
            "send s1 Fix the parser\n\nand its tests"
        );
        let calls = tg.flush(&mut bot, &mut host);
        let result = calls
            .iter()
            .find(|(method, _)| method == "editMessageText")
            .unwrap();
        assert_eq!(result.1["text"], "▶ Running");
        // Pressed again (or a question of the last run): it's old.
        bot.on_update(&mut host, press(6, 300, 77, &button(&question, "Run")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].1["text"], "This question is old");
        assert_eq!(sends(&host).len(), 2);
    }

    #[test]
    fn dont_run_turns_the_question_into_its_result() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        an_hour_later(
            &mut bot,
            &mut host,
            vec![user_message(1, Some(77), "Deploy it")],
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            calls[0].1["text"],
            "This came while Flux was closed. Run it?"
        );
        bot.on_update(
            &mut host,
            press(2, 300, 77, &button(&calls[0].1, "Don't run")),
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[1].1["text"], "✕ Not run");
        assert!(calls[1].1.get("reply_markup").is_none());
        assert!(sends(&host).is_empty());
    }

    #[test]
    fn never_notes_once_and_always_goes_at_once() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        host.settings.insert(SETTING.into(), json!(NEVER));
        an_hour_later(
            &mut bot,
            &mut host,
            vec![
                user_message(1, Some(77), "One"),
                user_message(2, Some(77), "Two"),
            ],
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls.len(), 1, "one note");
        assert_eq!(
            calls[0].1["text"],
            "2 messages came while Flux was closed and weren't run."
        );
        assert!(calls[0].1.get("reply_markup").is_none());
        host.settings.insert(SETTING.into(), json!(ALWAYS));
        an_hour_later(
            &mut bot,
            &mut host,
            vec![user_message(3, Some(77), "Three")],
        );
        assert_eq!(sends(&host), ["send s1 Three"]);
    }

    #[test]
    fn whatever_came_before_the_start_or_the_wake_is_late_however_short_the_wait() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        // Written ten seconds before Flux started (the author's check: not a minute — still late).
        host.now = (NOW_S as u64 + 10) * 1000;
        awake(&mut bot, &host);
        bot.on_update(&mut host, user_message(1, Some(77), "What's new?"));
        flush(&mut bot, &mut host);
        assert!(sends(&host).is_empty(), "held");
        let calls = tg.flush(&mut bot, &mut host);
        assert!(
            calls
                .iter()
                .any(|(_, params)| params["text"] == "This came while Flux was closed. Run it?")
        );
        // Written after the start (a second of the clocks' slack): it goes at once.
        let mut fresh = user_message(2, Some(77), "And now?");
        fresh.message.as_mut().unwrap().date = NOW_S + 9;
        bot.on_update(&mut host, fresh);
        assert_eq!(sends(&host), ["send s1 And now?"]);
        // Written while the computer slept: late again once it wakes.
        host.now = (NOW_S as u64 + 600) * 1000;
        bot.on_claude(
            &mut host,
            crate::bot::ClaudeEvent::System(SystemEvent::DidWake),
        );
        let mut asleep = user_message(3, Some(77), "Are you there?");
        asleep.message.as_mut().unwrap().date = NOW_S + 300;
        bot.on_update(&mut host, asleep);
        assert_eq!(sends(&host).len(), 1, "held, not sent");
    }

    #[test]
    fn a_late_new_chat_asks_then_goes_on_to_the_project() {
        let mut host = FakeHost::default();
        host.projects = vec![flux_plugin_api::remote::Project {
            path: "/work/flux".into(),
            name: "flux".into(),
            open: true,
        }];
        let mut bot = crate::testing::bound_bot(&mut host);
        let mut tg = Telegram::default();
        an_hour_later(
            &mut bot,
            &mut host,
            vec![
                topic_created(1, 80, "Fix the parser"),
                user_message(2, Some(80), "Fix the parser"),
            ],
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].1["message_thread_id"], 80);
        assert_eq!(
            calls[0].1["text"],
            "This came while Flux was closed. Run it?"
        );
        bot.on_update(&mut host, press(3, 300, 80, &button(&calls[0].1, "Run")));
        let calls = tg.flush(&mut bot, &mut host);
        let ask = calls
            .iter()
            .find(|(method, _)| method == "sendMessage")
            .unwrap();
        assert_eq!(ask.1["text"], "Which project?");
        // The question is the fake's third message (101 the question, 102 its edit).
        bot.on_update(&mut host, press(4, 103, 80, &button(&ask.1, "flux")));
        assert_eq!(sends(&host), ["new /work/flux Fix the parser"]);
    }
}
