//! A session's topic both ways — agent B's.
//!
//! - The user's message → the session ([`Host::send`](crate::host::Host::send)); an unloaded one
//!   resumes, a working one queues it (Flux does both). Pictures and files — part 3: a calm note.
//! - Claude's message as it is written ([`on_text`], the whole text each time) → a draft in the
//!   topic with «Stop» ([`crate::telegram::send_draft`], lane [`Lane::Draft`]: one request in
//!   flight, the newest text next, at most every [`DRAFT_EVERY_MS`]); when it is written to the end
//!   — the message itself: a rich message with Claude's Markdown, or HTML messages cut on a line's
//!   border when Telegram refuses the rich one.
//! - While Claude works without writing (steps, thinking): the empty draft — Telegram shows
//!   «Thinking…». A draft lives about 30 s, so a draft that waits is sent again every
//!   [`REFRESH_MS`] — by one timer that runs only while a draft is shown.
//! - A group of steps ([`on_steps`]) → one folded block when the group ends («Read 4 files ·
//!   searched env_override», a line per step inside).
//! - The turn's end ([`on_turn_end`]) → «✅ Done · 3 files · +12 −4» (part 3 adds the diff as a
//!   file and the rollback), «⚠️ The turn broke off» when it failed; an error ([`on_failed`]) →
//!   «⚠️ …».
//! - «Stop» under a draft ([`on_stop`], found by the draft's id — the topic isn't always named) →
//!   the session is interrupted and what was shown is sent as a message with «⏹ Stopped»
//!   (Telegram drops the draft's text); the rest of that message is dropped.
//! - A session closed in Flux ([`on_gone`]) → a note, and the topic is let go: a new message there
//!   starts a new chat.
//!
//! Any message the bot sends to a topic ends the topic's draft in Telegram ([`sent_to_topic`]): a
//! message still being written goes on in a new draft under it.
//!
//! Only sessions with a topic are told here; the others are «Общее»'s business.
//!
//! [`Lane::Draft`]: crate::transport::Lane::Draft
//! [`DRAFT_EVERY_MS`]: crate::transport::DRAFT_EVERY_MS

use std::collections::{BTreeMap, BTreeSet};

use flux_plugin_api::remote::{Session, SessionState, Steps, TextUpdate, TurnSummary};
use serde_json::Value;

use crate::bot::Bot;
use crate::host::{Host, trf};
use crate::render;
use crate::telegram::{self, Message, StoppedGeneration, TgError};
use crate::transport::{Lane, Module, Purpose};

/// A draft lives about 30 s: one shown this long is sent again.
pub const REFRESH_MS: u64 = 20_000;
/// «Thinking…» after the user's message waits this long for the session to start working (an
/// unloaded one resumes first).
const GETTING_GOING_MS: u64 = 60_000;

#[derive(Default)]
pub struct State {
    /// The draft each session's topic shows now.
    live: BTreeMap<String, Live>,
    /// The last of Claude's messages sent whole (or stopped), by session: older text is late.
    finished: BTreeMap<String, u64>,
    /// The last group of steps sent as a block, by session.
    steps_sent: BTreeMap<String, u64>,
    /// A group of steps still growing, by session (sent at the turn's end if it never ends).
    steps_open: BTreeMap<String, Steps>,
    /// The sessions that work now (starting or working), as their last change said.
    working: BTreeSet<String>,
    /// Sessions stopped from the phone: their turn's end needs no «broke off».
    stopped: BTreeSet<String>,
    /// The timer that sends the waiting drafts again.
    timer: Option<u64>,
    /// Final messages on their way, by their tag's number: (topic, Markdown) — for the fallback.
    finals: BTreeMap<u64, (i64, String)>,
    next_final: u64,
    /// Telegram refused a rich message: the answers go as HTML from then on.
    no_rich: bool,
}

/// A draft shown in a session's topic.
struct Live {
    thread: i64,
    draft: i64,
    /// Claude's message it shows; none — «Thinking…».
    message: Option<u64>,
    /// The Markdown shown so far.
    text: String,
    /// When it was sent last, and first (ms).
    sent: u64,
    since: u64,
}

fn thread_of(bot: &Bot, session: &str) -> Option<i64> {
    bot.store
        .topic_by_session(session)
        .map(|topic| topic.thread)
}

pub fn on_user_message(bot: &mut Bot, host: &mut dyn Host, message: &Message, session: &str) {
    let Some(thread) = message.thread() else {
        return;
    };
    if message.photo.is_some() || message.document.is_some() {
        let note = host.tr("I'll learn to take photos and files later.");
        send_note(bot, host, thread, &render::escape(&note));
        return;
    }
    let words = message.words().trim();
    if words.is_empty() {
        return;
    }
    match host.send(session, words, &[]) {
        Ok(()) => {
            bot.conversation.stopped.remove(session);
            think(bot, host, session);
        }
        Err(err) => {
            let note = trf(host, "Couldn't send it to Claude: {0}", &[&err]);
            send_note(bot, host, thread, &render::escape(&note));
        }
    }
}

/// A session of a topic is about to answer (a new chat's first message went): «Thinking…» now.
pub fn expect_answer(bot: &mut Bot, host: &mut dyn Host, session: &str) {
    think(bot, host, session);
}

pub fn on_text(bot: &mut Bot, host: &mut dyn Host, update: &TextUpdate) {
    let session = update.session.as_str();
    let Some(thread) = thread_of(bot, session) else {
        return;
    };
    if bot
        .conversation
        .finished
        .get(session)
        .is_some_and(|last| update.message <= *last)
    {
        return;
    }
    let now = host.now_ms();
    let fresh = match bot.conversation.live.get(session) {
        // «Thinking…» becomes the message.
        Some(live) => live
            .message
            .is_some_and(|message| message != update.message),
        None => true,
    };
    if fresh {
        let draft = bot.store.next_draft(now);
        bot.conversation.live.insert(
            session.to_string(),
            Live {
                thread,
                draft,
                message: None,
                text: String::new(),
                sent: now,
                since: now,
            },
        );
    }
    let Some(live) = bot.conversation.live.get_mut(session) else {
        return;
    };
    live.message = Some(update.message);
    live.text = update.text.clone();
    if update.done {
        bot.conversation.live.remove(session);
        bot.conversation
            .finished
            .insert(session.to_string(), update.message);
        send_final(bot, host, thread, &update.text);
    } else {
        push_draft(bot, host, session);
        arm(bot, host);
    }
}

pub fn on_steps(bot: &mut Bot, host: &mut dyn Host, session: &str, steps: &Steps) {
    let Some(thread) = thread_of(bot, session) else {
        return;
    };
    if !steps.done {
        bot.conversation
            .steps_open
            .insert(session.to_string(), steps.clone());
        think(bot, host, session);
        return;
    }
    if bot
        .conversation
        .steps_open
        .get(session)
        .is_some_and(|open| open.group == steps.group)
    {
        bot.conversation.steps_open.remove(session);
    }
    send_steps(bot, host, session, thread, steps);
}

fn send_steps(bot: &mut Bot, host: &mut dyn Host, session: &str, thread: i64, steps: &Steps) {
    if bot.conversation.steps_sent.get(session) == Some(&steps.group) || steps.lines.is_empty() {
        return;
    }
    bot.conversation
        .steps_sent
        .insert(session.to_string(), steps.group);
    let html = render::steps_html(&steps.summary, &steps.lines);
    send_note(bot, host, thread, &html);
}

pub fn on_turn_end(bot: &mut Bot, host: &mut dyn Host, session: &str, summary: &TurnSummary) {
    let stopped = bot.conversation.stopped.remove(session);
    let Some(thread) = thread_of(bot, session) else {
        bot.conversation.live.remove(session);
        bot.conversation.steps_open.remove(session);
        return;
    };
    // A draft still open: what it shows is the message.
    if let Some(live) = bot.conversation.live.remove(session)
        && let Some(message) = live.message
    {
        bot.conversation
            .finished
            .insert(session.to_string(), message);
        send_final(bot, host, thread, &live.text);
    }
    // A group of steps nothing came after.
    if let Some(open) = bot.conversation.steps_open.remove(session) {
        send_steps(bot, host, session, thread, &open);
    }
    let line = if !summary.failed {
        done_line(host, summary)
    } else if stopped {
        // «⏹ Stopped» said it.
        disarm_if_idle(bot, host);
        return;
    } else {
        host.tr("⚠️ The turn broke off")
    };
    send_note(bot, host, thread, &render::escape(&line));
    disarm_if_idle(bot, host);
}

/// «✅ Done · 3 files · +12 −4».
fn done_line(host: &dyn Host, summary: &TurnSummary) -> String {
    let mut line = host.tr("✅ Done");
    if !summary.files.is_empty() {
        let added: u64 = summary.files.iter().map(|file| u64::from(file.added)).sum();
        let removed: u64 = summary
            .files
            .iter()
            .map(|file| u64::from(file.removed))
            .sum();
        let files = render::count(host, summary.files.len() as u64, "{0} file", "{0} files");
        line.push_str(&format!(" · {files} · +{added} −{removed}"));
    }
    line
}

pub fn on_failed(bot: &mut Bot, host: &mut dyn Host, session: &str, error: &str) {
    let Some(thread) = thread_of(bot, session) else {
        return;
    };
    bot.conversation.live.remove(session);
    let line = format!("⚠️ {}", render::cut(error.trim(), 1000));
    send_note(bot, host, thread, &render::escape(&line));
    disarm_if_idle(bot, host);
}

/// «Stop» under a draft: the session is interrupted, what the draft showed stays as a message.
pub fn on_stop(bot: &mut Bot, host: &mut dyn Host, stopped: &StoppedGeneration) {
    let Some(session) = bot
        .conversation
        .live
        .iter()
        .find(|(_, live)| live.draft == stopped.draft_id)
        .map(|(session, _)| session.clone())
    else {
        return;
    };
    let Some(live) = bot.conversation.live.remove(&session) else {
        return;
    };
    if let Err(err) = host.interrupt(&session) {
        host.log(&format!("Couldn't stop {session}: {err}"));
    }
    bot.conversation.stopped.insert(session.clone());
    if let Some(message) = live.message {
        bot.conversation.finished.insert(session.clone(), message);
    }
    let mut parts = if live.text.trim().is_empty() {
        Vec::new()
    } else {
        render::html_parts(&live.text)
    };
    let mark = render::escape(&host.tr("⏹ Stopped"));
    match parts.last_mut() {
        Some(last) if render::length(last) + render::length(&mark) + 2 <= render::MESSAGE_LIMIT => {
            last.push_str(&format!("\n\n<i>{mark}</i>"));
        }
        _ => parts.push(format!("<i>{mark}</i>")),
    }
    for part in parts {
        send_note(bot, host, live.thread, &part);
    }
    disarm_if_idle(bot, host);
}

/// A session changed (state, mode, title): «Thinking…» while it works.
pub fn on_session(bot: &mut Bot, host: &mut dyn Host, session: &Session) {
    let works = matches!(
        session.state,
        SessionState::Starting | SessionState::Working
    );
    let was = if works {
        !bot.conversation.working.insert(session.id.clone())
    } else {
        bot.conversation.working.remove(&session.id)
    };
    if thread_of(bot, &session.id).is_none() {
        return;
    }
    if works {
        think(bot, host, &session.id);
    } else if was {
        // It stopped working (waits, rests): «Thinking…» goes; a message keeps its draft.
        if bot
            .conversation
            .live
            .get(&session.id)
            .is_some_and(|live| live.message.is_none())
        {
            bot.conversation.live.remove(&session.id);
        }
        disarm_if_idle(bot, host);
    }
}

/// A session is gone (closed in Flux): a note, and the topic is let go.
pub fn on_gone(bot: &mut Bot, host: &mut dyn Host, session: &str) {
    bot.conversation.live.remove(session);
    bot.conversation.finished.remove(session);
    bot.conversation.steps_sent.remove(session);
    bot.conversation.steps_open.remove(session);
    bot.conversation.working.remove(session);
    bot.conversation.stopped.remove(session);
    disarm_if_idle(bot, host);
    let Some(thread) = thread_of(bot, session) else {
        return;
    };
    let note = host.tr("The session was closed in Flux.");
    send_note(bot, host, thread, &render::escape(&note));
    bot.store.forget_topic(thread);
    bot.save(host);
}

/// The bot sent a message to a topic: Telegram ends the topic's draft. «Thinking…» goes (the next
/// sign of work brings it back); a message still being written goes on in a new draft under it.
pub fn sent_to_topic(bot: &mut Bot, host: &mut dyn Host, thread: i64) {
    let Some(session) = bot
        .conversation
        .live
        .iter()
        .find(|(_, live)| live.thread == thread)
        .map(|(session, _)| session.clone())
    else {
        return;
    };
    let writing = bot
        .conversation
        .live
        .get(&session)
        .is_some_and(|live| live.message.is_some());
    if writing {
        let draft = bot.store.next_draft(host.now_ms());
        if let Some(live) = bot.conversation.live.get_mut(&session) {
            live.draft = draft;
        }
        push_draft(bot, host, &session);
    } else {
        bot.conversation.live.remove(&session);
        disarm_if_idle(bot, host);
    }
}

pub fn on_answer(bot: &mut Bot, host: &mut dyn Host, tag: &str, result: Result<Value, TgError>) {
    let Some(number) = tag
        .strip_prefix("final:")
        .and_then(|n| n.parse::<u64>().ok())
    else {
        if let Err(err) = result
            && !err.topic_gone()
        {
            host.log(&format!("A draft failed: {err:?}"));
        }
        return;
    };
    let Some((thread, markdown)) = bot.conversation.finals.remove(&number) else {
        return;
    };
    match result {
        Ok(_) => {}
        Err(err) if err.topic_gone() => {}
        Err(err) => {
            host.log(&format!(
                "A rich message was refused ({err:?}): HTML instead"
            ));
            bot.conversation.no_rich = true;
            for part in render::html_parts(&markdown) {
                send_note(bot, host, thread, &part);
            }
        }
    }
}

pub fn on_timer(bot: &mut Bot, host: &mut dyn Host, timer: u64) -> bool {
    if bot.conversation.timer != Some(timer) {
        return false;
    }
    bot.conversation.timer = None;
    let now = host.now_ms();
    let state = &bot.conversation;
    // «Thinking…» stays while the session works, or gets going after the user's message.
    let gone: Vec<String> = state
        .live
        .iter()
        .filter(|(session, live)| {
            live.message.is_none()
                && !state.working.contains(*session)
                && now.saturating_sub(live.since) >= GETTING_GOING_MS
        })
        .map(|(session, _)| session.clone())
        .collect();
    for session in gone {
        bot.conversation.live.remove(&session);
    }
    let due: Vec<String> = bot
        .conversation
        .live
        .iter()
        .filter(|(_, live)| now.saturating_sub(live.sent) + 1_000 >= REFRESH_MS)
        .map(|(session, _)| session.clone())
        .collect();
    for session in due {
        push_draft(bot, host, &session);
    }
    arm(bot, host);
    true
}

// --- Sending ---

/// «Thinking…» in the session's topic, unless a draft is shown already.
fn think(bot: &mut Bot, host: &mut dyn Host, session: &str) {
    if bot.conversation.live.contains_key(session) {
        return;
    }
    let Some(thread) = thread_of(bot, session) else {
        return;
    };
    let now = host.now_ms();
    let draft = bot.store.next_draft(now);
    bot.conversation.live.insert(
        session.to_string(),
        Live {
            thread,
            draft,
            message: None,
            text: String::new(),
            sent: now,
            since: now,
        },
    );
    push_draft(bot, host, session);
    arm(bot, host);
}

fn push_draft(bot: &mut Bot, host: &mut dyn Host, session: &str) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let now = host.now_ms();
    let Some(live) = bot.conversation.live.get_mut(session) else {
        return;
    };
    live.sent = now;
    let html = render::draft_html(&live.text);
    let call = telegram::send_draft(chat, Some(live.thread), live.draft, &html, true);
    let lane = Lane::Draft(live.draft);
    bot.call(
        host,
        call,
        lane,
        Purpose::new(Module::Conversation, "draft"),
    );
}

/// Claude's message, whole: a rich message, or HTML in parts.
fn send_final(bot: &mut Bot, host: &mut dyn Host, thread: i64, markdown: &str) {
    let Some(chat) = bot.chat() else {
        return;
    };
    if markdown.trim().is_empty() {
        return;
    }
    if bot.conversation.no_rich {
        for part in render::html_parts(markdown) {
            send_note(bot, host, thread, &part);
        }
        return;
    }
    bot.conversation.next_final += 1;
    let number = bot.conversation.next_final;
    bot.conversation
        .finals
        .insert(number, (thread, markdown.to_string()));
    let call = telegram::send_rich(chat, Some(thread), &render::rich_markdown(markdown), None);
    let purpose = Purpose::new(Module::Conversation, format!("final:{number}"));
    bot.call(host, call, Lane::Topic(thread), purpose);
    sent_to_topic(bot, host, thread);
}

/// An HTML message in a topic (nobody waits for its answer).
fn send_note(bot: &mut Bot, host: &mut dyn Host, thread: i64, html: &str) {
    let Some(chat) = bot.chat() else {
        return;
    };
    let call = telegram::send_message(chat, Some(thread), html, None);
    bot.call(host, call, Lane::Topic(thread), Purpose::none());
    sent_to_topic(bot, host, thread);
}

/// The refresh's timer runs while a draft is shown.
fn arm(bot: &mut Bot, host: &mut dyn Host) {
    if bot.conversation.timer.is_none() && !bot.conversation.live.is_empty() {
        bot.conversation.timer = Some(host.timer(REFRESH_MS as u32));
    }
}

fn disarm_if_idle(bot: &mut Bot, host: &mut dyn Host) {
    if bot.conversation.live.is_empty()
        && let Some(timer) = bot.conversation.timer.take()
    {
        host.cancel_timer(timer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::ClaudeEvent;
    use crate::testing::{FakeHost, Telegram, bot_with_topic, session, user_message};
    use flux_plugin_api::remote::{SystemEvent, TurnFile};
    use serde_json::json;

    fn text(message: u64, text: &str, done: bool) -> ClaudeEvent {
        ClaudeEvent::Text(TextUpdate {
            session: "s1".into(),
            message,
            text: text.into(),
            done,
        })
    }

    fn turn(files: Vec<(u32, u32)>, failed: bool) -> ClaudeEvent {
        let files = files
            .into_iter()
            .enumerate()
            .map(|(index, (added, removed))| TurnFile {
                path: format!("f{index}.rs"),
                added,
                removed,
                created: false,
            })
            .collect();
        ClaudeEvent::TurnEnded(
            "s1".into(),
            TurnSummary {
                turn: 1,
                files,
                failed,
                text: String::new(),
                seconds: 3,
            },
        )
    }

    /// The calls to the session's topic: «Общее»'s status message changes along (sent to its
    /// topic, then edited by its id).
    fn in_topic(bot: &Bot, calls: Vec<(String, Value)>) -> Vec<(String, Value)> {
        let status = bot.store.status_message;
        calls
            .into_iter()
            .filter(|(_, params)| {
                params["message_thread_id"] != crate::testing::GENERAL
                    && (status.is_none() || params["message_id"].as_i64() != status)
            })
            .collect()
    }

    fn methods(calls: &[(String, Value)]) -> Vec<&str> {
        calls.iter().map(|(method, _)| method.as_str()).collect()
    }

    #[test]
    fn the_answer_grows_in_one_draft_then_is_the_message() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_claude(&mut host, text(1, "Hel", false));
        bot.on_claude(&mut host, text(1, "Hello **world", false));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(methods(&calls), ["sendMessageDraft", "sendMessageDraft"]);
        let draft = calls[0].1["draft_id"].clone();
        assert_eq!(calls[1].1["draft_id"], draft);
        assert_eq!(calls[1].1["text"], "Hello **world");
        assert_eq!(calls[1].1["message_thread_id"], 77);
        assert_eq!(calls[1].1["can_stop"], true);
        bot.on_claude(&mut host, text(1, "Hello **world**!", true));
        bot.on_claude(&mut host, text(1, "Hello **world**! late", false));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            methods(&calls),
            ["sendRichMessage"],
            "no draft after the message"
        );
        assert_eq!(calls[0].1["rich_message"]["markdown"], "Hello **world**!");
        bot.on_claude(&mut host, turn(vec![(10, 2), (2, 2), (0, 0)], false));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].1["text"], "✅ Done · 3 files · +12 −4");
        bot.on_claude(&mut host, turn(vec![], true));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].1["text"], "⚠️ The turn broke off");
    }

    #[test]
    fn stop_finds_the_draft_by_its_id_without_the_topic() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_claude(&mut host, text(4, "Looking at `src/a.rs` and", false));
        let calls = tg.flush(&mut bot, &mut host);
        let draft = calls[0].1["draft_id"].as_i64().unwrap();
        let stop: crate::telegram::Update = serde_json::from_value(json!({
            "update_id": 9,
            "stopped_message_generation": { "chat": { "id": crate::testing::OWNER, "type": "private" }, "draft_id": draft.to_string() }
        }))
        .unwrap();
        bot.on_update(&mut host, stop);
        assert_eq!(host.claude.last().unwrap(), "interrupt s1");
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(methods(&calls), ["sendMessage"]);
        assert_eq!(
            calls[0].1["text"],
            "Looking at <code>src/a.rs</code> and\n\n<i>⏹ Stopped</i>"
        );
        assert_eq!(calls[0].1["message_thread_id"], 77);
        // The rest of that message, and the turn's broken end, say nothing more.
        bot.on_claude(&mut host, text(4, "Looking at `src/a.rs` and more", true));
        bot.on_claude(&mut host, turn(vec![], true));
        let rest = tg.flush(&mut bot, &mut host);
        let rest = in_topic(&bot, rest);
        assert!(rest.is_empty(), "{rest:?}");
    }

    #[test]
    fn steps_fold_when_their_group_ends() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        let steps = |done| Steps {
            group: 3,
            summary: "Read 2 files".into(),
            lines: vec!["Read a.rs".into(), "Read b.rs".into()],
            done,
        };
        bot.on_claude(&mut host, ClaudeEvent::Steps("s1".into(), steps(false)));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(
            methods(&calls),
            ["sendMessageDraft"],
            "«Thinking…» while the steps run"
        );
        assert_eq!(calls[0].1["text"], "");
        bot.on_claude(&mut host, ClaudeEvent::Steps("s1".into(), steps(true)));
        bot.on_claude(&mut host, ClaudeEvent::Steps("s1".into(), steps(true)));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(methods(&calls), ["sendMessage"]);
        assert_eq!(
            calls[0].1["text"],
            "<blockquote expandable><b>Read 2 files</b>\nRead a.rs\nRead b.rs</blockquote>"
        );
    }

    #[test]
    fn a_refused_rich_message_goes_as_html() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_claude(
            &mut host,
            text(1, "# Done\n| a | b |\n|---|---|\n| 1 | 2 |", true),
        );
        assert_eq!(
            tg.refuse(&mut bot, &mut host, "Bad Request: can't parse rich message"),
            "sendRichMessage"
        );
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(methods(&calls), ["sendMessage"]);
        assert_eq!(calls[0].1["text"], "<b>Done</b>\n<pre>a  b\n1  2</pre>");
        bot.on_claude(&mut host, text(2, "Next", true));
        assert_eq!(
            methods(&tg.flush(&mut bot, &mut host)),
            ["sendMessage"],
            "HTML from then on"
        );
    }

    #[test]
    fn the_users_words_go_to_the_session_and_thinking_shows_while_it_works() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_update(&mut host, user_message(1, Some(77), "Run the tests"));
        assert_eq!(host.claude.last().unwrap(), "send s1 Run the tests");
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(methods(&calls), ["sendMessageDraft"]);
        let draft = calls[0].1["draft_id"].clone();
        assert_eq!(host.timers.len(), 1, "one timer while a draft is shown");
        bot.on_claude(
            &mut host,
            ClaudeEvent::Session(session("s1", "/work/project", SessionState::Working)),
        );
        host.now += REFRESH_MS;
        let (timer, _) = host.timers.remove(0);
        bot.on_timer(&mut host, timer);
        let calls = tg.flush(&mut bot, &mut host);
        let calls = in_topic(&bot, calls);
        assert_eq!(calls[0].1["draft_id"], draft, "the same draft again");
        bot.on_claude(
            &mut host,
            ClaudeEvent::Session(session("s1", "/work/project", SessionState::Idle)),
        );
        assert!(host.timers.is_empty(), "no timer when nothing works");
        // A photo: a note; a failed send: a note.
        let mut photo = user_message(2, Some(77), "");
        photo.message.as_mut().unwrap().photo = Some(vec![Default::default()]);
        bot.on_update(&mut host, photo);
        host.fail = Some("The session is gone".into());
        bot.on_update(&mut host, user_message(3, Some(77), "Hi"));
        let calls = tg.flush(&mut bot, &mut host);
        let calls = in_topic(&bot, calls);
        assert_eq!(
            calls[0].1["text"],
            "I'll learn to take photos and files later."
        );
        assert_eq!(
            calls[1].1["text"],
            "Couldn't send it to Claude: The session is gone"
        );
    }

    #[test]
    fn a_message_sent_meanwhile_moves_the_draft_under_it() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_claude(&mut host, text(1, "Part", false));
        let first = tg.flush(&mut bot, &mut host)[0].1["draft_id"].clone();
        bot.on_claude(
            &mut host,
            ClaudeEvent::Failed("s1".into(), "The process exited".into()),
        );
        bot.on_claude(&mut host, text(2, "Again", false));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].1["text"], "⚠️ The process exited");
        assert_ne!(calls[1].1["draft_id"], first);
        let _ = SystemEvent::ScreenLocked;
    }

    #[test]
    fn a_closed_session_lets_its_topic_go() {
        let mut host = FakeHost::default();
        let mut bot = bot_with_topic(&mut host, 77, "s1");
        let mut tg = Telegram::default();
        bot.on_claude(&mut host, ClaudeEvent::Gone("s1".into()));
        let calls = tg.flush(&mut bot, &mut host);
        assert_eq!(calls[0].1["text"], "The session was closed in Flux.");
        assert!(bot.store.topic_by_thread(77).is_none());
        assert!(
            host.storage["state"]["topics"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
