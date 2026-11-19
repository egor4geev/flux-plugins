//! `/settings`: the plugin's settings as a menu of buttons in one message — a press edits the
//! message's text and buttons into the next screen, never a new message. They are the settings of
//! the plugin's page in Flux, read and written there ([`Host::setting`], [`Host::set_setting`]): a
//! change made in Flux while the menu is open shows on its next screen.
//!
//! - The main screen: a button per setting with its value — «Сообщения, пока Flux был закрыт:
//!   Спрашивать ›», «Не спать на зарядке: выкл ›», «Привязка ›» — and «Закрыть» (the message goes).
//! - A setting's screen: what it does in one sentence, the choices with ✓ on the current one,
//!   «‹ Назад».
//! - «Привязка»: whom the bot is bound to, «Отвязать…» → a short question and one sentence →
//!   «Отвязать» ([`binding::unbind`]) / «Отмена».
//!
//! A button's data is the screen or the choice (`late`, `late=never`): the menu keeps no state, so
//! a menu of the last run works too.

use serde_json::{Value, json};

use crate::binding;
use crate::bot::Bot;
use crate::conversation;
use crate::host::{Host, trf};
use crate::keys::{self, Kind};
use crate::render::escape;
use crate::telegram::{self, Button, CallbackQuery, Keyboard, TgError};
use crate::transport::{Lane, Module, Purpose};
use crate::{awake, late};

/// The choices of «Сообщения, пока Flux был закрыт»: (value, name).
const LATE: [(&str, &str); 3] = [
    (late::ASK, "Ask"),
    (late::ALWAYS, "Run always"),
    (late::NEVER, "Don't run"),
];

/// The menu's main screen, sent to a topic.
pub fn open(bot: &mut Bot, host: &mut dyn Host, thread: i64) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let (html, keyboard) = screen(bot, host, "main");
    let call = telegram::send_message(chat, Some(thread), &html, Some(&keyboard));
    bot.call(
        host,
        call,
        Lane::Topic(thread),
        Purpose::new(Module::Menu, "open"),
    );
    conversation::sent_to_topic(bot, host, thread);
}

/// A press in the menu: a choice is written first (`late=never`), then its screen shows.
pub fn on_press(
    bot: &mut Bot,
    host: &mut dyn Host,
    query: &CallbackQuery,
    _key: u64,
    choice: &str,
) {
    let (Some(chat), Some(message)) = (bot.chat(), query.message.clone()) else {
        bot.answer_press(host, query, None);
        return;
    };
    let lane = Lane::Topic(message.thread().unwrap_or(0));
    let mut shown = choice;
    if let Some((setting, value)) = choice.split_once('=') {
        if let Err(err) = set(bot, host, setting, value) {
            let toast = trf(host, "Couldn't change the setting: {0}", &[&err]);
            bot.answer_press(host, query, Some(&toast));
            return;
        }
        shown = setting;
    }
    bot.answer_press(host, query, None);
    match shown {
        "close" => bot.call(
            host,
            telegram::delete_message(chat, message.message_id),
            lane,
            Purpose::new(Module::Menu, "close"),
        ),
        "unbind!" => {
            let html =
                escape(&host.tr("The bot is unbound. You can bind it again in Flux's settings."));
            bot.call(
                host,
                telegram::edit_text(chat, message.message_id, &html, None),
                lane,
                Purpose::new(Module::Menu, "edit"),
            );
            binding::unbind(bot, host);
        }
        _ => {
            let (html, keyboard) = screen(bot, host, shown);
            bot.call(
                host,
                telegram::edit_text(chat, message.message_id, &html, Some(&keyboard)),
                lane,
                Purpose::new(Module::Menu, "edit"),
            );
        }
    }
}

pub fn on_answer(_bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    if let Err(err) = result
        && !err.not_modified()
    {
        host.log(&format!("The settings' menu: {tag} failed: {err:?}"));
    }
}

/// Writes a setting as its page in Flux would; the bot hears of it as of a change made there.
fn set(bot: &mut Bot, host: &mut dyn Host, setting: &str, value: &str) -> Result<(), String> {
    let (key, value) = match setting {
        "late" if LATE.iter().any(|(known, _)| *known == value) => (late::SETTING, json!(value)),
        "awake" => (awake::ON_POWER, json!(value == "on")),
        _ => return Err(format!("unknown setting {setting}")),
    };
    host.set_setting(key, &value)?;
    bot.settings_changed(host);
    Ok(())
}

/// A screen: its text and buttons.
fn screen(bot: &Bot, host: &dyn Host, name: &str) -> (String, Keyboard) {
    let press =
        |label: &str, choice: &str| Button::press(label, keys::encode(Kind::Menu, 0, choice));
    let back = || vec![press(&host.tr("‹ Back"), "main")];
    let marked = |name: String, on: bool| if on { format!("✓ {name}") } else { name };
    match name {
        "late" => {
            let current = late::setting(host);
            let mut rows: Keyboard = LATE
                .iter()
                .map(|(value, title)| {
                    vec![press(
                        &marked(host.tr(title), current == *value),
                        &format!("late={value}"),
                    )]
                })
                .collect();
            rows.push(back());
            let html = format!(
                "<b>{}</b>\n{}",
                escape(&host.tr("Messages that came while Flux was closed")),
                escape(&host.tr("What the bot does with messages that came while Flux was closed or the computer slept."))
            );
            (html, rows)
        }
        "awake" => {
            let on = awake_on(host);
            let rows = vec![
                vec![
                    press(&marked(host.tr("On"), on), "awake=on"),
                    press(&marked(host.tr("Off"), !on), "awake=off"),
                ],
                back(),
            ];
            let html = format!(
                "<b>{}</b>\n{}",
                escape(&host.tr("Don't sleep on the adapter")),
                escape(&host.tr("While the bot is on, the computer on the power adapter doesn't sleep; a closed lid still puts a laptop to sleep."))
            );
            (html, rows)
        }
        "bind" => {
            let owner = bot
                .store
                .owner
                .as_ref()
                .map(|owner| owner.name.clone())
                .unwrap_or_default();
            let html = format!(
                "<b>{}</b>\n{}",
                escape(&host.tr("Binding")),
                escape(&trf(host, "The bot is bound to {0}.", &[&owner]))
            );
            (
                html,
                vec![vec![press(&host.tr("Unbind…"), "unbind")], back()],
            )
        }
        "unbind" => {
            let html = format!(
                "<b>{}</b>\n{}",
                escape(&host.tr("Unbind the bot?")),
                escape(&host.tr("It stops answering until it is bound again in Flux's settings."))
            );
            let rows = vec![vec![
                press(&host.tr("Unbind"), "unbind!").red(),
                press(&host.tr("Cancel"), "bind"),
            ]];
            (html, rows)
        }
        _ => {
            let late_name = LATE
                .iter()
                .find(|(value, _)| *value == late::setting(host))
                .map_or("Ask", |(_, title)| *title);
            let late_line = format!(
                "{}: {} ›",
                host.tr("Messages that came while Flux was closed"),
                host.tr(late_name)
            );
            let awake_line = format!(
                "{}: {} ›",
                host.tr("Don't sleep on the adapter"),
                host.tr(if awake_on(host) { "on" } else { "off" })
            );
            let rows = vec![
                vec![press(&late_line, "late")],
                vec![press(&awake_line, "awake")],
                vec![press(&format!("{} ›", host.tr("Binding")), "bind")],
                vec![press(&host.tr("Close"), "close")],
            ];
            let html = format!(
                "<b>{}</b>\n{}",
                escape(&host.tr("Settings")),
                escape(&host.tr("The same as in Flux: Settings → Flux Remote."))
            );
            (html, rows)
        }
    }
}

fn awake_on(host: &dyn Host) -> bool {
    host.setting(awake::ON_POWER) == Some(Value::Bool(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        FakeHost, GENERAL, Telegram, bound_bot, button, buttons, press, user_message,
    };

    #[test]
    fn the_screens_replace_each_other_in_one_message() {
        let mut host = FakeHost::default();
        let mut bot = bound_bot(&mut host);
        let mut tg = Telegram::default();
        bot.on_update(&mut host, user_message(1, Some(GENERAL), "/settings"));
        let calls = tg.flush(&mut bot, &mut host);
        let main = calls[0].1.clone();
        assert_eq!(
            main["text"],
            "<b>Settings</b>\nThe same as in Flux: Settings → Flux Remote."
        );
        assert_eq!(
            buttons(&main),
            [
                ["Messages that came while Flux was closed: Ask ›"],
                ["Don't sleep on the adapter: off ›"],
                ["Binding ›"],
                ["Close"]
            ]
        );
        // A setting's screen, its choice, back: the same message each time.
        bot.on_update(
            &mut host,
            press(2, 101, GENERAL, &button(&main, "Messages")),
        );
        let calls = tg.flush(&mut bot, &mut host);
        let late = calls[1].1.clone();
        assert_eq!(calls[1].0, "editMessageText");
        assert_eq!(late["message_id"], 101);
        assert_eq!(
            buttons(&late),
            [["✓ Ask"], ["Run always"], ["Don't run"], ["‹ Back"]]
        );
        bot.on_update(
            &mut host,
            press(3, 101, GENERAL, &button(&late, "Don't run")),
        );
        assert_eq!(host.settings[late::SETTING], "never", "Flux's setting");
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(buttons(&calls[1].1)[2], ["✓ Don't run"]);
        // A change made in Flux meanwhile shows on the next screen.
        host.settings
            .insert(awake::ON_POWER.into(), Value::Bool(true));
        bot.on_update(&mut host, press(4, 101, GENERAL, &button(&late, "Back")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            buttons(&calls[1].1)[..2],
            [
                ["Messages that came while Flux was closed: Don't run ›"],
                ["Don't sleep on the adapter: on ›"]
            ]
        );
        let main = calls[1].1.clone();
        bot.on_update(&mut host, press(5, 101, GENERAL, &button(&main, "sleep")));
        let calls = tg.flush(&mut bot, &mut host);
        let awake_screen = calls[1].1.clone();
        assert_eq!(buttons(&awake_screen)[0], ["✓ On", "Off"]);
        bot.on_update(
            &mut host,
            press(6, 101, GENERAL, &button(&awake_screen, "Off")),
        );
        assert_eq!(host.settings[awake::ON_POWER], false);
        assert_eq!(
            host.claude.last().unwrap(),
            "awake Awake::Off",
            "the bot follows its own write"
        );
        bot.on_update(&mut host, press(7, 101, GENERAL, &button(&main, "Close")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls.last().unwrap().0, "deleteMessage");
        assert!(
            calls.iter().all(|(method, _)| method != "sendMessage"),
            "never a new message"
        );
    }

    #[test]
    fn unbinding_asks_first_then_forgets_the_chat() {
        let mut host = FakeHost::default();
        let mut bot = bound_bot(&mut host);
        let mut tg = Telegram::default();
        bot.on_update(&mut host, user_message(1, Some(GENERAL), "/settings"));
        let main = tg.flush(&mut bot, &mut host)[0].1.clone();
        bot.on_update(&mut host, press(2, 101, GENERAL, &button(&main, "Binding")));
        let calls = tg.flush(&mut bot, &mut host);
        let binding = calls[1].1.clone();
        assert_eq!(binding["text"], "<b>Binding</b>\nThe bot is bound to Egor.");
        bot.on_update(
            &mut host,
            press(3, 101, GENERAL, &button(&binding, "Unbind")),
        );
        let calls = tg.flush(&mut bot, &mut host);
        let ask = calls[1].1.clone();
        assert_eq!(
            ask["text"],
            "<b>Unbind the bot?</b>\nIt stops answering until it is bound again in Flux's settings."
        );
        assert_eq!(
            ask["reply_markup"]["inline_keyboard"][0][0]["style"],
            "danger"
        );
        assert!(bot.store.owner.is_some(), "nothing before the answer");
        bot.on_update(&mut host, press(4, 101, GENERAL, &button(&ask, "Unbind")));
        assert!(bot.store.owner.is_none() && bot.store.general.is_none());
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            calls[1].1["text"],
            "The bot is unbound. You can bind it again in Flux's settings."
        );
        assert!(calls.iter().any(|(method, _)| method == "deleteMyCommands"));
    }
}
