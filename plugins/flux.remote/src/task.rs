//! «From a task» in a new chat (part 3): between the project and the session, when a tracker's
//! plugin gives Flux the user's tasks ([`Host::my_tasks`] answers; no source — no step, the
//! session starts as before).
//!
//! - The step, in the project's question: «My tasks» / «Find…» / «No task». A task's key in the
//!   user's words (`[A-Z][A-Z0-9]+-\d+`) brings that task's card at once.
//! - «My tasks» → a button per task «KEY · title», [`PAGE`] a page, «More…»; «Find…» → «Write a key
//!   or words»: the topic's next message is the search, as the tracker's own search finds it
//!   ([`Host::search_tasks`]) — the results as buttons, and «Search again».
//! - A task → its card ([`Host::task`]): the key, the title, the status; the tracker's own Claude
//!   actions, a button each (plan ones marked), and «Discuss» — a session in plan mode with the task
//!   attached (its Markdown as a chip), the user's words its prompt («Let's discuss the task KEY.»
//!   when they were only the key: without words Flux would keep the chip in its field).
//! - An action → «⏳ …» while the tracker does what it does at the computer — the status, the
//!   branch, the tabs, the timer, Claude — but asks nothing ([`Host::run_task`], up to a minute;
//!   it runs on a short timer, after the press's answer and the line went). Then the topic is the
//!   session's: bound, renamed «KEY: title», the card its first line «📁 project · mode»; the
//!   tracker's notes (the branch it left alone…) go to the topic as they are; the words that opened
//!   the chat don't follow the action's prompt (they only opened it). An error — a calm line, and
//!   «Discuss» / «No task» to go on.
//!
//! The buttons carry the new chat's key and a short choice; this module keeps the rest (a button
//! of the last run does nothing).

use std::collections::BTreeMap;

use flux_plugin_api::remote::Mode;
use flux_plugin_api::tasks::{Task, TaskRun};
use serde_json::Value;

use crate::bot::Bot;
use crate::conversation;
use crate::host::{Host, NewSession, trf};
use crate::keys::{self, Kind};
use crate::newchat::{self, Chat};
use crate::render;
use crate::telegram::{self, Button, CallbackQuery, Keyboard, Message, TgError};
use crate::transport::{Lane, Module, Purpose};

/// Tasks on one page.
pub const PAGE: usize = 6;
/// The action runs this long after its press: the press's answer and the «⏳» line go first (the
/// run blocks the service).
const RUN_AFTER_MS: u32 = 300;

#[derive(Default)]
pub struct State {
    /// The new chats at their task step, by the new chat's key.
    steps: BTreeMap<u64, Step>,
    /// The action waiting for its moment: (timer, the chat's key, the action's index).
    run: Option<(u64, u64, usize)>,
}

/// A new chat at its task step.
struct Step {
    chat: Chat,
    /// The user's open tasks, as the source gave them when the step began.
    mine: Vec<Task>,
    /// The list the buttons show: the user's tasks, or the search's.
    shown: Vec<Task>,
    /// The search's words the list shows; none — «My tasks».
    found: Option<String>,
    page: usize,
    /// The topic's next message is a search.
    searching: bool,
    /// The task chosen: with its text and actions.
    task: Option<Task>,
}

/// Flux's sentence when no tracker plugin runs (`tasks.find`): then there is no step at all.
const NO_SOURCE: &str = "No plugin gives tasks";

/// The project is chosen: the task step when a tracker gives tasks (true — the chat is the step's
/// now); a task's key in the words brings its card at once. A tracker that can't give them now
/// (signed out…) gets a line saying why, and the chat goes on without a task.
pub fn offer(bot: &mut Bot, host: &mut dyn Host, key: u64, chat: &Chat) -> bool {
    let mine = match host.my_tasks() {
        Ok(mine) => mine,
        Err(error) if error == NO_SOURCE => return false,
        Err(error) => {
            let line = trf(host, "Tasks aren't available: {0}", &[&error.trim()]);
            conversation::send_note(bot, host, chat.thread, &render::escape(&line));
            return false;
        }
    };
    let named = task_key(&chat.text()).and_then(|named| host.task(&named).ok().flatten());
    let mut step = Step {
        chat: chat.clone(),
        mine,
        shown: Vec::new(),
        found: None,
        page: 0,
        searching: false,
        task: None,
    };
    let (html, keyboard) = match named {
        Some(task) => {
            let screen = card(host, key, &step.chat, &task, None);
            step.task = Some(task);
            screen
        }
        None => (
            format!(
                "{}\n{}",
                project_line(&step.chat),
                render::escape(&host.tr("Start from a task?"))
            ),
            vec![
                vec![
                    Button::press(&host.tr("My tasks"), data(key, "mine")),
                    Button::press(&host.tr("Find…"), data(key, "find")),
                ],
                vec![Button::press(&host.tr("No task"), data(key, "none"))],
            ],
        ),
    };
    let question = step.chat.question;
    let thread = step.chat.thread;
    bot.task.steps.insert(key, step);
    show(bot, host, key, thread, question, &html, &keyboard);
    true
}

/// The user's words (and pictures) in a topic at its task step: the search the step waits for, or
/// more words for the session (true — the topic is at its step).
pub fn take(
    bot: &mut Bot,
    host: &mut dyn Host,
    thread: i64,
    words: &str,
    images: &[String],
) -> bool {
    let Some((&key, step)) = bot
        .task
        .steps
        .iter_mut()
        .find(|(_, step)| step.chat.thread == thread)
    else {
        return false;
    };
    step.chat.images.extend(images.iter().cloned());
    if step.searching && !words.is_empty() {
        step.searching = false;
        search(bot, host, key, words);
    } else if !words.is_empty() {
        step.chat.words.push(words.to_string());
    }
    true
}

pub fn on_press(bot: &mut Bot, host: &mut dyn Host, query: &CallbackQuery, key: u64, choice: &str) {
    bot.answer_press(host, query, None);
    let Some(pressed) = query.message.as_ref().map(|message| message.message_id) else {
        return;
    };
    let Some(step) = bot.task.steps.get_mut(&key) else {
        return;
    };
    // The pressed message is the step's now: its questions go there, it becomes the first line.
    step.chat.question = Some(pressed);
    let thread = step.chat.thread;
    match choice {
        "mine" => {
            step.shown = step.mine.clone();
            step.found = None;
            step.page = 0;
            step.searching = false;
            let (html, keyboard) = list(host, key, step);
            show(bot, host, key, thread, Some(pressed), &html, &keyboard);
        }
        "more" => {
            step.page = (step.page + 1) % step.shown.len().div_ceil(PAGE).max(1);
            let (html, keyboard) = list(host, key, step);
            show(bot, host, key, thread, Some(pressed), &html, &keyboard);
        }
        "find" | "again" => {
            step.searching = true;
            let html = format!(
                "{}\n{}",
                project_line(&step.chat),
                render::escape(&host.tr("Write a key or words"))
            );
            let keyboard = vec![vec![Button::press(&host.tr("No task"), data(key, "none"))]];
            show(bot, host, key, thread, Some(pressed), &html, &keyboard);
        }
        "none" => start(bot, host, key, None),
        "talk" => {
            let task = step.task.clone();
            start(bot, host, key, task);
        }
        _ => {
            if let Some(index) = choice
                .strip_prefix('k')
                .and_then(|i| i.parse::<usize>().ok())
            {
                let Some(chosen) = step.shown.get(index).map(|task| task.key.clone()) else {
                    return;
                };
                open_card(bot, host, key, &chosen, Some(pressed));
            } else if let Some(index) = choice.strip_prefix('a').and_then(|i| i.parse().ok()) {
                act(bot, host, key, index, pressed);
            }
        }
    }
}

/// The action's moment came: it runs (true — the timer was ours).
pub fn on_timer(bot: &mut Bot, host: &mut dyn Host, timer: u64) -> bool {
    let Some((_, key, index)) = bot.task.run.filter(|(ours, _, _)| *ours == timer) else {
        return false;
    };
    bot.task.run = None;
    run(bot, host, key, index);
    true
}

pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    if let Some(key) = tag
        .strip_prefix("shown:")
        .and_then(|key| key.parse::<u64>().ok())
    {
        // A new message of the step (the search's results): its presses come from there.
        if let Ok(message) = result.and_then(telegram::result::<Message>)
            && let Some(step) = bot.task.steps.get_mut(&key)
        {
            step.chat.question = Some(message.message_id);
        }
        return;
    }
    if let Err(err) = result
        && !err.not_modified()
        && !err.topic_gone()
    {
        host.log(&format!("A task step's call failed: {err:?}"));
    }
}

/// The search's words: a key alone — that task's card; else the tasks found, as buttons.
fn search(bot: &mut Bot, host: &mut dyn Host, key: u64, words: &str) {
    let Some(thread) = bot.task.steps.get(&key).map(|step| step.chat.thread) else {
        return;
    };
    // The prompt's buttons go: the answer comes as a new message under the user's.
    if let (Some(chat), Some(prompt)) = (
        bot.chat(),
        bot.task.steps.get(&key).and_then(|step| step.chat.question),
    ) {
        let call = telegram::edit_keyboard(chat, prompt, None);
        bot.call(
            host,
            call,
            Lane::Topic(thread),
            Purpose::new(Module::Task, "edit"),
        );
    }
    if task_key(words).as_deref() == Some(words.trim())
        && open_card(bot, host, key, words.trim(), None)
    {
        return;
    }
    let found = host.search_tasks(words);
    let Some(step) = bot.task.steps.get_mut(&key) else {
        return;
    };
    let (html, keyboard) = match found {
        Ok(found) => {
            step.shown = found;
            step.found = Some(words.to_string());
            step.page = 0;
            list(host, key, step)
        }
        Err(err) => (
            render::escape(&trf(host, "The search failed: {0}", &[&err])),
            vec![vec![
                Button::press(&host.tr("Search again"), data(key, "again")),
                Button::press(&host.tr("No task"), data(key, "none")),
            ]],
        ),
    };
    show(bot, host, key, thread, None, &html, &keyboard);
}

/// A task's card (`message` — edited into it; none — a new message): false — no source knows the
/// key (a calm line instead when it was chosen from a list).
fn open_card(
    bot: &mut Bot,
    host: &mut dyn Host,
    key: u64,
    task: &str,
    message: Option<i64>,
) -> bool {
    let found = host.task(task);
    let Some(step) = bot.task.steps.get_mut(&key) else {
        return false;
    };
    let thread = step.chat.thread;
    match found {
        Ok(Some(task)) => {
            let (html, keyboard) = card(host, key, &step.chat, &task, None);
            step.task = Some(task);
            show(bot, host, key, thread, message, &html, &keyboard);
            true
        }
        Ok(None) if message.is_none() => false,
        result => {
            let line = match result {
                Err(error) => trf(host, "Couldn't open {0}: {1}", &[&task, &error]),
                Ok(_) => trf(host, "The tracker has no task {0}", &[&task]),
            };
            let html = format!("{}\n{}", project_line(&step.chat), render::escape(&line));
            let keyboard = vec![vec![
                Button::press(&host.tr("My tasks"), data(key, "mine")),
                Button::press(&host.tr("No task"), data(key, "none")),
            ]];
            show(bot, host, key, thread, message, &html, &keyboard);
            true
        }
    }
}

/// An action pressed: the card says it runs, the run waits for its moment ([`RUN_AFTER_MS`]).
fn act(bot: &mut Bot, host: &mut dyn Host, key: u64, index: usize, pressed: i64) {
    if bot.task.run.is_some() {
        return;
    }
    let Some(step) = bot.task.steps.get(&key) else {
        return;
    };
    let Some(task) = step.task.as_ref() else {
        return;
    };
    let Some(action) = task.actions.get(index) else {
        return;
    };
    let line = trf(
        host,
        "⏳ «{0}» — this takes up to a minute…",
        &[&action.title],
    );
    let (html, _) = card(host, key, &step.chat, task, Some(&line));
    let thread = step.chat.thread;
    show(
        bot,
        host,
        key,
        thread,
        Some(pressed),
        &html,
        &Keyboard::new(),
    );
    bot.task.run = Some((host.timer(RUN_AFTER_MS), key, index));
}

/// Runs the action in the tracker: the topic is the session's then, or the card says why not.
fn run(bot: &mut Bot, host: &mut dyn Host, key: u64, index: usize) {
    let Some(step) = bot.task.steps.get(&key) else {
        return;
    };
    let Some(task) = step.task.clone() else {
        return;
    };
    let Some(action) = task.actions.get(index).cloned() else {
        return;
    };
    let run = TaskRun {
        source: task.source.clone(),
        key: task.key.clone(),
        action: action.id.clone(),
        project: step.chat.project.path.clone(),
    };
    match host.run_task(&run) {
        Ok(started) => {
            let Some(step) = bot.task.steps.remove(&key) else {
                return;
            };
            let chat = step.chat;
            let mode = if action.plan { Mode::Plan } else { Mode::Ask };
            let title = format!("{}: {}", task.key, task.title);
            newchat::bind(bot, host, &chat, &started.session, Some(mode), Some(&title));
            for note in &started.notes {
                let html = render::escape(&format!("⚠️ {}", note.trim()));
                conversation::send_note(bot, host, chat.thread, &html);
            }
            // The action's own prompt says what to do: the words that opened the chat only
            // opened it (the author, 2026-11-20) — they don't follow it.
            conversation::expect_answer(bot, host, &started.session);
        }
        Err(err) => {
            let line = trf(host, "Couldn't run «{0}»: {1}", &[&action.title, &err]);
            let (html, _) = card(host, key, &step.chat, &task, Some(&line));
            let keyboard = vec![vec![
                Button::press(&host.tr("Discuss"), data(key, "talk")),
                Button::press(&host.tr("No task"), data(key, "none")),
            ]];
            let (thread, question) = (step.chat.thread, step.chat.question);
            show(bot, host, key, thread, question, &html, &keyboard);
        }
    }
}

/// «No task» (none) or «Discuss» (the task): the session in the chat's project — the task's one
/// in plan mode with the task attached.
fn start(bot: &mut Bot, host: &mut dyn Host, key: u64, task: Option<Task>) {
    let Some(step) = bot.task.steps.get(&key) else {
        return;
    };
    let chat = step.chat.clone();
    let mut text = chat.text();
    let mut spec = NewSession {
        project: chat.project.path.clone(),
        images: chat.images.clone(),
        ..Default::default()
    };
    if let Some(task) = &task {
        text = own_words(&text, &task.key);
        if text.is_empty() {
            // Only the key: Flux sends a session nothing without words (the chip would wait in its
            // field at the computer).
            text = trf(host, "Let's discuss the task {0}.", &[&task.key]);
        }
        let markdown = if task.markdown.trim().is_empty() {
            format!("# {}: {}", task.key, task.title)
        } else {
            task.markdown.clone()
        };
        spec.title = Some(format!("{}: {}", task.key, task.title));
        spec.attachments = vec![(task.key.clone(), markdown)];
        spec.mode = Some(Mode::Plan);
    }
    spec.prompt = (!text.is_empty()).then_some(text);
    match newchat::open(bot, host, &chat, &spec) {
        Ok(_) => {
            bot.task.steps.remove(&key);
        }
        Err(err) => {
            let note = trf(host, "Couldn't start the session: {0}", &[&err]);
            conversation::send_note(bot, host, chat.thread, &render::escape(&note));
        }
    }
}

/// The list's screen: «My tasks» or the search's results, a page of tasks, «More…».
fn list(host: &dyn Host, key: u64, step: &Step) -> (String, Keyboard) {
    let title = match &step.found {
        None if step.shown.is_empty() => host.tr("No open tasks"),
        None => host.tr("My tasks"),
        Some(words) if step.shown.is_empty() => trf(host, "Nothing found for «{0}»", &[words]),
        Some(words) => trf(host, "Found for «{0}»", &[words]),
    };
    let html = format!("{}\n{}", project_line(&step.chat), render::escape(&title));
    let mut rows: Keyboard = step
        .shown
        .iter()
        .enumerate()
        .skip(step.page * PAGE)
        .take(PAGE)
        .map(|(index, task)| {
            let text = render::cut(&format!("{} · {}", task.key, task.title), 48);
            vec![Button::press(&text, data(key, &format!("k{index}")))]
        })
        .collect();
    if step.shown.len() > PAGE {
        rows.push(vec![Button::press(&host.tr("More…"), data(key, "more"))]);
    }
    let other = match step.found {
        None => Button::press(&host.tr("Find…"), data(key, "find")),
        Some(_) => Button::press(&host.tr("Search again"), data(key, "again")),
    };
    rows.push(vec![
        other,
        Button::press(&host.tr("No task"), data(key, "none")),
    ]);
    (html, rows)
}

/// A task's card: the project, the key and the title, the status; the tracker's actions and
/// «Discuss» — or, with `line`, that line instead of the buttons.
fn card(
    host: &dyn Host,
    key: u64,
    chat: &Chat,
    task: &Task,
    line: Option<&str>,
) -> (String, Keyboard) {
    let mut html = format!(
        "{}\n<b>{}</b> · {}",
        project_line(chat),
        render::escape(&task.key),
        render::escape(&task.title)
    );
    if !task.status.trim().is_empty() {
        html.push_str(&format!("\n<i>{}</i>", render::escape(task.status.trim())));
    }
    if let Some(line) = line {
        html.push_str(&format!("\n\n{}", render::escape(line)));
        return (html, Keyboard::new());
    }
    let mut rows: Keyboard = task
        .actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            // Plan ones marked, unless their name says it already («Plan»).
            let named = action
                .title
                .to_lowercase()
                .contains(&host.tr("Plan").to_lowercase());
            let text = if action.plan && !named {
                trf(host, "{0} · plan", &[&action.title])
            } else {
                action.title.clone()
            };
            vec![Button::press(&text, data(key, &format!("a{index}"))).primary()]
        })
        .collect();
    rows.push(vec![
        Button::press(&host.tr("Discuss"), data(key, "talk")),
        Button::press(&host.tr("No task"), data(key, "none")),
    ]);
    (html, rows)
}

/// «📁 <b>project</b>».
fn project_line(chat: &Chat) -> String {
    format!("📁 <b>{}</b>", render::escape(&chat.project.name))
}

/// A screen of the step: the message edited into it, or a new one (its id comes back).
fn show(
    bot: &mut Bot,
    host: &mut dyn Host,
    key: u64,
    thread: i64,
    message: Option<i64>,
    html: &str,
    keyboard: &Keyboard,
) {
    let Some(chat) = bot.chat() else {
        return;
    };
    match message {
        Some(message) => {
            let call = telegram::edit_text(chat, message, html, Some(keyboard));
            bot.call(
                host,
                call,
                Lane::Topic(thread),
                Purpose::new(Module::Task, "edit"),
            );
        }
        None => {
            let call = telegram::send_message(chat, Some(thread), html, Some(keyboard));
            let purpose = Purpose::new(Module::Task, format!("shown:{key}"));
            bot.call(host, call, Lane::Topic(thread), purpose);
            conversation::sent_to_topic(bot, host, thread);
        }
    }
}

fn data(key: u64, choice: &str) -> String {
    keys::encode(Kind::Task, key, choice)
}

/// The user's words for the session: none when they were only the task's key.
fn own_words(text: &str, key: &str) -> String {
    if text.trim() == key {
        String::new()
    } else {
        text.trim().to_string()
    }
}

/// The first task key in the text: «PROJ-142» — a capital letter, capitals or digits, a dash,
/// digits, standing apart.
pub fn task_key(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let apart = |index: usize| chars.get(index).is_none_or(|c| !c.is_alphanumeric());
    for start in 0..chars.len() {
        if !chars[start].is_ascii_uppercase() || (start > 0 && !apart(start - 1)) {
            continue;
        }
        let mut end = start + 1;
        while end < chars.len() && (chars[end].is_ascii_uppercase() || chars[end].is_ascii_digit())
        {
            end += 1;
        }
        if end == start + 1 || chars.get(end) != Some(&'-') {
            continue;
        }
        let mut digits = end + 1;
        while digits < chars.len() && chars[digits].is_ascii_digit() {
            digits += 1;
        }
        if digits > end + 1 && apart(digits) {
            return Some(chars[start..digits].iter().collect());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeHost, Telegram, bound_bot, button, buttons, press, user_message};
    use flux_plugin_api::remote::Project;
    use flux_plugin_api::tasks::{TaskAction, TaskStarted};

    fn task(key: &str, title: &str) -> Task {
        Task {
            key: key.into(),
            title: title.into(),
            status: "Open".into(),
            url: None,
            markdown: format!("# {key}\n\nThe text of {key}."),
            actions: vec![
                TaskAction {
                    id: "work".into(),
                    title: "Take to Work".into(),
                    plan: false,
                    start: true,
                },
                TaskAction {
                    id: "plan".into(),
                    title: "Plan".into(),
                    plan: true,
                    start: false,
                },
                TaskAction {
                    id: "dpt".into(),
                    title: "DPT".into(),
                    plan: true,
                    start: true,
                },
            ],
            source: "flux.tracker".into(),
        }
    }

    /// A bot with a project and the tracker's tasks; the user's first words in topic 80, the
    /// project chosen: the step's calls.
    fn at_step(host: &mut FakeHost, tg: &mut Telegram, words: &str) -> (Bot, Vec<(String, Value)>) {
        host.projects = vec![Project {
            path: "/work/flux".into(),
            name: "flux".into(),
            open: true,
        }];
        host.tasks = Some(
            (1..=8)
                .map(|n| task(&format!("PLAY-{n}"), &format!("Task {n}")))
                .collect(),
        );
        let mut bot = bound_bot(host);
        bot.on_update(host, user_message(1, Some(80), words));
        let calls = tg.flush(&mut bot, host);
        let data = button(&calls[0].1, "flux");
        bot.on_update(host, press(2, 101, 80, &data));
        let calls = tg.flush(&mut bot, host);
        (bot, calls)
    }

    fn edits(calls: &[(String, Value)]) -> Vec<Value> {
        calls
            .iter()
            .filter(|(method, _)| method == "editMessageText" || method == "sendMessage")
            .map(|(_, params)| params.clone())
            .collect()
    }

    #[test]
    fn keys_are_found_standing_apart() {
        assert_eq!(task_key("Fix PLAY-12 now"), Some("PLAY-12".into()));
        assert_eq!(task_key("(TRACK2-7)"), Some("TRACK2-7".into()));
        assert_eq!(task_key("aPLAY-1 P-1 PLAY- PLAY-1x"), None);
        assert_eq!(task_key("no key"), None);
    }

    #[test]
    fn no_source_no_step() {
        let mut host = FakeHost::default();
        let mut tg = Telegram::default();
        host.projects = vec![Project {
            path: "/work/flux".into(),
            name: "flux".into(),
            open: true,
        }];
        let mut bot = bound_bot(&mut host);
        bot.on_update(&mut host, user_message(1, Some(80), "Hi"));
        let calls = tg.flush(&mut bot, &mut host);
        let data = button(&calls[0].1, "flux");
        bot.on_update(&mut host, press(2, 101, 80, &data));
        assert_eq!(host.claude.last().unwrap(), "new /work/flux Hi");
    }

    #[test]
    fn a_tracker_that_cant_give_tasks_says_why_and_the_chat_goes_on() {
        let mut host = FakeHost::default();
        let mut tg = Telegram::default();
        host.projects = vec![Project {
            path: "/work/flux".into(),
            name: "flux".into(),
            open: true,
        }];
        host.tasks_error = Some("Sign in to Yandex in the plugin's settings.".into());
        let mut bot = bound_bot(&mut host);
        bot.on_update(&mut host, user_message(1, Some(80), "Hi"));
        let calls = tg.flush(&mut bot, &mut host);
        let data = button(&calls[0].1, "flux");
        bot.on_update(&mut host, press(2, 101, 80, &data));
        assert_eq!(host.claude.last().unwrap(), "new /work/flux Hi");
        let calls = tg.flush(&mut bot, &mut host);
        assert!(calls.iter().any(|(method, params)| method == "sendMessage"
            && params["message_thread_id"] == 80
            && params["text"]
                == "Tasks aren't available: Sign in to Yandex in the plugin's settings."));
    }

    #[test]
    fn my_tasks_a_task_and_its_action_bind_the_topic() {
        let mut host = FakeHost::default();
        let mut tg = Telegram::default();
        let (mut bot, calls) = at_step(&mut host, &mut tg, "Make it faster");
        let step = &edits(&calls)[0];
        assert_eq!(
            step["message_id"], 101,
            "the project's question becomes the step"
        );
        assert_eq!(step["text"], "📁 <b>flux</b>\nStart from a task?");
        assert_eq!(
            buttons(step),
            vec![vec!["My tasks", "Find…"], vec!["No task"]]
        );
        bot.on_update(&mut host, press(3, 101, 80, &button(step, "My tasks")));
        let list = edits(&tg.flush(&mut bot, &mut host))[0].clone();
        assert_eq!(list["text"], "📁 <b>flux</b>\nMy tasks");
        let rows = buttons(&list);
        assert_eq!(rows.len(), 8, "six tasks, «More…», «Find…» and «No task»");
        assert_eq!(rows[0], vec!["PLAY-1 · Task 1"]);
        assert_eq!(rows[6], vec!["More…"]);
        bot.on_update(&mut host, press(4, 101, 80, &button(&list, "More…")));
        let page = edits(&tg.flush(&mut bot, &mut host))[0].clone();
        assert_eq!(buttons(&page)[0], vec!["PLAY-7 · Task 7"]);
        bot.on_update(&mut host, press(5, 101, 80, &button(&page, "PLAY-8")));
        assert_eq!(host.claude.last().unwrap(), "task PLAY-8");
        let card = edits(&tg.flush(&mut bot, &mut host))[0].clone();
        assert_eq!(
            card["text"],
            "📁 <b>flux</b>\n<b>PLAY-8</b> · Task 8\n<i>Open</i>"
        );
        assert_eq!(
            buttons(&card),
            vec![
                vec!["Take to Work".to_string()],
                vec!["Plan".to_string()],
                vec!["DPT · plan".to_string()],
                vec!["Discuss".to_string(), "No task".to_string()],
            ]
        );
        // The action: the line, then the run on its short timer.
        host.run = Some(Ok(TaskStarted {
            session: "s-run".into(),
            notes: vec![
                "The branch PLAY-8 wasn't checked out: local changes are in the way.".into(),
            ],
        }));
        bot.on_update(&mut host, press(6, 101, 80, &button(&card, "Take to Work")));
        let line = host.http.len();
        let calls = tg.flush(&mut bot, &mut host);
        assert!(host.http.len() > line);
        let running = edits(&calls)[0].clone();
        assert_eq!(
            running["text"],
            "📁 <b>flux</b>\n<b>PLAY-8</b> · Task 8\n<i>Open</i>\n\n⏳ «Take to Work» — this takes up to a minute…"
        );
        assert!(
            host.claude
                .contains(&"run PLAY-8 work /work/flux".to_string())
        );
        let topic = bot.store.topic_by_thread(80).unwrap();
        assert_eq!(topic.session, "s-run");
        assert_eq!(topic.title, "PLAY-8: Task 8");
        assert_eq!(topic.head, Some(101));
        let rename = calls
            .iter()
            .find(|(method, _)| method == "editForumTopic")
            .unwrap();
        assert_eq!(rename.1["name"], "PLAY-8: Task 8");
        let texts: Vec<String> = edits(&calls)
            .iter()
            .map(|params| params["text"].as_str().unwrap().to_string())
            .collect();
        assert!(
            texts.contains(&"📁 <b>flux</b> · Ask".to_string()),
            "{texts:?}"
        );
        let note = calls
            .iter()
            .find(|(method, params)| {
                method == "sendMessage" && params["text"].as_str().unwrap().starts_with("⚠️")
            })
            .unwrap();
        assert_eq!(
            note.1["text"],
            "⚠️ The branch PLAY-8 wasn't checked out: local changes are in the way."
        );
        // The words that opened the chat don't follow the action's prompt.
        assert!(
            !host
                .claude
                .iter()
                .any(|line| line.starts_with("send s-run")),
            "{:?}",
            host.claude
        );
    }

    #[test]
    fn a_key_in_the_words_brings_its_card_and_discuss_plans_with_the_task() {
        let mut host = FakeHost::default();
        let mut tg = Telegram::default();
        let (mut bot, calls) = at_step(&mut host, &mut tg, "PLAY-3");
        let card = edits(&calls)[0].clone();
        assert_eq!(
            card["text"],
            "📁 <b>flux</b>\n<b>PLAY-3</b> · Task 3\n<i>Open</i>"
        );
        bot.on_update(&mut host, press(3, 101, 80, &button(&card, "Discuss")));
        let spec = host.started.last().unwrap().clone();
        assert_eq!(spec.project, "/work/flux");
        assert_eq!(spec.mode, Some(Mode::Plan));
        assert_eq!(
            spec.prompt.as_deref(),
            Some("Let's discuss the task PLAY-3."),
            "only the key"
        );
        assert_eq!(spec.attachments[0].0, "PLAY-3");
        assert!(spec.attachments[0].1.contains("The text of PLAY-3"));
        assert_eq!(spec.title.as_deref(), Some("PLAY-3: Task 3"));
        let calls = tg.flush(&mut bot, &mut host);
        assert!(
            edits(&calls)
                .iter()
                .any(|params| params["text"] == "📁 <b>flux</b> · Plan")
        );
        assert_eq!(
            bot.store.topic_by_thread(80).unwrap().title,
            "PLAY-3: Task 3"
        );
    }

    #[test]
    fn find_searches_by_the_next_message() {
        let mut host = FakeHost::default();
        let mut tg = Telegram::default();
        let (mut bot, calls) = at_step(&mut host, &mut tg, "Hi");
        let step = edits(&calls)[0].clone();
        bot.on_update(&mut host, press(3, 101, 80, &button(&step, "Find…")));
        let prompt = edits(&tg.flush(&mut bot, &mut host))[0].clone();
        assert_eq!(prompt["text"], "📁 <b>flux</b>\nWrite a key or words");
        bot.on_update(&mut host, user_message(4, Some(80), "task 5"));
        assert_eq!(host.claude.last().unwrap(), "tasks search task 5");
        let calls = tg.flush(&mut bot, &mut host);
        assert!(
            calls
                .iter()
                .any(|(method, params)| method == "editMessageReplyMarkup"
                    && params["message_id"] == 101),
            "the prompt's buttons go"
        );
        let found = edits(&calls)[0].clone();
        assert_eq!(found["text"], "📁 <b>flux</b>\nFound for «task 5»");
        assert_eq!(
            buttons(&found),
            vec![vec!["PLAY-5 · Task 5"], vec!["Search again", "No task"]]
        );
        // «No task»: the session with the first words.
        bot.on_update(&mut host, press(5, 104, 80, &button(&found, "No task")));
        assert_eq!(host.claude.last().unwrap(), "new /work/flux Hi");
        assert!(bot.store.topic_by_thread(80).is_some());
    }

    #[test]
    fn a_failed_run_says_so_and_offers_to_go_on() {
        let mut host = FakeHost::default();
        let mut tg = Telegram::default();
        let (mut bot, calls) = at_step(&mut host, &mut tg, "PLAY-2 please");
        let card = edits(&calls)[0].clone();
        host.run = Some(Err("Yandex Tracker didn't answer in a minute".into()));
        bot.on_update(&mut host, press(3, 101, 80, &button(&card, "Plan")));
        let calls = tg.flush(&mut bot, &mut host);
        let last = edits(&calls).last().unwrap().clone();
        assert_eq!(
            last["text"],
            "📁 <b>flux</b>\n<b>PLAY-2</b> · Task 2\n<i>Open</i>\n\nCouldn't run «Plan»: Yandex Tracker didn't answer in a minute"
        );
        assert_eq!(buttons(&last), vec![vec!["Discuss", "No task"]]);
        assert!(bot.store.topic_by_thread(80).is_none());
        bot.on_update(&mut host, press(4, 101, 80, &button(&last, "Discuss")));
        let spec = host.started.last().unwrap();
        assert_eq!(spec.prompt.as_deref(), Some("PLAY-2 please"));
        assert_eq!(spec.mode, Some(Mode::Plan));
    }
}
