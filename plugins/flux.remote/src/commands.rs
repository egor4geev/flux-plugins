//! The bot's commands — the «/» menu by the input field (`setMyCommands` for the owner's chat, in
//! Flux's language, on each connection of the bound bot, [`publish`]): `/sessions`, `/rename`,
//! `/delete`, `/mode`, `/usage`, `/settings` ([`crate::menu`]); `/start` stays the binding's. Any other
//! `/word` is the user's text for Claude (its own commands: `/compact`).
//!
//! A command is answered where it was typed: a session's topic, «Общее», a new chat's topic. Typed
//! outside topics, it opens a topic of its own (Telegram names it after the command, as the
//! binding's `/start`): that topic goes, and «Общее» answers ([`on_topic_created`]).
//!
//! - `/sessions` — every session of Flux, the recent first ([`PAGE`] to a page, «Ещё…»), each as
//!   «Общее»'s status message has it, with a button: a session with a topic — the bot posts «📍»
//!   and its state there, so the topic rises to the top of the list (links to topics open
//!   nothing); without one — «Продолжить здесь» ([`crate::handoff`]).
//!
//! No `/stop`: while the bot writes in a topic, Telegram holds the user's messages there until the
//! draft ends — «Стоп» under the draft stops the turn (the author, 2026-11-20: cut it).
//! - `/rename <name>` — in a session's topic: the session's title in Flux (its tab) and the topic's
//!   name; without a name the bot asks, and the topic's next message is it.
//! - `/delete` — in a session's topic, after a question in the same message: the session closes as
//!   the Claude console's Close closes it (it leaves «Сейчас» for the history) and the topic goes.
//! - `/mode` — in a session's topic: the modes as buttons, the current one marked; «Без проверок»
//!   asks first, in the same message. The topic's first line follows the mode
//!   ([`crate::newchat::on_session`]).
//! - `/usage` — the 5-hour and the 7-day windows: how much is used and when each resets, in the
//!   user's time ([`resets`]). Flux is asked each time (it answers with what came within a minute,
//!   or asks Claude Code): Claude may work elsewhere, and the last `ClaudeLimits` event can be
//!   hours old. When Flux can't ask, the last known ones come with their time.
//!
//! The buttons keep no state: their data names the session by [`handoff::key_for`] and the page or
//! the mode by the choice, so a button of the last run still works.

use std::collections::{BTreeMap, BTreeSet};

use flux_plugin_api::remote::{Mode, Session, UsageLimits, UsageWindow};
use flux_plugin_api::system::LocalTime;
use serde_json::Value;

use crate::bot::Bot;
use crate::conversation;
use crate::general::{self, folder_name, title_of};
use crate::handoff;
use crate::host::{Host, trf};
use crate::keys::{self, Kind};
use crate::newchat::mode_name;
use crate::render::{cut, escape};
use crate::telegram::{self, Button, CallbackQuery, Keyboard, Message, TgError};
use crate::transport::{Lane, Module, Purpose};

/// Sessions on a page of `/sessions`.
pub const PAGE: usize = 6;
/// A session's title on a button, in characters.
const TITLE_CHARS: usize = 28;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Sessions,
    Rename,
    Delete,
    Mode,
    Usage,
    Settings,
}

/// The «/» menu: (command, what it does — translated).
const COMMANDS: [(&str, Command, &str); 6] = [
    ("sessions", Command::Sessions, "Sessions in Flux"),
    ("rename", Command::Rename, "Rename the session"),
    ("delete", Command::Delete, "Delete the session"),
    ("mode", Command::Mode, "The session's mode"),
    ("usage", Command::Usage, "Claude's limits"),
    ("settings", Command::Settings, "The bot's settings"),
];

/// The modes `/mode` sets, by their choice in the buttons' data (`Other` is shown, never set).
const MODES: [(Mode, &str); 4] = [
    (Mode::Ask, "ask"),
    (Mode::AcceptEdits, "edits"),
    (Mode::Plan, "plan"),
    (Mode::Bypass, "bypass"),
];

#[derive(Default)]
pub struct State {
    /// The topics the user's commands outside topics opened: they go when the command comes.
    opened: BTreeSet<i64>,
    /// `/rename` without a name: the topic whose next message is the session's new title, and the
    /// session.
    renaming: BTreeMap<i64, String>,
}

/// The command a text starts with — "/mode", "/mode@flux_bot"; none — not one of the bot's.
pub fn parse(text: &str) -> Option<Command> {
    let word = text.trim().strip_prefix('/')?.split_whitespace().next()?;
    let name = word.split('@').next()?;
    COMMANDS
        .iter()
        .find(|(command, ..)| command.eq_ignore_ascii_case(name))
        .map(|(_, command, _)| *command)
}

/// The owner's «/» menu, in Flux's language (on each connection: the language may have changed).
pub fn publish(bot: &mut Bot, host: &mut dyn Host) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let commands: Vec<(&str, String)> = COMMANDS
        .iter()
        .map(|(name, _, about)| (*name, host.tr(about)))
        .collect();
    bot.call(
        host,
        telegram::set_commands(chat, &commands),
        Lane::Topic(0),
        Purpose::new(Module::Commands, "menu"),
    );
}

/// A topic the user's message outside topics opened, named after one of the bot's commands: it
/// goes when the command comes. True — it was a command's.
pub fn on_topic_created(bot: &mut Bot, _host: &mut dyn Host, message: &Message) -> bool {
    let (Some(created), Some(thread)) = (&message.forum_topic_created, message.thread()) else {
        return false;
    };
    if !created.is_name_implicit || parse(&created.name).is_none() {
        return false;
    }
    bot.commands.opened.insert(thread);
    true
}

/// The user's message: one of the bot's commands is run (true); anything else isn't theirs.
pub fn on_message(bot: &mut Bot, host: &mut dyn Host, message: &Message) -> bool {
    let Some(command) = message.text.as_deref().and_then(parse) else {
        return false;
    };
    let Some((thread, session)) = place(bot, host, message) else {
        return true;
    };
    bot.commands.renaming.remove(&thread);
    match command {
        Command::Sessions => {
            let (html, keyboard) = sessions_page(bot, host, 0);
            send(bot, host, thread, &html, Some(&keyboard));
        }
        Command::Rename => match session {
            Some(session) => {
                let name = message.text.as_deref().map(argument).unwrap_or_default();
                if name.is_empty() {
                    bot.commands.renaming.insert(thread, session);
                    let text = host.tr("What should the session be called? Write the name.");
                    send(bot, host, thread, &escape(&text), None);
                } else {
                    rename(bot, host, thread, &session, &name);
                }
            }
            None => {
                let text = host.tr("Only a session is renamed: send /rename in its topic.");
                send(bot, host, thread, &escape(&text), None);
            }
        },
        Command::Delete => match session {
            Some(session) => {
                let html = format!(
                    "<b>{}</b>\n{}",
                    escape(&host.tr("Delete the session?")),
                    escape(&host.tr("The topic goes, and the session closes in Flux: it stays among the recent ones there."))
                );
                let key = handoff::key_for(&session);
                let keyboard = vec![vec![
                    Button::press(
                        &host.tr("Delete"),
                        keys::encode(Kind::Command, key, "delete"),
                    )
                    .red(),
                    Button::press(&host.tr("Cancel"), keys::encode(Kind::Command, key, "keep")),
                ]];
                send(bot, host, thread, &html, Some(&keyboard));
            }
            None => {
                let text = host.tr("Only a session is deleted: send /delete in its topic.");
                send(bot, host, thread, &escape(&text), None);
            }
        },
        Command::Mode => match session {
            Some(session) => {
                let (html, keyboard) = modes(bot, host, &session);
                send(bot, host, thread, &html, Some(&keyboard));
            }
            None => {
                let text = host.tr("The mode is a session's: send /mode in its topic.");
                send(bot, host, thread, &escape(&text), None);
            }
        },
        Command::Usage => usage(bot, host, thread),
        Command::Settings => crate::menu::open(bot, host, thread),
    }
    true
}

/// The words after a command: "/rename New name" → "New name".
fn argument(text: &str) -> String {
    let text = text.trim();
    text.split_once(char::is_whitespace)
        .map(|(_, rest)| rest.trim().to_string())
        .unwrap_or_default()
}

/// The topic's message after `/rename` without a name: the session's new title (true — taken).
pub fn take_title(bot: &mut Bot, host: &mut dyn Host, message: &Message) -> bool {
    let Some(thread) = message.thread() else {
        return false;
    };
    let name = message
        .text
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() || !bot.commands.renaming.contains_key(&thread) {
        return false;
    }
    let Some(session) = bot.commands.renaming.remove(&thread) else {
        return false;
    };
    rename(bot, host, thread, &session, &name);
    true
}

// --- /rename, /delete ---

/// The session's new title in Flux, and the topic's name at once (the session's title follows when
/// Claude Code has it: the same name, nothing more to do).
fn rename(bot: &mut Bot, host: &mut dyn Host, thread: i64, session: &str, name: &str) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let title = telegram::topic_name(name);
    if let Err(err) = host.rename(session, &title) {
        let text = trf(host, "Couldn't rename the session: {0}", &[&err]);
        send(bot, host, thread, &escape(&text), None);
        return;
    }
    if let Some(topic) = bot
        .store
        .topics
        .iter_mut()
        .find(|topic| topic.thread == thread)
    {
        topic.title = title.clone();
    }
    bot.save(host);
    bot.call(
        host,
        telegram::rename_topic(chat, thread, &title),
        Lane::Topic(thread),
        Purpose::new(Module::Commands, "rename"),
    );
}

/// «Удалить» under `/delete`'s question: the session closes in Flux, the topic goes — before Flux
/// says the session is gone (its note would go to the topic otherwise).
fn delete(bot: &mut Bot, host: &mut dyn Host, query: &CallbackQuery, session: &str) {
    let Some(chat) = bot.chat() else {
        return;
    };
    bot.answer_press(host, query, None);
    if let Err(err) = host.close(session) {
        let text = trf(host, "Couldn't close the session: {0}", &[&err]);
        edit(bot, host, query, &escape(&text), None);
        return;
    }
    let Some(thread) = bot
        .store
        .topic_by_session(session)
        .map(|topic| topic.thread)
    else {
        return;
    };
    bot.store.forget_topic(thread);
    bot.save(host);
    bot.call(
        host,
        telegram::delete_topic(chat, thread),
        Lane::Topic(thread),
        Purpose::new(Module::Commands, "delete"),
    );
}

/// Where a command is answered: (the topic, its session). The topic a command outside topics
/// opened goes — «Общее» answers then.
fn place(bot: &mut Bot, host: &mut dyn Host, message: &Message) -> Option<(i64, Option<String>)> {
    let chat = bot.chat()?;
    let thread = message.thread();
    if let Some(thread) = thread
        && bot.commands.opened.remove(&thread)
    {
        bot.call(
            host,
            telegram::delete_topic(chat, thread),
            Lane::Topic(thread),
            Purpose::new(Module::Commands, "opened"),
        );
        return bot.store.general.map(|general| (general, None));
    }
    match thread {
        Some(thread) => {
            let session = bot
                .store
                .topic_by_thread(thread)
                .map(|topic| topic.session.clone());
            Some((thread, session))
        }
        None => bot.store.general.map(|general| (general, None)),
    }
}

// --- /sessions ---

/// A page of `/sessions`: the text and the buttons.
fn sessions_page(bot: &Bot, host: &dyn Host, page: usize) -> (String, Keyboard) {
    let mut sessions: Vec<&Session> = bot.sessions.values().collect();
    if sessions.is_empty() {
        return (escape(&host.tr("Flux has no sessions now.")), Vec::new());
    }
    sessions.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    let pages = sessions.len().div_ceil(PAGE);
    let page = page % pages;
    let mut lines = vec![format!("<b>{}</b>", escape(&host.tr("Sessions in Flux")))];
    let mut rows = Keyboard::new();
    for session in sessions.iter().skip(page * PAGE).take(PAGE) {
        let topic = bot.store.topic_by_session(&session.id);
        let title = topic
            .map(|topic| topic.title.clone())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| title_of(session));
        let label = format!(
            "<b>{}</b> · {}",
            escape(&title),
            escape(&folder_name(&session.project))
        );
        lines.push(general::session_line(host, session, &label));
        let key = handoff::key_for(&session.id);
        let short = cut(&title, TITLE_CHARS);
        rows.push(vec![if topic.is_some() {
            Button::press(
                &format!("📍 {short}"),
                keys::encode(Kind::Command, key, "go"),
            )
        } else {
            Button::press(
                &format!("{short} · {}", host.tr("Continue here")),
                keys::encode(Kind::Handoff, key, ""),
            )
        }]);
    }
    if pages > 1 {
        lines.push(String::new());
        let of = trf(host, "Page {0} of {1}", &[&(page + 1), &pages]);
        lines.push(format!("<i>{}</i>", escape(&of)));
        let next = ((page + 1) % pages) as u64;
        rows.push(vec![Button::press(
            &host.tr("More…"),
            keys::encode(Kind::Command, next, "more"),
        )]);
    }
    (lines.join("\n"), rows)
}

/// «📍»: the session's topic gets a line with its state, so it rises to the top of the list.
fn raise(bot: &mut Bot, host: &mut dyn Host, query: &CallbackQuery, session: &str) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let Some(thread) = bot
        .store
        .topic_by_session(session)
        .map(|topic| topic.thread)
    else {
        // The topic went meanwhile: a new one, as «Продолжить здесь» makes it.
        bot.answer_press(host, query, None);
        handoff::continue_in_telegram(bot, host, session);
        return;
    };
    let toast = host.tr("The topic is at the top of the list");
    bot.answer_press(host, query, Some(&toast));
    let line = match bot.sessions.get(session) {
        Some(found) => {
            let title = bot
                .store
                .topic_by_session(session)
                .map(|topic| topic.title.clone())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| title_of(found));
            general::session_line(host, found, &escape(&title))
        }
        None => escape(&host.tr("This session is closed in Flux")),
    };
    // Without a sound: the user is looking at the chat.
    let call = telegram::send_message(chat, Some(thread), &format!("📍 {line}"), None).silent();
    bot.call(
        host,
        call,
        Lane::Topic(thread),
        Purpose::new(Module::Commands, format!("raise:{thread}:{session}")),
    );
    conversation::sent_to_topic(bot, host, thread);
}

// --- /mode ---

/// The modes' screen: the session's mode marked.
fn modes(bot: &Bot, host: &dyn Host, session: &str) -> (String, Keyboard) {
    let current = bot
        .sessions
        .get(session)
        .map_or(Mode::Ask, |found| found.mode);
    let mut html = format!("<b>{}</b>", escape(&host.tr("The session's mode")));
    if current == Mode::Other {
        let now = trf(host, "Now: {0}", &[&mode_name(host, current)]);
        html.push_str(&format!("\n{}", escape(&now)));
    }
    let key = handoff::key_for(session);
    let buttons: Vec<Button> = MODES
        .iter()
        .map(|(mode, choice)| {
            let name = mode_name(host, *mode);
            let label = if *mode == current {
                format!("✓ {name}")
            } else {
                name
            };
            Button::press(
                &label,
                keys::encode(Kind::Command, key, &format!("mode:{choice}")),
            )
        })
        .collect();
    (html, buttons.chunks(2).map(<[Button]>::to_vec).collect())
}

/// A press on `/mode`'s screen: a mode (`mode:plan`), «Без проверок» — its question first
/// (`mode:bypass` → `mode:bypass!`), «Отмена» — the modes again (`mode:back`).
fn mode_pressed(
    bot: &mut Bot,
    host: &mut dyn Host,
    query: &CallbackQuery,
    session: &str,
    choice: &str,
) {
    match choice {
        "back" => {
            bot.answer_press(host, query, None);
            let (html, keyboard) = modes(bot, host, session);
            edit(bot, host, query, &html, Some(&keyboard));
        }
        "bypass" => {
            bot.answer_press(host, query, None);
            let html = format!(
                "<b>{}</b>\n{}",
                escape(&host.tr("Turn on «No checks»?")),
                escape(
                    &host.tr("Claude will edit files and run commands without asking anything.")
                )
            );
            let key = handoff::key_for(session);
            let keyboard = vec![vec![
                Button::press(
                    &host.tr("Turn On"),
                    keys::encode(Kind::Command, key, "mode:bypass!"),
                )
                .red(),
                Button::press(
                    &host.tr("Cancel"),
                    keys::encode(Kind::Command, key, "mode:back"),
                ),
            ]];
            edit(bot, host, query, &html, Some(&keyboard));
        }
        _ => {
            let choice = choice.trim_end_matches('!');
            let Some(mode) = MODES
                .iter()
                .find(|(_, name)| *name == choice)
                .map(|(mode, _)| *mode)
            else {
                bot.answer_press(host, query, None);
                return;
            };
            if let Err(err) = host.set_mode(session, mode) {
                let toast = trf(host, "Couldn't change the mode: {0}", &[&err]);
                bot.answer_press(host, query, Some(&toast));
                return;
            }
            bot.answer_press(host, query, None);
            let html = escape(&trf(host, "Mode: {0}", &[&mode_name(host, mode)]));
            edit(bot, host, query, &html, None);
            // Flux tells the new mode with the session's next change: the topic's first line and
            // «Общее» follow it now.
            if let Some(found) = bot.sessions.get_mut(session) {
                found.mode = mode;
                let found = found.clone();
                crate::newchat::on_session(bot, host, &found);
                general::sessions_changed(bot, host);
            }
        }
    }
}

// --- /usage ---

fn usage(bot: &mut Bot, host: &mut dyn Host, thread: i64) {
    let now = (host.now_ms() / 1000) as i64;
    let asked = match host.limits() {
        Ok(limits) => {
            bot.limits = Some(limits);
            bot.limits_at = now;
            true
        }
        Err(err) => {
            host.log(&format!("No limits: {err}"));
            false
        }
    };
    // The last known ones while none of their windows has reset.
    let known = bot.limits.as_ref().filter(|limits| {
        (limits.five_hour.is_some() || limits.seven_day.is_some())
            && (asked || windows(limits).all(|(_, window)| window.resets_at > now))
    });
    let html = match known {
        Some(limits) if asked => usage_html(host, limits, now),
        Some(limits) => {
            let at = LocalTime::at(bot.limits_at, host.utc_offset(bot.limits_at)).hm();
            let note = trf(host, "Flux can't ask Claude Code now: as of {0}.", &[&at]);
            format!("{}\n{}", usage_html(host, limits, now), escape(&note))
        }
        None => escape(&host.tr("Flux can't tell Claude's limits now.")),
    };
    send(bot, host, thread, &html, None);
}

/// The windows there are: (their name, the window).
fn windows(limits: &UsageLimits) -> impl Iterator<Item = (&'static str, &UsageWindow)> {
    [
        ("5 hours: {0}", limits.five_hour.as_ref()),
        ("7 days: {0}", limits.seven_day.as_ref()),
    ]
    .into_iter()
    .filter_map(|(name, window)| window.map(|window| (name, window)))
}

/// «<b>Лимиты Claude</b>», «5 часов: 42%, сброс в 19:00», «7 дней: лимит исчерпан, сброс в пт,
/// 12:00» (the fullest window, when Claude refuses).
fn usage_html(host: &dyn Host, limits: &UsageLimits, now: i64) -> String {
    let fullest = windows(limits)
        .map(|(_, window)| window.percent)
        .max()
        .unwrap_or_default();
    let mut lines = vec![format!("<b>{}</b>", escape(&host.tr("Claude's limits")))];
    for (name, window) in windows(limits) {
        let used = if limits.rejected && window.percent == fullest {
            host.tr("limit reached")
        } else {
            format!("{}%", window.percent)
        };
        let value = format!("{used}, {}", resets(host, window.resets_at, now));
        lines.push(escape(&trf(host, name, &[&value])));
    }
    lines.join("\n")
}

/// When a window resets, in the user's time: «сброс в 19:00» today, «сброс в пт, 12:00» on
/// another day.
pub fn resets(host: &dyn Host, at: i64, now: i64) -> String {
    let local = LocalTime::at(at, host.utc_offset(at));
    let today = LocalTime::at(now, host.utc_offset(now));
    if local.day_number() == today.day_number() {
        trf(host, "resets at {0}", &[&local.hm()])
    } else {
        const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let day = host.tr(DAYS[local.weekday as usize % 7]);
        trf(host, "resets {0} at {1}", &[&day, &local.hm()])
    }
}

// --- Presses and answers ---

/// A press of `/sessions`' or `/mode`'s buttons: the key names the session (or the page
/// of «Ещё…»).
pub fn on_press(bot: &mut Bot, host: &mut dyn Host, query: &CallbackQuery, key: u64, choice: &str) {
    if choice == "more" {
        bot.answer_press(host, query, None);
        let (html, keyboard) = sessions_page(bot, host, key as usize);
        edit(bot, host, query, &html, Some(&keyboard));
        return;
    }
    let Some(session) = handoff::session_by_key(bot, key) else {
        let toast = host.tr("This session is closed in Flux");
        bot.answer_press(host, query, Some(&toast));
        return;
    };
    match choice.split_once(':') {
        Some(("mode", mode)) => mode_pressed(bot, host, query, &session, mode),
        _ if choice == "go" => raise(bot, host, query, &session),
        _ if choice == "delete" => delete(bot, host, query, &session),
        _ if choice == "keep" => {
            bot.answer_press(host, query, None);
            let text = host.tr("The session stays.");
            edit(bot, host, query, &escape(&text), None);
        }
        _ => bot.answer_press(host, query, None),
    }
}

pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    let Err(err) = result else {
        return;
    };
    // «📍» in a topic that is gone: the session gets a new one.
    if let Some(rest) = tag.strip_prefix("raise:")
        && err.topic_gone()
        && let Some((thread, session)) = rest.split_once(':')
        && let Ok(thread) = thread.parse::<i64>()
    {
        bot.store.forget_topic(thread);
        bot.save(host);
        handoff::continue_in_telegram(bot, host, session);
        return;
    }
    if !err.not_modified() {
        host.log(&format!("A command's call {tag} failed: {err:?}"));
    }
}

// --- Sending ---

fn send(bot: &mut Bot, host: &mut dyn Host, thread: i64, html: &str, keyboard: Option<&Keyboard>) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let keyboard = keyboard.filter(|keyboard| !keyboard.is_empty());
    let call = telegram::send_message(chat, Some(thread), html, keyboard);
    bot.call(
        host,
        call,
        Lane::Topic(thread),
        Purpose::new(Module::Commands, "send"),
    );
    conversation::sent_to_topic(bot, host, thread);
}

/// The pressed message becomes another screen (none — without buttons).
fn edit(
    bot: &mut Bot,
    host: &mut dyn Host,
    query: &CallbackQuery,
    html: &str,
    keyboard: Option<&Keyboard>,
) {
    let (Some(chat), Some(message)) = (bot.chat(), query.message.as_ref()) else {
        return;
    };
    let keyboard = keyboard.filter(|keyboard| !keyboard.is_empty());
    let call = telegram::edit_text(chat, message.message_id, html, keyboard);
    bot.call(
        host,
        call,
        Lane::Topic(message.thread().unwrap_or(0)),
        Purpose::new(Module::Commands, "edit"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::ClaudeEvent;
    use crate::testing::{
        FakeHost, GENERAL, NOW_S, OWNER, Telegram, bot_with_topic, bound_bot, button, buttons,
        press, session, topic_created, user_message,
    };
    use flux_plugin_api::remote::SessionState;
    use serde_json::json;

    /// The calls but «Общее»'s status message (sent to its topic first, then edited by its id).
    fn without_status(bot: &Bot, calls: Vec<(String, Value)>) -> Vec<(String, Value)> {
        let status = bot.store.status_message;
        calls
            .into_iter()
            .filter(|(method, params)| {
                !(method == "editMessageText"
                    && status.is_some()
                    && params["message_id"].as_i64() == status)
            })
            .collect()
    }

    fn named(id: &str, title: &str, state: SessionState, updated: i64) -> Session {
        let mut found = session(id, &format!("/work/{title}"), state);
        found.title = title.into();
        found.updated = updated;
        found
    }

    #[test]
    fn commands_are_the_bots_only() {
        assert_eq!(parse("/sessions"), Some(Command::Sessions));
        assert_eq!(parse(" /mode@flux_bot plan"), Some(Command::Mode));
        assert_eq!(parse("/USAGE"), Some(Command::Usage));
        assert_eq!(parse("/compact"), None, "Claude's own: the session's text");
        assert_eq!(parse("/start abc"), None, "the binding's");
        assert_eq!(parse("stop"), None);
        assert_eq!(parse("/stop"), None, "cut: «Стоп» under the draft does it");
    }

    #[test]
    fn the_menu_is_the_owners_chats_in_fluxs_language() {
        let mut host = FakeHost::default();
        let mut bot = bound_bot(&mut host);
        publish(&mut bot, &mut host);
        let (_, params) = host.last_call("setMyCommands").unwrap();
        assert_eq!(params["scope"], json!({ "type": "chat", "chat_id": OWNER }));
        assert_eq!(
            params["commands"][0],
            json!({ "command": "sessions", "description": "Sessions in Flux" })
        );
        let names: Vec<&str> = params["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|command| command["command"].as_str())
            .collect();
        assert_eq!(
            names,
            ["sessions", "rename", "delete", "mode", "usage", "settings"],
            "no /stop"
        );
    }

    #[test]
    fn a_command_outside_topics_loses_its_topic_and_general_answers() {
        let mut host = FakeHost::default();
        let mut bot = bound_bot(&mut host);
        let mut tg = Telegram::default();
        bot.on_update(&mut host, topic_created(1, 950, "/sessions"));
        bot.on_update(&mut host, user_message(2, Some(950), "/sessions"));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].0, "deleteForumTopic");
        assert_eq!(calls[0].1["message_thread_id"], 950);
        assert_eq!(calls[1].0, "sendMessage");
        assert_eq!(calls[1].1["message_thread_id"], GENERAL);
        assert_eq!(calls[1].1["text"], "Flux has no sessions now.");
        // Not a command: a new chat's first words, the topic stays.
        bot.on_update(&mut host, topic_created(3, 951, "/compact"));
        bot.on_update(&mut host, user_message(4, Some(951), "/compact"));
        let calls = tg.flush(&mut bot, &mut host);
        assert!(calls.iter().all(|(method, _)| method != "deleteForumTopic"));
    }

    #[test]
    fn sessions_go_recent_first_and_a_press_raises_the_topic() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        for found in [
            named("s1", "Parser", SessionState::Working, 100),
            named("s2", "Migrations", SessionState::Waiting, 200),
        ] {
            bot.sessions.insert(found.id.clone(), found);
        }
        bot.on_update(&mut host, user_message(1, Some(GENERAL), "/sessions"));
        let calls = tg.flush(&mut bot, &mut host);
        let list = &calls[0].1;
        assert_eq!(
            list["text"],
            "<b>Sessions in Flux</b>\n🟡 <b>Migrations</b> · Migrations — waiting for an answer\n🔵 <b>Parser</b> · Parser — working"
        );
        assert_eq!(
            buttons(list),
            [["Migrations · Continue here"], ["📍 Parser"]]
        );
        assert!(
            button(list, "Migrations").starts_with("h:"),
            "the handoff's"
        );
        bot.on_update(&mut host, press(2, 101, GENERAL, &button(list, "Parser")));
        let calls = {
            let calls = tg.flush(&mut bot, &mut host);
            without_status(&bot, calls)
        };
        assert_eq!(calls[0].0, "answerCallbackQuery");
        assert_eq!(calls[0].1["text"], "The topic is at the top of the list");
        assert_eq!(calls[1].1["message_thread_id"], 77);
        assert_eq!(calls[1].1["text"], "📍 🔵 Parser — working");
        assert_eq!(calls[1].1["disable_notification"], true);
    }

    #[test]
    fn many_sessions_have_pages() {
        let mut host = FakeHost::default();
        let mut bot = bound_bot(&mut host);
        let mut tg = Telegram::default();
        for index in 0..8 {
            let found = named(
                &format!("s{index}"),
                &format!("t{index}"),
                SessionState::Idle,
                index,
            );
            bot.sessions.insert(found.id.clone(), found);
        }
        bot.on_update(&mut host, user_message(1, Some(GENERAL), "/sessions"));
        let calls = tg.flush(&mut bot, &mut host);
        let list = calls[0].1.clone();
        assert_eq!(buttons(&list).len(), PAGE + 1);
        assert_eq!(
            buttons(&list)[0],
            ["t7 · Continue here"],
            "the recent first"
        );
        assert!(
            list["text"]
                .as_str()
                .unwrap()
                .ends_with("<i>Page 1 of 2</i>")
        );
        bot.on_update(&mut host, press(2, 101, GENERAL, &button(&list, "More")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[1].0, "editMessageText");
        assert_eq!(calls[1].1["message_id"], 101);
        assert_eq!(buttons(&calls[1].1)[0], ["t1 · Continue here"]);
    }

    #[test]
    fn rename_names_the_session_and_its_topic_or_asks_for_the_name() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_update(&mut host, user_message(1, Some(77), "/rename  Parser fix "));
        assert_eq!(host.claude.last().unwrap(), "rename s1 Parser fix");
        let calls = tg.flush(&mut bot, &mut host);
        assert!(
            calls
                .iter()
                .any(|(method, params)| method == "editForumTopic"
                    && params["message_thread_id"] == 77
                    && params["name"] == "Parser fix")
        );
        assert_eq!(bot.store.topic_by_thread(77).unwrap().title, "Parser fix");
        // No name: the bot asks, and the topic's next message is it — not a message to Claude.
        bot.on_update(&mut host, user_message(2, Some(77), "/rename"));
        let calls = tg.flush(&mut bot, &mut host);
        let calls = without_status(&bot, calls);
        assert_eq!(
            calls[0].1["text"],
            "What should the session be called? Write the name."
        );
        bot.on_update(&mut host, user_message(3, Some(77), "Migrations"));
        assert_eq!(host.claude.last().unwrap(), "rename s1 Migrations");
        assert!(!host.claude.iter().any(|line| line.starts_with("send s1")));
        // Elsewhere: where it works.
        bot.on_update(
            &mut host,
            user_message(4, Some(GENERAL), "/rename Something"),
        );
        let calls = tg.flush(&mut bot, &mut host);
        let calls = without_status(&bot, calls);
        assert!(
            calls.iter().any(|(_, params)| params["text"]
                == "Only a session is renamed: send /rename in its topic.")
        );
    }

    #[test]
    fn delete_asks_then_closes_the_session_and_the_topic_goes() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_update(&mut host, user_message(1, Some(77), "/delete"));
        let calls = tg.flush(&mut bot, &mut host);
        let calls = without_status(&bot, calls);
        let question = calls[0].1.clone();
        assert_eq!(
            question["text"],
            "<b>Delete the session?</b>\nThe topic goes, and the session closes in Flux: it stays among the recent ones there."
        );
        assert_eq!(buttons(&question), [["Delete", "Cancel"]]);
        // «Cancel»: nothing closes.
        bot.on_update(&mut host, press(2, 300, 77, &button(&question, "Cancel")));
        let calls = tg.flush(&mut bot, &mut host);
        let calls = without_status(&bot, calls);
        assert!(
            calls
                .iter()
                .any(|(_, params)| params["text"] == "The session stays.")
        );
        assert!(!host.claude.iter().any(|line| line.starts_with("close")));
        // «Delete»: the session closes in Flux, the topic goes and is forgotten.
        bot.on_update(&mut host, press(3, 300, 77, &button(&question, "Delete")));
        assert_eq!(host.claude.last().unwrap(), "close s1");
        let calls = tg.flush(&mut bot, &mut host);
        assert!(
            calls
                .iter()
                .any(|(method, params)| method == "deleteForumTopic"
                    && params["message_thread_id"] == 77)
        );
        assert!(bot.store.topic_by_thread(77).is_none());
        // Flux tells it's gone: no note to a topic that is no more.
        bot.on_claude(&mut host, ClaudeEvent::Gone("s1".into()));
        let calls = tg.flush(&mut bot, &mut host);
        assert!(
            !calls
                .iter()
                .any(|(_, params)| params["message_thread_id"] == 77),
            "{calls:?}"
        );
    }

    #[test]
    fn mode_marks_the_current_one_and_no_checks_asks_first() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.store.topics[0].head = Some(555);
        bot.sessions
            .insert("s1".into(), named("s1", "Parser", SessionState::Idle, 1));
        bot.on_update(&mut host, user_message(1, Some(77), "/mode"));
        let calls = tg.flush(&mut bot, &mut host);
        let screen = calls[0].1.clone();
        assert_eq!(screen["text"], "<b>The session's mode</b>");
        assert_eq!(
            buttons(&screen),
            [["✓ Ask", "Accept edits"], ["Plan", "No checks"]]
        );
        bot.on_update(&mut host, press(2, 101, 77, &button(&screen, "Plan")));
        assert_eq!(host.claude.last().unwrap(), "mode s1 Mode::Plan");
        // The topic's first line follows at once (Flux tells the mode with the session's next
        // change), and again with Flux's event: nothing more then.
        let calls = tg.flush(&mut bot, &mut host);
        let mut planning = named("s1", "Parser", SessionState::Idle, 1);
        planning.mode = Mode::Plan;
        bot.on_claude(&mut host, ClaudeEvent::Session(planning));
        assert!(
            tg.flush(&mut bot, &mut host)
                .iter()
                .all(|(_, params)| params["message_id"] != 555)
        );
        let head = calls
            .iter()
            .find(|(_, params)| params["message_id"] == 555)
            .unwrap();
        assert_eq!(head.1["text"], "📁 <b>project</b> · Plan");
        assert!(
            calls
                .iter()
                .any(|(_, params)| params["text"] == "Mode: Plan")
        );
        // «No checks»: the same message asks first; «Cancel» brings the modes back.
        bot.on_update(&mut host, press(3, 102, 77, &button(&screen, "No checks")));
        let calls = tg.flush(&mut bot, &mut host);
        let ask = calls[1].1.clone();
        assert_eq!(ask["message_id"], 102);
        assert_eq!(
            ask["text"],
            "<b>Turn on «No checks»?</b>\nClaude will edit files and run commands without asking anything."
        );
        assert!(host.claude.iter().all(|line| !line.contains("Bypass")));
        bot.on_update(&mut host, press(4, 102, 77, &button(&ask, "Cancel")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(buttons(&calls[1].1)[1], ["✓ Plan", "No checks"]);
        bot.on_update(&mut host, press(5, 102, 77, &button(&ask, "Turn On")));
        assert_eq!(host.claude.last().unwrap(), "mode s1 Mode::Bypass");
        let calls = tg.flush(&mut bot, &mut host);
        assert!(
            calls
                .iter()
                .any(|(_, params)| params["text"] == "📁 <b>project</b> · No checks")
        );
        // Outside a session's topic: where it works.
        bot.on_update(&mut host, user_message(6, Some(GENERAL), "/mode"));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            calls.last().unwrap().1["text"],
            "The mode is a session's: send /mode in its topic."
        );
    }

    #[test]
    fn usage_asks_flux_each_time_and_tells_the_windows_in_the_users_time() {
        let mut host = FakeHost::default();
        let mut bot = bound_bot(&mut host);
        let mut tg = Telegram::default();
        let usage = |bot: &mut Bot, host: &mut FakeHost, tg: &mut Telegram, update: i64| {
            bot.on_update(host, user_message(update, Some(GENERAL), "/usage"));
            let calls = tg.flush(bot, host);
            let calls = without_status(bot, calls);
            calls.last().unwrap().1["text"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            usage(&mut bot, &mut host, &mut tg, 1),
            "Flux can't tell Claude's limits now."
        );
        // NOW_S is a Wednesday, 11:06 UTC (the fake's zone).
        host.limits = Some(UsageLimits {
            five_hour: Some(UsageWindow {
                percent: 42,
                resets_at: NOW_S + 3600,
            }),
            seven_day: Some(UsageWindow {
                percent: 13,
                resets_at: NOW_S + 3 * 86_400,
            }),
            rejected: false,
        });
        assert_eq!(
            usage(&mut bot, &mut host, &mut tg, 2),
            "<b>Claude's limits</b>\n5 hours: 42%, resets at 12:06\n7 days: 13%, resets Sat at 11:06"
        );
        // An event is kept, yet Flux is asked again: Claude may have worked elsewhere since.
        let mut full = host.limits.unwrap();
        full.five_hour = Some(UsageWindow {
            percent: 100,
            resets_at: NOW_S + 1800,
        });
        full.rejected = true;
        bot.on_claude(&mut host, ClaudeEvent::Limits(full));
        assert!(
            usage(&mut bot, &mut host, &mut tg, 3).contains("5 hours: 42%, resets at 12:06"),
            "Flux's answer wins over the event"
        );
        assert_eq!(
            host.claude.iter().filter(|line| *line == "limits").count(),
            3
        );
        // Flux can't ask: the last known ones, with their time, while their windows last.
        host.limits = None;
        assert_eq!(
            usage(&mut bot, &mut host, &mut tg, 4),
            "<b>Claude's limits</b>\n5 hours: 42%, resets at 12:06\n7 days: 13%, resets Sat at 11:06\n\
             Flux can't ask Claude Code now: as of 11:06."
        );
        host.now += 3600 * 1000;
        assert_eq!(
            usage(&mut bot, &mut host, &mut tg, 5),
            "Flux can't tell Claude's limits now."
        );
    }
}
