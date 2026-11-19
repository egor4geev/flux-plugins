//! What a session waits for the user to answer — agent B's: a permission (the card's title, the
//! command or the file, an edit's short diff; «Allow» green, «Deny» red, the request's other
//! choices — «Allow for this session»…), Claude's questions (one at a time, options as buttons,
//! «Other…» — the next message is the answer; several to choose — toggles and «Done»), the plan
//! (folded; «Run, accept edits» / «Run, ask» / «Keep planning» — the next message is the
//! feedback). A press is answered at once, then answers the session
//! ([`Host::answer`](crate::host::Host::answer)) and turns the buttons into the result («✓
//! Allowed»); a request answered at the computer meanwhile ([`on_resolved`]) gets «✓ Answered in
//! Flux», one already gone when pressed — «Already answered in Flux».
//!
//! Where: in the session's topic; a session without one — only while the Mac is locked
//! (`SystemState::screen_locked`, [`on_lock_changed`]), in «Общее» with the session's name and
//! «Продолжить здесь» ([`crate::handoff`]). Words aren't written there («Общее» is the console's):
//! «Other…» and «Keep planning» on its cards continue the session in a topic of its own, and the
//! topic's card waits for the words, as if pressed there (the questions answered in «Общее» go
//! along). The buttons' data names a short key ([`crate::keys`]) this module maps to the card: the
//! session, the request, where it is.

use std::collections::{BTreeMap, BTreeSet};

use flux_plugin_api::remote::{PermissionRequest, Question, Request, RequestKind, Response};
use serde_json::Value;

use crate::bot::Bot;
use crate::conversation;
use crate::handoff;
use crate::host::Host;
use crate::keys::{self, Kind};
use crate::render;
use crate::telegram::{self, Button, CallbackQuery, Keyboard, Message, TgError};
use crate::transport::{Lane, Module, Purpose};

/// An edit's diff on a card, in characters.
const DIFF_CHARS: usize = 2500;
/// A command or a file on a card.
const SUBJECT_CHARS: usize = 1000;
/// A button's text.
const BUTTON_CHARS: usize = 60;

#[derive(Default)]
pub struct State {
    /// The cards on their way or shown, by key.
    cards: BTreeMap<u64, Card>,
    /// The last key given (they start from the clock: a button of the last run means nothing).
    last_key: u64,
    /// What each session waits on, as Flux said (gone when it is resolved).
    waiting: BTreeMap<String, Request>,
    /// Topics whose next message is a card's words («Other…», «Keep planning»): thread → key.
    words: BTreeMap<i64, u64>,
    /// «Other…» or «Keep planning» pressed in «Общее»: the session's topic is on its way, its card
    /// will wait for the words — session → (the request, the prompt).
    words_after_handoff: BTreeMap<String, (String, &'static str)>,
}

/// A request on a card.
struct Card {
    session: String,
    request: Request,
    thread: i64,
    /// In «Общее»: the session's name on top, «Продолжить здесь» under the buttons.
    general: bool,
    /// The card's message, once Telegram took it.
    message: Option<i64>,
    /// Claude's questions: the one shown now, the answers so far, the options picked of several.
    question: usize,
    answers: Vec<Vec<String>>,
    picked: BTreeSet<usize>,
}

pub fn on_request(bot: &mut Bot, host: &mut dyn Host, session: &str, request: &Request) {
    bot.requests
        .waiting
        .insert(session.to_string(), request.clone());
    if let Some(thread) = bot
        .store
        .topic_by_session(session)
        .map(|topic| topic.thread)
    {
        if !shown(bot, session, &request.id, false) {
            post(
                bot,
                host,
                session,
                request,
                thread,
                false,
                Progress::default(),
            );
        }
    } else if bot.system.screen_locked {
        to_general(bot, host, session, request);
    }
}

/// Answered elsewhere (at the computer), or gone.
pub fn on_resolved(bot: &mut Bot, host: &mut dyn Host, session: &str, request: &str) {
    if bot
        .requests
        .waiting
        .get(session)
        .is_some_and(|waiting| waiting.id == request)
    {
        bot.requests.waiting.remove(session);
    }
    let keys: Vec<u64> = bot
        .requests
        .cards
        .iter()
        .filter(|(_, card)| card.session == session && card.request.id == request)
        .map(|(key, _)| *key)
        .collect();
    let result = format!("✓ {}", host.tr("Answered in Flux"));
    for key in keys {
        finish(bot, host, key, &result);
    }
}

pub fn on_press(bot: &mut Bot, host: &mut dyn Host, query: &CallbackQuery, key: u64, choice: &str) {
    // At once (a press waits for its answer); «Done» with nothing picked is answered by the hint.
    let nothing_picked = choice == "ok"
        && bot
            .requests
            .cards
            .get(&key)
            .is_some_and(|card| card.picked.is_empty());
    let hint = nothing_picked.then(|| host.tr("Pick at least one option"));
    bot.answer_press(host, query, hint.as_deref());
    if hint.is_some() {
        return;
    }
    let pressed = query.message.as_ref().map(|message| message.message_id);
    let Some(card) = bot.requests.cards.get(&key) else {
        // A card of the last run, or one answered meanwhile.
        if let (Some(chat), Some(message)) = (bot.chat(), query.message.as_ref()) {
            let gone = host.tr("Already answered in Flux");
            let call = telegram::edit_keyboard(chat, message.message_id, Some(&inert(&gone)));
            bot.call(
                host,
                call,
                Lane::Topic(message.thread().unwrap_or(0)),
                Purpose::new(Module::Requests, "edit"),
            );
        }
        return;
    };
    if card.message.is_some() && card.message != pressed {
        return;
    }
    let (current, picked) = (card.question, card.picked.clone());
    match card.request.kind.clone() {
        RequestKind::Permission(permission) => {
            let (response, result) = match choice {
                "a" => (Response::Allow(None), format!("✓ {}", host.tr("Allowed"))),
                "d" => (
                    Response::Deny(String::new()),
                    format!("✕ {}", host.tr("Denied")),
                ),
                _ => {
                    let Some((id, label)) = choice
                        .strip_prefix('c')
                        .and_then(|index| index.parse::<usize>().ok())
                        .and_then(|index| permission.choices.get(index))
                    else {
                        return;
                    };
                    (Response::Allow(Some(id.clone())), format!("✓ {label}"))
                }
            };
            answer(bot, host, key, &response, &result);
        }
        RequestKind::Questions(questions) => {
            let Some(question) = questions.get(current) else {
                return;
            };
            if choice == "x" {
                ask_words(bot, host, key, "Write the answer in one message.");
            } else if let Some(index) = choice
                .strip_prefix('o')
                .and_then(|index| index.parse::<usize>().ok())
            {
                let Some(option) = question.options.get(index) else {
                    return;
                };
                let label = option.label.clone();
                answered_one(bot, host, key, vec![label]);
            } else if let Some(index) = choice
                .strip_prefix('t')
                .and_then(|index| index.parse::<usize>().ok())
            {
                toggle(bot, host, key, index);
            } else if choice == "ok" {
                let labels: Vec<String> = picked
                    .iter()
                    .filter_map(|index| question.options.get(*index))
                    .map(|option| option.label.clone())
                    .collect();
                answered_one(bot, host, key, labels);
            }
        }
        RequestKind::Plan(_) => {
            let running = format!("▶ {}", host.tr("Running"));
            match choice {
                "p1" => answer(bot, host, key, &Response::ApprovePlan(true), &running),
                "p0" => answer(bot, host, key, &Response::ApprovePlan(false), &running),
                "pk" => ask_words(
                    bot,
                    host,
                    key,
                    "Write what to change in the plan, in one message.",
                ),
                _ => {}
            }
        }
    }
}

/// The user's words for «Other…» or the plan's feedback: true — taken.
pub fn on_user_message(bot: &mut Bot, host: &mut dyn Host, message: &Message) -> bool {
    let Some(thread) = message.thread() else {
        return false;
    };
    let Some(&key) = bot.requests.words.get(&thread) else {
        return false;
    };
    let words = message.words().trim().to_string();
    if words.is_empty() {
        return false;
    }
    bot.requests.words.remove(&thread);
    let Some(card) = bot.requests.cards.get(&key) else {
        return false;
    };
    match &card.request.kind {
        RequestKind::Questions(_) => answered_one(bot, host, key, vec![words]),
        RequestKind::Plan(_) => {
            let result = format!("✎ {}", host.tr("Sent back to planning"));
            answer(bot, host, key, &Response::KeepPlanning(words), &result);
        }
        RequestKind::Permission(_) => return false,
    }
    true
}

/// The Mac was locked or unlocked (`bot.system.screen_locked`): the waiting requests of sessions
/// without a topic go to «Общее» (and stay there when it is unlocked — answered at the computer
/// they turn into the result).
pub fn on_lock_changed(bot: &mut Bot, host: &mut dyn Host) {
    if !bot.system.screen_locked {
        return;
    }
    let mut waiting: Vec<(String, Request)> = bot
        .requests
        .waiting
        .iter()
        .map(|(session, request)| (session.clone(), request.clone()))
        .collect();
    for session in bot.sessions.values() {
        if let Some(request) = &session.request
            && !waiting.iter().any(|(id, _)| *id == session.id)
        {
            waiting.push((session.id.clone(), request.clone()));
        }
    }
    for (session, request) in waiting {
        if bot.store.topic_by_session(&session).is_none() {
            to_general(bot, host, &session, &request);
        }
    }
}

/// The session's waiting request again — in its topic now ([`crate::handoff`]); the copy in
/// «Общее» says so.
pub fn repost(bot: &mut Bot, host: &mut dyn Host, session: &str) {
    let Some(thread) = bot
        .store
        .topic_by_session(session)
        .map(|topic| topic.thread)
    else {
        return;
    };
    let copies: Vec<u64> = bot
        .requests
        .cards
        .iter()
        .filter(|(_, card)| card.session == session && card.general)
        .map(|(key, _)| *key)
        .collect();
    // The questions answered in «Общее» go along.
    let progress = copies
        .iter()
        .filter_map(|key| bot.requests.cards.get(key))
        .map(|card| {
            (
                card.request.id.clone(),
                Progress {
                    question: card.question,
                    answers: card.answers.clone(),
                },
            )
        })
        .next();
    let result = format!("→ {}", host.tr("Continued in the topic"));
    for key in copies {
        finish(bot, host, key, &result);
    }
    let request = bot.requests.waiting.get(session).cloned().or_else(|| {
        bot.sessions
            .get(session)
            .and_then(|session| session.request.clone())
    });
    let Some(request) = request else {
        bot.requests.words_after_handoff.remove(session);
        return;
    };
    if !shown(bot, session, &request.id, false) {
        let progress = progress
            .filter(|(id, _)| *id == request.id)
            .map(|(_, progress)| progress)
            .unwrap_or_default();
        post(bot, host, session, &request, thread, false, progress);
    }
    // «Other…» or «Keep planning» pressed in «Общее»: the topic's card waits for the words.
    if let Some((id, prompt)) = bot.requests.words_after_handoff.remove(session)
        && id == request.id
        && let Some(key) = topic_card(bot, session, &id)
    {
        ask_words(bot, host, key, prompt);
    }
}

/// The card of a session's request in its topic.
fn topic_card(bot: &Bot, session: &str, request: &str) -> Option<u64> {
    bot.requests
        .cards
        .iter()
        .find(|(_, card)| card.session == session && card.request.id == request && !card.general)
        .map(|(key, _)| *key)
}

pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    let (kind, key) = tag.split_once(':').unwrap_or((tag, ""));
    let key = key.parse::<u64>().ok();
    match (kind, key, result) {
        ("card" | "plan", Some(key), Ok(value)) => {
            if let (Ok(message), Some(card)) = (
                telegram::result::<Message>(value),
                bot.requests.cards.get_mut(&key),
            ) {
                card.message = Some(message.message_id);
            }
        }
        ("plan", Some(key), Err(err)) if !err.topic_gone() => {
            // No rich message: the plan as HTML.
            host.log(&format!(
                "The plan's rich message was refused ({err:?}): HTML instead"
            ));
            send_card(bot, host, key, false);
        }
        ("card" | "plan", Some(key), Err(err)) => {
            host.log(&format!("A request's card failed: {err:?}"));
            bot.requests.cards.remove(&key);
            bot.requests.words.retain(|_, waiting| *waiting != key);
        }
        (_, _, Err(err)) if !err.not_modified() => {
            host.log(&format!("A request's edit failed: {err:?}"));
        }
        _ => {}
    }
}

// --- Cards ---

/// Is the request on a card already (in «Общее», or in a topic)?
fn shown(bot: &Bot, session: &str, request: &str, general: bool) -> bool {
    bot.requests.cards.values().any(|card| {
        card.session == session && card.request.id == request && card.general == general
    })
}

/// A request of a session without a topic, while the Mac is locked: in «Общее».
fn to_general(bot: &mut Bot, host: &mut dyn Host, session: &str, request: &Request) {
    let Some(general) = bot.store.general else {
        return;
    };
    if !shown(bot, session, &request.id, true) {
        post(
            bot,
            host,
            session,
            request,
            general,
            true,
            Progress::default(),
        );
    }
}

/// Claude's questions answered so far: the one shown now, the answers before it.
#[derive(Default)]
struct Progress {
    question: usize,
    answers: Vec<Vec<String>>,
}

fn post(
    bot: &mut Bot,
    host: &mut dyn Host,
    session: &str,
    request: &Request,
    thread: i64,
    general: bool,
    progress: Progress,
) {
    bot.requests.last_key = (bot.requests.last_key + 1).max(host.now_ms() % 1_000_000_000_000);
    let key = bot.requests.last_key;
    bot.requests.cards.insert(
        key,
        Card {
            session: session.to_string(),
            request: request.clone(),
            thread,
            general,
            message: None,
            question: progress.question,
            answers: progress.answers,
            picked: BTreeSet::new(),
        },
    );
    send_card(bot, host, key, true);
}

/// Sends a card's message (the plan as a rich message unless `rich` is off).
fn send_card(bot: &mut Bot, host: &mut dyn Host, key: u64, rich: bool) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let Some((session, general)) = bot
        .requests
        .cards
        .get(&key)
        .map(|card| (card.session.clone(), card.general))
    else {
        return;
    };
    let handoff = general.then(|| handoff::key_for(&session));
    let Some(card) = bot.requests.cards.get(&key) else {
        return;
    };
    let thread = card.thread;
    let mut keyboard = card_keyboard(host, key, card);
    if let Some(handoff) = handoff {
        let here = host.tr("Continue here");
        keyboard.push(vec![Button::press(
            &here,
            keys::encode(Kind::Handoff, handoff, ""),
        )]);
    }
    let (call, tag) = match &card.request.kind {
        RequestKind::Plan(plan) if rich => {
            let header = if card.general {
                let (title, project) = session_names(bot, host, &card.session);
                format!("**{title}** · {project}\n\n")
            } else {
                String::new()
            };
            let markdown = format!(
                "{header}**{}**\n\n<details><summary>{}</summary>\n\n{}\n\n</details>",
                host.tr("The plan is ready"),
                host.tr("Plan"),
                plan.trim()
            );
            let markdown = render::rich_markdown(&markdown);
            (
                telegram::send_rich(chat, Some(thread), &markdown, Some(&keyboard)),
                "plan",
            )
        }
        _ => {
            let html = card_html(bot, host, card);
            (
                telegram::send_message(chat, Some(thread), &html, Some(&keyboard)),
                "card",
            )
        }
    };
    bot.call(
        host,
        call,
        Lane::Topic(thread),
        Purpose::new(Module::Requests, format!("{tag}:{key}")),
    );
    if !general {
        conversation::sent_to_topic(bot, host, thread);
    }
}

/// The session's name on a card in «Общее»: «<b>Fix the parser</b> · project».
fn header_html(bot: &Bot, host: &dyn Host, card: &Card) -> String {
    if !card.general {
        return String::new();
    }
    let (title, project) = session_names(bot, host, &card.session);
    format!(
        "<b>{}</b> · {}\n",
        render::escape(&title),
        render::escape(&project)
    )
}

/// The session's title (or «Untitled session») and its project's folder name.
fn session_names(bot: &Bot, host: &dyn Host, session: &str) -> (String, String) {
    let known = bot.sessions.get(session);
    let title = known
        .map(|session| session.title.trim().to_string())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| host.tr("Untitled session"));
    let project = known
        .map(|session| session.project.trim_end_matches(['/', '\\']))
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .unwrap_or_default()
        .to_string();
    (title, project)
}

fn card_html(bot: &Bot, host: &dyn Host, card: &Card) -> String {
    let mut html = header_html(bot, host, card);
    match &card.request.kind {
        RequestKind::Permission(permission) => html.push_str(&permission_html(permission)),
        RequestKind::Questions(questions) => {
            if let Some(question) = questions.get(card.question) {
                html.push_str(&question_html(
                    host,
                    question,
                    card.question,
                    questions.len(),
                ));
            }
        }
        RequestKind::Plan(plan) => {
            // Without rich messages: the plan as a folded quote.
            html.push_str(&format!(
                "<b>{}</b>\n<blockquote expandable>{}</blockquote>",
                render::escape(&host.tr("The plan is ready")),
                render::escape(&render::cut(plan.trim(), render::MESSAGE_LIMIT - 400))
            ));
        }
    }
    html
}

/// «<b>Claude wants to run a command</b>», the command, an edit's diff.
fn permission_html(permission: &PermissionRequest) -> String {
    let title = permission.title.trim();
    let mut html = format!("<b>{}</b>", render::escape(title));
    // «Изменить greeting.txt» names its file already.
    if let Some(subject) = permission
        .subject
        .as_deref()
        .map(str::trim)
        .filter(|subject| !subject.is_empty() && !title.contains(subject))
    {
        html.push_str(&format!(
            "\n<code>{}</code>",
            render::escape(&render::cut(subject, SUBJECT_CHARS))
        ));
    }
    if let Some(diff) = permission
        .diff
        .as_deref()
        .filter(|diff| !diff.trim().is_empty())
    {
        html.push_str(&format!(
            "\n<pre><code class=\"language-diff\">{}</code></pre>",
            render::escape(&cut_lines(diff.trim_end(), DIFF_CHARS))
        ));
    }
    html
}

/// «<b>Claude asks</b> · 1/2», the question, a line per option with its description.
fn question_html(host: &dyn Host, question: &Question, index: usize, count: usize) -> String {
    let mut html = format!("<b>{}</b>", render::escape(&host.tr("Claude asks")));
    if count > 1 {
        html.push_str(&format!(" · {}/{count}", index + 1));
    }
    html.push('\n');
    html.push_str(&render::escape(question.question.trim()));
    if question
        .options
        .iter()
        .any(|option| !option.description.trim().is_empty())
    {
        html.push('\n');
        for (number, option) in question.options.iter().enumerate() {
            html.push_str(&format!(
                "\n<b>{}</b> · {}",
                number + 1,
                render::escape(option.label.trim())
            ));
            if !option.description.trim().is_empty() {
                html.push_str(&format!(
                    " — <i>{}</i>",
                    render::escape(option.description.trim())
                ));
            }
        }
    }
    html
}

fn card_keyboard(host: &dyn Host, key: u64, card: &Card) -> Keyboard {
    let data = |choice: &str| keys::encode(Kind::Request, key, choice);
    let mut rows: Keyboard = Vec::new();
    match &card.request.kind {
        RequestKind::Permission(permission) => {
            rows.push(vec![
                Button::press(&host.tr("Allow"), data("a")).green(),
                Button::press(&host.tr("Deny"), data("d")).red(),
            ]);
            for (index, (_, label)) in permission.choices.iter().enumerate() {
                rows.push(vec![Button::press(
                    &render::cut(label, BUTTON_CHARS),
                    data(&format!("c{index}")),
                )]);
            }
        }
        RequestKind::Questions(questions) => {
            let Some(question) = questions.get(card.question) else {
                return rows;
            };
            for (index, option) in question.options.iter().enumerate() {
                let label = render::cut(option.label.trim(), BUTTON_CHARS);
                rows.push(vec![if question.multi_select {
                    let mark = if card.picked.contains(&index) {
                        "☑"
                    } else {
                        "☐"
                    };
                    Button::press(&format!("{mark} {label}"), data(&format!("t{index}")))
                } else {
                    Button::press(
                        &format!("{} · {label}", index + 1),
                        data(&format!("o{index}")),
                    )
                }]);
            }
            if question.multi_select {
                rows.push(vec![Button::press(&host.tr("Done"), data("ok")).primary()]);
            }
            rows.push(vec![Button::press(&host.tr("Other…"), data("x"))]);
        }
        RequestKind::Plan(_) => {
            rows.push(vec![
                Button::press(&host.tr("Run, accept edits"), data("p1")).primary(),
            ]);
            rows.push(vec![Button::press(&host.tr("Run, ask"), data("p0"))]);
            rows.push(vec![Button::press(&host.tr("Keep planning"), data("pk"))]);
        }
    }
    rows
}

/// One button that only shows the result.
fn inert(text: &str) -> Keyboard {
    vec![vec![Button::press(
        &render::cut(text, BUTTON_CHARS),
        keys::noop(),
    )]]
}

/// At most `max` characters, cut on a line's border.
fn cut_lines(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for line in text.lines() {
        if out.chars().count() + line.chars().count() + 1 > max {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    if out.is_empty() {
        out = render::cut(text, max);
        out.push('\n');
    }
    out.push('…');
    out
}

// --- Answers ---

/// Answers the session; the card's buttons become the result (or «Already answered in Flux»).
fn answer(bot: &mut Bot, host: &mut dyn Host, key: u64, response: &Response, result: &str) {
    let Some(card) = bot.requests.cards.get(&key) else {
        return;
    };
    let (session, request) = (card.session.clone(), card.request.id.clone());
    match host.answer(&session, &request, response) {
        Ok(()) => {
            if bot
                .requests
                .waiting
                .get(&session)
                .is_some_and(|waiting| waiting.id == request)
            {
                bot.requests.waiting.remove(&session);
            }
            finish(bot, host, key, result);
            // The same request's other card (the copy in «Общее», or the topic's).
            let others: Vec<u64> = bot
                .requests
                .cards
                .iter()
                .filter(|(_, card)| card.session == session && card.request.id == request)
                .map(|(key, _)| *key)
                .collect();
            for other in others {
                finish(bot, host, other, result);
            }
        }
        Err(err) => {
            host.log(&format!("Couldn't answer {request}: {err}"));
            let gone = host.tr("Already answered in Flux");
            finish(bot, host, key, &gone);
        }
    }
}

/// The card is done: its buttons become one that shows `result`.
fn finish(bot: &mut Bot, host: &mut dyn Host, key: u64, result: &str) {
    let Some(card) = bot.requests.cards.remove(&key) else {
        return;
    };
    bot.requests.words.retain(|_, waiting| *waiting != key);
    let (Some(chat), Some(message)) = (bot.chat(), card.message) else {
        return;
    };
    let call = telegram::edit_keyboard(chat, message, Some(&inert(result)));
    bot.call(
        host,
        call,
        Lane::Topic(card.thread),
        Purpose::new(Module::Requests, "edit"),
    );
}

/// A question answered: the next one on a new card, or all the answers to the session.
fn answered_one(bot: &mut Bot, host: &mut dyn Host, key: u64, labels: Vec<String>) {
    let chat = bot.chat();
    let Some(card) = bot.requests.cards.get_mut(&key) else {
        return;
    };
    let shown = format!("✓ {}", labels.join(", "));
    card.answers.push(labels);
    card.question += 1;
    card.picked.clear();
    let count = match &card.request.kind {
        RequestKind::Questions(questions) => questions.len(),
        _ => 0,
    };
    if card.question < count {
        // This card shows its answer; the next question comes on a new one.
        if let (Some(chat), Some(message)) = (chat, card.message.take()) {
            let thread = card.thread;
            let call = telegram::edit_keyboard(chat, message, Some(&inert(&shown)));
            bot.call(
                host,
                call,
                Lane::Topic(thread),
                Purpose::new(Module::Requests, "edit"),
            );
        }
        bot.requests.words.retain(|_, waiting| *waiting != key);
        send_card(bot, host, key, true);
        return;
    }
    let answers = std::mem::take(&mut card.answers);
    answer(bot, host, key, &Response::Answers(answers), &shown);
}

/// One of several options picked or unpicked: the buttons change in place.
fn toggle(bot: &mut Bot, host: &mut dyn Host, key: u64, index: usize) {
    let chat = bot.chat();
    let Some(card) = bot.requests.cards.get_mut(&key) else {
        return;
    };
    if !card.picked.remove(&index) {
        card.picked.insert(index);
    }
    let (Some(chat), Some(message)) = (chat, card.message) else {
        return;
    };
    let thread = card.thread;
    let keyboard = card_keyboard(host, key, card);
    let call = telegram::edit_keyboard(chat, message, Some(&keyboard));
    bot.call(
        host,
        call,
        Lane::Topic(thread),
        Purpose::new(Module::Requests, "edit"),
    );
}

/// «Other…», «Keep planning»: the topic's next message is the card's words. Pressed in «Общее» —
/// the session continues in its topic first ([`handoff::continue_in_telegram`]), whose card then
/// waits for them ([`repost`]).
fn ask_words(bot: &mut Bot, host: &mut dyn Host, key: u64, prompt: &'static str) {
    let Some(card) = bot.requests.cards.get(&key) else {
        return;
    };
    if card.general {
        let (session, request) = (card.session.clone(), card.request.id.clone());
        if bot.store.topic_by_session(&session).is_none() {
            bot.requests
                .words_after_handoff
                .insert(session.clone(), (request, prompt));
            handoff::continue_in_telegram(bot, host, &session);
            return;
        }
        // The session has its topic already: its card waits for the words there.
        bot.requests
            .words_after_handoff
            .insert(session.clone(), (request.clone(), prompt));
        match topic_card(bot, &session, &request) {
            Some(topic_key) => {
                bot.requests.words_after_handoff.remove(&session);
                let result = format!("→ {}", host.tr("Continued in the topic"));
                finish(bot, host, key, &result);
                ask_words(bot, host, topic_key, prompt);
            }
            None => repost(bot, host, &session),
        }
        return;
    }
    let thread = card.thread;
    bot.requests.words.insert(thread, key);
    let Some(chat) = bot.chat() else {
        return;
    };
    let text = render::escape(&host.tr(prompt));
    let call = telegram::send_message(chat, Some(thread), &text, None);
    bot.call(host, call, Lane::Topic(thread), Purpose::none());
    conversation::sent_to_topic(bot, host, thread);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::ClaudeEvent;
    use crate::testing::{
        FakeHost, GENERAL, Telegram, bot_with_topic, press, session, user_message,
    };
    use flux_plugin_api::remote::{QuestionOption, SessionState, SystemEvent};

    fn permission() -> Request {
        Request {
            id: "r1".into(),
            kind: RequestKind::Permission(PermissionRequest {
                title: "Claude wants to run a command".into(),
                tool: "Bash".into(),
                subject: Some("cargo test".into()),
                diff: None,
                choices: vec![("session".into(), "Allow for this session".into())],
            }),
        }
    }

    fn question(text: &str, labels: &[&str], multi_select: bool) -> Question {
        Question {
            question: text.into(),
            header: "Q".into(),
            options: labels
                .iter()
                .map(|label| QuestionOption {
                    label: (*label).into(),
                    description: format!("About {label}"),
                })
                .collect(),
            multi_select,
        }
    }

    fn request(id: &str, kind: RequestKind) -> ClaudeEvent {
        ClaudeEvent::Request(
            "s1".into(),
            Request {
                id: id.into(),
                kind,
            },
        )
    }

    /// The data of the button whose text has `text`, on the call.
    fn button(call: &Value, text: &str) -> String {
        call["reply_markup"]["inline_keyboard"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row.as_array().unwrap())
            .find(|button| button["text"].as_str().unwrap().contains(text))
            .unwrap_or_else(|| panic!("no «{text}» in {call}"))["callback_data"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn inert_text(call: &Value) -> &str {
        call["reply_markup"]["inline_keyboard"][0][0]["text"]
            .as_str()
            .unwrap()
    }

    #[test]
    fn a_permission_is_answered_by_a_press() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_claude(&mut host, ClaudeEvent::Request("s1".into(), permission()));
        let calls = tg.flush(&mut bot, &mut host);
        let card = &calls[0].1;
        assert_eq!(
            card["text"],
            "<b>Claude wants to run a command</b>\n<code>cargo test</code>"
        );
        assert_eq!(card["message_thread_id"], 77);
        let rows = &card["reply_markup"]["inline_keyboard"];
        assert_eq!(rows[0][0]["text"], "Allow");
        assert_eq!(rows[0][0]["style"], "success");
        assert_eq!(rows[0][1]["style"], "danger");
        assert_eq!(rows[1][0]["text"], "Allow for this session");
        bot.on_update(&mut host, press(2, 101, 77, &button(card, "Allow for")));
        assert_eq!(
            host.claude.last().unwrap(),
            "answer s1 r1 Response::Allow(Some(\"session\"))"
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].0, "answerCallbackQuery", "answered at once");
        assert_eq!(calls[1].0, "editMessageReplyMarkup");
        assert_eq!(inert_text(&calls[1].1), "✓ Allow for this session");
        // Flux tells it went: nothing more.
        bot.on_claude(&mut host, ClaudeEvent::Resolved("s1".into(), "r1".into()));
        assert!(tg.flush(&mut bot, &mut host).is_empty());
    }

    #[test]
    fn answered_at_the_computer_or_gone_before_the_press() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_claude(&mut host, ClaudeEvent::Request("s1".into(), permission()));
        let card = tg.flush(&mut bot, &mut host)[0].1.clone();
        bot.on_claude(&mut host, ClaudeEvent::Resolved("s1".into(), "r1".into()));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(inert_text(&calls[0].1), "✓ Answered in Flux");
        // A press that comes after: the request is gone.
        bot.on_update(&mut host, press(3, 101, 77, &button(&card, "Deny")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].0, "answerCallbackQuery");
        assert_eq!(inert_text(&calls[1].1), "Already answered in Flux");
        // Flux refuses the answer: gone meanwhile.
        let request = Request {
            id: "r2".into(),
            ..permission()
        };
        bot.on_claude(&mut host, ClaudeEvent::Request("s1".into(), request));
        let card = tg.flush(&mut bot, &mut host)[0].1.clone();
        host.fail = Some("No such request".into());
        bot.on_update(&mut host, press(4, 102, 77, &button(&card, "Deny")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(inert_text(&calls[1].1), "Already answered in Flux");
    }

    #[test]
    fn questions_come_one_at_a_time_and_other_takes_the_next_message() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        let questions = vec![
            question("What about the old login?", &["Keep", "Remove"], false),
            question("Where?", &["Here", "There"], false),
        ];
        bot.on_claude(&mut host, request("q1", RequestKind::Questions(questions)));
        let card = tg.flush(&mut bot, &mut host)[0].1.clone();
        assert_eq!(
            card["text"],
            "<b>Claude asks</b> · 1/2\nWhat about the old login?\n\n<b>1</b> · Keep — <i>About Keep</i>\n<b>2</b> · Remove — <i>About Remove</i>"
        );
        assert_eq!(
            card["reply_markup"]["inline_keyboard"][1][0]["text"],
            "2 · Remove"
        );
        bot.on_update(&mut host, press(2, 101, 77, &button(&card, "Remove")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(inert_text(&calls[1].1), "✓ Remove");
        let second = calls[2].1.clone();
        assert!(second["text"].as_str().unwrap().contains("2/2\nWhere?"));
        bot.on_update(&mut host, press(3, 102, 77, &button(&second, "Other")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[1].1["text"], "Write the answer in one message.");
        bot.on_update(&mut host, user_message(4, Some(77), "Behind a flag"));
        assert_eq!(
            host.claude.last().unwrap(),
            "answer s1 q1 Response::Answers([[\"Remove\"], [\"Behind a flag\"]])"
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(inert_text(&calls[0].1), "✓ Behind a flag");
        // The next message is the session's again.
        bot.on_update(&mut host, user_message(5, Some(77), "Go on"));
        assert_eq!(host.claude.last().unwrap(), "send s1 Go on");
    }

    #[test]
    fn several_to_choose_are_toggles_and_done() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        let questions = vec![question("Which?", &["A", "B", "C"], true)];
        bot.on_claude(&mut host, request("q2", RequestKind::Questions(questions)));
        let card = tg.flush(&mut bot, &mut host)[0].1.clone();
        assert_eq!(card["reply_markup"]["inline_keyboard"][0][0]["text"], "☐ A");
        bot.on_update(&mut host, press(2, 101, 77, &button(&card, "Done")));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].1["text"], "Pick at least one option");
        bot.on_update(&mut host, press(3, 101, 77, &button(&card, "A")));
        bot.on_update(&mut host, press(4, 101, 77, &button(&card, "C")));
        let calls = tg.flush(&mut bot, &mut host);
        let last = calls
            .iter()
            .rev()
            .find(|(method, _)| method == "editMessageReplyMarkup")
            .unwrap();
        assert_eq!(
            last.1["reply_markup"]["inline_keyboard"][0][0]["text"],
            "☑ A"
        );
        assert_eq!(
            last.1["reply_markup"]["inline_keyboard"][2][0]["text"],
            "☑ C"
        );
        bot.on_update(&mut host, press(5, 101, 77, &button(&card, "Done")));
        assert_eq!(
            host.claude.last().unwrap(),
            "answer s1 q2 Response::Answers([[\"A\", \"C\"]])"
        );
    }

    #[test]
    fn the_plan_is_folded_then_run_or_planned_further() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_claude(
            &mut host,
            request("p1", RequestKind::Plan("1. Do it\n2. Test it".into())),
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].0, "sendRichMessage");
        assert_eq!(
            calls[0].1["rich_message"]["markdown"],
            "**The plan is ready**\n\n<details><summary>Plan</summary>\n\n1. Do it\n2. Test it\n\n</details>"
        );
        let card = calls[0].1.clone();
        assert_eq!(
            card["reply_markup"]["inline_keyboard"][0][0]["style"],
            "primary"
        );
        bot.on_update(&mut host, press(2, 101, 77, &button(&card, "accept edits")));
        assert_eq!(
            host.claude.last().unwrap(),
            "answer s1 p1 Response::ApprovePlan(true)"
        );
        assert_eq!(inert_text(&tg.flush(&mut bot, &mut host)[1].1), "▶ Running");
        // Another plan: «Keep planning» and the words.
        bot.on_claude(&mut host, request("p2", RequestKind::Plan("Plan B".into())));
        let card = tg.flush(&mut bot, &mut host)[0].1.clone();
        bot.on_update(
            &mut host,
            press(3, 102, 77, &button(&card, "Keep planning")),
        );
        bot.on_update(&mut host, user_message(4, Some(77), "Shorter, please"));
        assert_eq!(
            host.claude.last().unwrap(),
            "answer s1 p2 Response::KeepPlanning(\"Shorter, please\")"
        );
        // No rich messages: the plan as HTML.
        bot.on_claude(&mut host, request("p3", RequestKind::Plan("Plan C".into())));
        tg.flush(&mut bot, &mut host);
        assert_eq!(tg.flush(&mut bot, &mut host).len(), 0);
        bot.on_claude(&mut host, request("p4", RequestKind::Plan("Plan D".into())));
        assert_eq!(
            tg.refuse(&mut bot, &mut host, "Bad Request: unknown method"),
            "sendRichMessage"
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].0, "sendMessage");
        assert!(
            calls[0].1["text"]
                .as_str()
                .unwrap()
                .contains("<blockquote expandable>Plan D</blockquote>")
        );
    }

    #[test]
    fn while_the_mac_is_locked_requests_without_a_topic_come_to_general() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        let mut other = session("s2", "/work/project", SessionState::Waiting);
        other.title = "Migrations".into();
        bot.sessions.insert("s2".into(), other);
        let edit = Request {
            id: "r9".into(),
            ..permission()
        };
        bot.on_claude(&mut host, ClaudeEvent::Request("s2".into(), edit.clone()));
        assert!(
            tg.flush(&mut bot, &mut host).is_empty(),
            "unlocked: Flux asks at the computer"
        );
        bot.on_claude(&mut host, ClaudeEvent::System(SystemEvent::ScreenLocked));
        let calls = tg.flush(&mut bot, &mut host);
        let card = calls
            .iter()
            .find(|(method, _)| method == "sendMessage")
            .unwrap()
            .1
            .clone();
        assert_eq!(card["message_thread_id"], GENERAL);
        assert_eq!(
            card["text"],
            "<b>Migrations</b> · project\n<b>Claude wants to run a command</b>\n<code>cargo test</code>"
        );
        let rows = card["reply_markup"]["inline_keyboard"].as_array().unwrap();
        assert_eq!(rows.last().unwrap()[0]["text"], "Continue here");
        assert!(
            rows.last().unwrap()[0]["callback_data"]
                .as_str()
                .unwrap()
                .starts_with("h:")
        );
        // Another request while locked comes at once; the press answers it.
        bot.on_claude(&mut host, ClaudeEvent::System(SystemEvent::ScreenUnlocked));
        bot.on_claude(&mut host, ClaudeEvent::System(SystemEvent::ScreenLocked));
        assert!(
            tg.flush(&mut bot, &mut host)
                .iter()
                .all(|(method, _)| method != "sendMessage"),
            "no second copy"
        );
        bot.on_update(&mut host, press(5, 101, GENERAL, &button(&card, "Allow")));
        assert_eq!(
            host.claude.last().unwrap(),
            "answer s2 r9 Response::Allow(None)"
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(inert_text(&calls[1].1), "✓ Allowed");
    }

    #[test]
    fn other_and_keep_planning_in_general_continue_in_the_topic() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        let mut other = session("s2", "/work/project", SessionState::Waiting);
        other.title = "Migrations".into();
        bot.sessions.insert("s2".into(), other);
        let questions = vec![
            question("What about the old login?", &["Keep", "Remove"], false),
            question("Where?", &["Here", "There"], false),
        ];
        bot.on_claude(
            &mut host,
            ClaudeEvent::Request(
                "s2".into(),
                Request {
                    id: "q1".into(),
                    kind: RequestKind::Questions(questions),
                },
            ),
        );
        bot.on_claude(&mut host, ClaudeEvent::System(SystemEvent::ScreenLocked));
        let calls = tg.flush(&mut bot, &mut host);
        let card = calls
            .iter()
            .find(|(method, _)| method == "sendMessage")
            .unwrap()
            .1
            .clone();
        assert_eq!(card["message_thread_id"], GENERAL);
        bot.on_update(&mut host, press(2, 101, GENERAL, &button(&card, "Keep")));
        let calls = tg.flush(&mut bot, &mut host);
        let second = calls
            .iter()
            .find(|(method, _)| method == "sendMessage")
            .unwrap()
            .1
            .clone();
        assert!(second["text"].as_str().unwrap().contains("2/2\nWhere?"));
        // «Other…» in «Общее»: the session gets its topic, the card waits for the words there.
        let shown_in_general = |bot: &Bot| {
            bot.requests
                .cards
                .values()
                .find(|card| card.general)
                .and_then(|card| card.message)
                .unwrap()
        };
        let pressed = shown_in_general(&bot);
        bot.on_update(
            &mut host,
            press(3, pressed, GENERAL, &button(&second, "Other")),
        );
        let calls = tg.flush(&mut bot, &mut host);
        let topic = bot.store.topic_by_session("s2").unwrap().thread;
        assert_eq!(topic, 800);
        let in_topic: Vec<&Value> = calls
            .iter()
            .filter(|(_, params)| params["message_thread_id"] == topic)
            .map(|(_, params)| params)
            .collect();
        let texts: Vec<&str> = in_topic
            .iter()
            .map(|params| params["text"].as_str().unwrap_or_default())
            .collect();
        assert!(
            texts.iter().any(|text| text.contains("2/2\nWhere?")),
            "the question goes on where it was: {texts:?}"
        );
        assert_eq!(texts.last().unwrap(), &"Write the answer in one message.");
        assert!(
            calls
                .iter()
                .any(|(method, params)| method == "editMessageReplyMarkup"
                    && inert_text(params) == "→ Continued in the topic")
        );
        bot.on_update(&mut host, user_message(4, Some(topic), "Behind a flag"));
        assert_eq!(
            host.claude.last().unwrap(),
            "answer s2 q1 Response::Answers([[\"Keep\"], [\"Behind a flag\"]])"
        );
        // The plan's «Keep planning» the same way (the session has its topic now).
        bot.on_claude(
            &mut host,
            ClaudeEvent::Request(
                "s3".into(),
                Request {
                    id: "p1".into(),
                    kind: RequestKind::Plan("1. Do it".into()),
                },
            ),
        );
        bot.sessions.insert(
            "s3".into(),
            session("s3", "/work/api", SessionState::Waiting),
        );
        let calls = tg.flush(&mut bot, &mut host);
        let plan = calls
            .iter()
            .find(|(_, params)| params["message_thread_id"] == GENERAL)
            .unwrap()
            .1
            .clone();
        bot.on_update(
            &mut host,
            press(
                5,
                shown_in_general(&bot),
                GENERAL,
                &button(&plan, "Keep planning"),
            ),
        );
        let calls = tg.flush(&mut bot, &mut host);
        let topic = bot.store.topic_by_session("s3").unwrap().thread;
        assert_eq!(
            calls
                .iter()
                .rfind(|(_, params)| params["message_thread_id"] == topic)
                .unwrap()
                .1["text"],
            "Write what to change in the plan, in one message."
        );
        bot.on_update(&mut host, user_message(6, Some(topic), "Shorter"));
        assert_eq!(
            host.claude.last().unwrap(),
            "answer s3 p1 Response::KeepPlanning(\"Shorter\")"
        );
    }

    #[test]
    fn repost_brings_the_request_into_the_new_topic() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.system.screen_locked = true;
        bot.on_claude(&mut host, ClaudeEvent::Request("s2".into(), permission()));
        tg.flush(&mut bot, &mut host);
        bot.store.add_topic(crate::store::Topic {
            thread: 88,
            session: "s2".into(),
            ..Default::default()
        });
        repost(&mut bot, &mut host, "s2");
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(inert_text(&calls[0].1), "→ Continued in the topic");
        assert_eq!(calls[1].1["message_thread_id"], 88);
        assert!(
            calls[1].1["text"]
                .as_str()
                .unwrap()
                .starts_with("<b>Claude wants")
        );
    }
}
