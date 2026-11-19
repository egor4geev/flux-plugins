//! The turn's end in a session's topic: «✅ Done · 3 files · +12 −4» with «Diff» and «Roll back»
//! when the turn changed files (part 3).
//!
//! - «Diff» → the turn's unified diff ([`Host::turn_diff`]) as a file in the topic
//!   (`<topic>-turn-<n>.diff`, [`telegram::send_document`] — an upload in the topic's lane, so it
//!   keeps its place among the topic's messages), its caption the turn's numbers.
//! - «Roll back» → what the rollback would do ([`Host::check_rollback`]) as a question in the same
//!   message: the files that go back, the new ones that go to the Trash, the edits made after
//!   Claude that go too; «Roll back» there rolls the turn back ([`Host::rollback`]) and the message
//!   says so, without buttons; «Cancel» brings the buttons back. An error (the turn can't be rolled
//!   back any more, the session works) — a calm line, the buttons gone.
//!
//! The buttons name the turn by its number (the key) and the session by the topic they are in, and
//! the line is the message's first paragraph: nothing is kept, they work after a restart too.

use flux_plugin_api::remote::RollbackCheck;
use serde_json::Value;

use crate::bot::Bot;
use crate::conversation;
use crate::host::{Host, trf};
use crate::keys::{self, Kind};
use crate::render;
use crate::telegram::{self, Button, CallbackQuery, Keyboard, TgError};
use crate::transport::{Lane, Module, Purpose};

/// The files named in the question at most; the rest are «…».
const NAMED: usize = 3;

/// «Diff» and «Roll back» under the turn's «Done».
pub fn keyboard(host: &dyn Host, turn: u32) -> Keyboard {
    let data = |choice: &str| keys::encode(Kind::Turn, u64::from(turn), choice);
    vec![vec![
        Button::press(&host.tr("Diff"), data("diff")),
        Button::press(&host.tr("Roll back"), data("back")),
    ]]
}

pub fn on_press(
    bot: &mut Bot,
    host: &mut dyn Host,
    query: &CallbackQuery,
    turn: u64,
    choice: &str,
) {
    let Some(chat) = bot.chat() else {
        bot.answer_press(host, query, None);
        return;
    };
    let Some(message) = query.message.as_ref() else {
        bot.answer_press(host, query, None);
        return;
    };
    let pressed = message.message_id;
    let session = message
        .thread()
        .and_then(|thread| bot.store.topic_by_thread(thread))
        .map(|topic| (topic.thread, topic.session.clone(), topic.title.clone()));
    let Some((thread, session, title)) = session else {
        let toast = host.tr("The session is gone");
        bot.answer_press(host, query, Some(&toast));
        return;
    };
    let turn = turn as u32;
    // «✅ Done · …»: the message's first paragraph, whatever it shows below.
    let line = message
        .words()
        .split("\n\n")
        .next()
        .filter(|line| !line.trim().is_empty())
        .map_or_else(|| host.tr("✅ Done"), str::to_string);
    let line = render::escape(&line);
    let edit = |bot: &mut Bot, host: &mut dyn Host, html: &str, keyboard: Option<&Keyboard>| {
        let call = telegram::edit_text(chat, pressed, html, keyboard);
        bot.call(
            host,
            call,
            Lane::Topic(thread),
            Purpose::new(Module::Turn, "edit"),
        );
    };
    match choice {
        "diff" => match host.turn_diff(&session, turn) {
            Ok(diff) if diff.trim().is_empty() => {
                let toast = host.tr("The turn changed nothing");
                bot.answer_press(host, query, Some(&toast));
            }
            Ok(diff) => {
                bot.answer_press(host, query, None);
                let name = format!("{}-turn-{turn}.diff", file_stem(&title, &session));
                let caption = render::escape(&caption(host, turn, &diff));
                let call =
                    telegram::send_document(chat, Some(thread), &name, diff.into_bytes(), &caption);
                bot.call(
                    host,
                    call,
                    Lane::Topic(thread),
                    Purpose::new(Module::Turn, "diff"),
                );
                conversation::sent_to_topic(bot, host, thread);
            }
            Err(err) => {
                let toast = trf(host, "Couldn't get the diff: {0}", &[&err]);
                bot.answer_press(host, query, Some(&toast));
            }
        },
        "back" => {
            bot.answer_press(host, query, None);
            match host.check_rollback(&session, turn) {
                Ok(check) => {
                    let html = format!(
                        "{line}\n\n<b>{}</b>\n{}",
                        render::escape(&host.tr("Roll back the turn?")),
                        render::escape(&what_goes(host, &check)),
                    );
                    let data = |choice: &str| keys::encode(Kind::Turn, u64::from(turn), choice);
                    let keyboard = vec![vec![
                        Button::press(&host.tr("Roll back"), data("yes")).red(),
                        Button::press(&host.tr("Cancel"), data("no")),
                    ]];
                    edit(bot, host, &html, Some(&keyboard));
                }
                Err(err) => {
                    let note = trf(host, "Can't roll back: {0}", &[&err]);
                    edit(
                        bot,
                        host,
                        &format!("{line}\n\n{}", render::escape(&note)),
                        None,
                    );
                }
            }
        }
        "yes" => {
            bot.answer_press(host, query, None);
            let note = match host.rollback(&session, turn) {
                Ok(()) => host.tr("↩️ Rolled back"),
                Err(err) => trf(host, "Can't roll back: {0}", &[&err]),
            };
            edit(
                bot,
                host,
                &format!("{line}\n\n{}", render::escape(&note)),
                None,
            );
        }
        "no" => {
            bot.answer_press(host, query, None);
            let buttons = keyboard(host, turn);
            edit(bot, host, &line, Some(&buttons));
        }
        _ => bot.answer_press(host, query, None),
    }
}

/// The question's sentence: «2 files go back to how they were, 1 new file goes to the Trash. Your
/// edits made after Claude in main.rs go too.»
fn what_goes(host: &dyn Host, check: &RollbackCheck) -> String {
    let back = check
        .files
        .iter()
        .filter(|file| !check.created.contains(file))
        .count() as u64;
    let mut parts = Vec::new();
    if back > 0 {
        parts.push(render::count(
            host,
            back,
            "{0} file goes back to how it was",
            "{0} files go back to how they were",
        ));
    }
    if !check.created.is_empty() {
        parts.push(render::count(
            host,
            check.created.len() as u64,
            "{0} new file goes to the Trash",
            "{0} new files go to the Trash",
        ));
    }
    if parts.is_empty() {
        parts.push(host.tr("Nothing changes on disk"));
    }
    let mut sentence = format!("{}.", parts.join(", "));
    if !check.changed_since.is_empty() {
        let mut names: Vec<String> = check
            .changed_since
            .iter()
            .take(NAMED)
            .map(|path| crate::general::folder_name(path))
            .collect();
        if check.changed_since.len() > NAMED {
            names.push("…".into());
        }
        sentence.push(' ');
        sentence.push_str(&trf(
            host,
            "Edits made after Claude go too: {0}.",
            &[&names.join(", ")],
        ));
    }
    sentence
}

/// «Turn 2 · 3 files · +12 −4», counted from the diff.
fn caption(host: &dyn Host, turn: u32, diff: &str) -> String {
    let (mut files, mut added, mut removed) = (0u64, 0u64, 0u64);
    for line in diff.lines() {
        if line.starts_with("+++ ") {
            files += 1;
        } else if line.starts_with("--- ") {
        } else if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    let files = render::count(host, files, "{0} file", "{0} files");
    format!(
        "{} · {files} · +{added} −{removed}",
        trf(host, "Turn {0}", &[&turn])
    )
}

/// The diff file's name: the topic's name in letters, digits and dashes, or the session's id.
fn file_stem(title: &str, session: &str) -> String {
    let mut stem = String::new();
    for c in title.chars() {
        if c.is_alphanumeric() || c == '_' {
            stem.push(c);
        } else if !stem.is_empty() && !stem.ends_with('-') {
            stem.push('-');
        }
        if stem.chars().count() >= 40 {
            break;
        }
    }
    let stem = stem.trim_end_matches('-');
    if stem.is_empty() {
        session.chars().take(8).collect()
    } else {
        stem.to_string()
    }
}

pub fn on_answer(_bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    if let Err(err) = result
        && !err.not_modified()
        && !err.topic_gone()
    {
        host.log(&format!("A turn's {tag} failed: {err:?}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::ClaudeEvent;
    use crate::testing::{FakeHost, OWNER, Telegram, bot_with_topic, button, buttons};
    use flux_plugin_api::remote::{TurnFile, TurnSummary};
    use serde_json::json;

    /// The owner's press under the bot's message `text` in topic 77.
    fn press(update: i64, message: i64, text: &str, data: &str) -> crate::telegram::Update {
        let query = json!({
            "id": format!("q{update}"), "from": { "id": OWNER, "is_bot": false, "first_name": "Egor" },
            "message": { "message_id": message, "message_thread_id": 77, "text": text,
                         "chat": { "id": OWNER, "type": "private" },
                         "from": { "id": 77, "is_bot": true, "first_name": "Flux" } },
            "data": data,
        });
        crate::telegram::Update {
            update_id: update,
            callback_query: Some(serde_json::from_value(query).unwrap()),
            ..Default::default()
        }
    }

    fn ended(turn: u32, files: usize) -> ClaudeEvent {
        let files = (0..files)
            .map(|index| TurnFile {
                path: format!("src/f{index}.rs"),
                added: 3,
                removed: 1,
                created: false,
            })
            .collect();
        ClaudeEvent::TurnEnded(
            "s1".into(),
            TurnSummary {
                turn,
                files,
                failed: false,
                text: String::new(),
                seconds: 3,
            },
        )
    }

    /// The «Done» message's call.
    fn done(calls: &[(String, Value)]) -> Value {
        calls
            .iter()
            .find(|(method, params)| {
                method == "sendMessage" && params["text"].as_str().unwrap().starts_with("✅")
            })
            .expect("«Done»")
            .1
            .clone()
    }

    #[test]
    fn done_gets_the_buttons_only_when_files_changed() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_claude(&mut host, ended(1, 0));
        let calls = tg.flush(&mut bot, &mut host);
        let call = done(&calls);
        assert_eq!(call["text"], "✅ Done");
        assert!(buttons(&call).is_empty());
        bot.on_claude(&mut host, ended(2, 2));
        let calls = tg.flush(&mut bot, &mut host);
        let call = done(&calls);
        assert_eq!(call["text"], "✅ Done · 2 files · +6 −2");
        assert_eq!(buttons(&call), vec![vec!["Diff", "Roll back"]]);
        assert_eq!(button(&call, "Diff"), "d:2:diff");
    }

    #[test]
    fn the_diff_goes_as_a_file_in_the_topic() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        bot.store.topics[0].title = "Fix: the parser".into();
        let mut tg = Telegram::default();
        host.diff = "--- a/src/f0.rs\n+++ b/src/f0.rs\n@@ -1 +1,2 @@\n-old\n+new\n+more\n".into();
        bot.on_update(
            &mut host,
            press(1, 120, "✅ Done · 1 file · +2 −1", "d:2:diff"),
        );
        assert_eq!(host.claude.last().unwrap(), "diff s1 2");
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].0, "answerCallbackQuery");
        assert!(calls[0].1.get("text").is_none());
        let (method, params) = &calls[1];
        assert_eq!(method, "sendDocument");
        assert_eq!(params["message_thread_id"], 77);
        assert_eq!(params["chat_id"], OWNER);
        assert_eq!(params["caption"], "Turn 2 · 1 file · +2 −1");
        assert_eq!(params["document"]["filename"], "Fix-the-parser-turn-2.diff");
        assert_eq!(params["document"]["text"], host.diff.as_str());
        // An empty diff: a note over the chat, no file.
        host.diff.clear();
        bot.on_update(&mut host, press(2, 120, "✅ Done", "d:2:diff"));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1["text"], "The turn changed nothing");
    }

    #[test]
    fn roll_back_asks_in_the_same_message_then_rolls_back() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        host.rollback = RollbackCheck {
            files: vec!["src/a.rs".into(), "src/b.rs".into(), "src/new.rs".into()],
            changed_since: vec!["src/a.rs".into()],
            created: vec!["src/new.rs".into()],
        };
        let line = "✅ Done · 3 files · +12 −4";
        bot.on_update(&mut host, press(1, 120, line, "d:3:back"));
        assert_eq!(host.claude.last().unwrap(), "check s1 3");
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].0, "answerCallbackQuery");
        let (method, ask) = &calls[1];
        assert_eq!(method, "editMessageText");
        assert_eq!(ask["message_id"], 120);
        assert_eq!(
            ask["text"],
            "✅ Done · 3 files · +12 −4\n\n<b>Roll back the turn?</b>\n2 files go back to how they \
             were, 1 new file goes to the Trash. Edits made after Claude go too: a.rs."
        );
        assert_eq!(buttons(ask), vec![vec!["Roll back", "Cancel"]]);
        // «Cancel»: the line and its buttons again.
        let text = ask["text"]
            .as_str()
            .unwrap()
            .replace("<b>", "")
            .replace("</b>", "");
        bot.on_update(&mut host, press(2, 120, &text, &button(ask, "Cancel")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[1].1["text"], line);
        assert_eq!(buttons(&calls[1].1), vec![vec!["Diff", "Roll back"]]);
        // «Roll back» → the question → «Roll back»: done, no buttons.
        bot.on_update(&mut host, press(3, 120, line, "d:3:back"));
        tg.flush(&mut bot, &mut host);
        bot.on_update(&mut host, press(4, 120, &text, "d:3:yes"));
        assert_eq!(host.claude.last().unwrap(), "rollback s1 3");
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[1].1["text"], format!("{line}\n\n↩️ Rolled back"));
        assert!(buttons(&calls[1].1).is_empty());
    }

    #[test]
    fn a_turn_that_cant_roll_back_says_so_calmly() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        host.fail = Some("Only the last turn can be rolled back".into());
        bot.on_update(
            &mut host,
            press(1, 120, "✅ Done · 1 file · +1 −0", "d:1:back"),
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            calls[1].1["text"],
            "✅ Done · 1 file · +1 −0\n\nCan't roll back: Only the last turn can be rolled back"
        );
        assert!(buttons(&calls[1].1).is_empty());
        // A topic without a session: a note over the chat.
        bot.store.forget_topic(77);
        bot.on_update(&mut host, press(2, 120, "✅ Done", "d:1:diff"));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].1["text"], "The session is gone");
    }
}
