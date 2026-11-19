//! Claude in the chats (part 6): the chats the user makes available to Claude, and Claude's tools
//! over them — `chats` (the available chats), `read_chat` (a page of a chat's history: the newest,
//! older than a message, or around one), `search_chat` (in one chat, or in every available one),
//! `send_message` (a message Claude proposes: it leaves only by the user's Send on its card).
//!
//! The access is the list's ([`crate::chats`]): Available to Claude — a ⋯ menu's item, the
//! profile's switch — keeps the chat's id in `claude/<account>` of the storage, a part of the
//! account's list. A hidden chat leaves the list, so it is never available: hiding a chat, or
//! removing it from FluxGram, drops its access, and a key Claude gives for it is "not available",
//! as an unknown one is. The rows carry the access (`ChatRow::claude`): the list's mark, the
//! menus' item, the profile's switch.
//!
//! A call comes to a window's instance (`tool-called`) and goes on to the service
//! ([`ClaudeAsk::Call`]), which holds TDLib: [`Tools`] checks the access at that moment, reads the
//! store for the chats, pages the history (`getChatHistory` gives fewer messages than asked — the
//! ones at hand first —, so it is asked again until the page is full or the chat starts), searches
//! (`searchChatMessages`: each available chat for a search of all), and answers the window that
//! asked ([`Note::Tool`]), which hands the answer to Flux (`tools.respond`). A cancelled call stops
//! its paging; the helper's exit or an account signed out fail the calls in progress with an
//! error: a call never hangs.
//!
//! `send_message` ([`Card`]): the service checks the access, asks TDLib for the message answered
//! (the card's quote) and hands the card to the window that got the call ([`Note::Card`]), which
//! shows it (`tools.confirm`). Send comes back to the service ([`ClaudeAsk::Confirm`]): the access
//! again — turned off meanwhile, nothing goes —, then the feed sends the text as the field does
//! (the Markdown parsed, the reply), and the call is answered once Telegram took the message
//! (`updateMessageSendSucceeded`: its id). Edit puts the text and the quote into the chat's field
//! (`chat.rs`), Don't Send tells Claude so: the window answers those itself.
//!
//! Reply with Claude (a message's action, the field's button — `chat.rs`) turns the chat on and
//! asks the service for the new session's attachment ([`ClaudeAsk::Reply`] → [`Reply`]): the
//! chat, the message and what it answers in Claude's lines, the tools to call with their
//! arguments spelled out.
//!
//! What Claude reads is English, a message a line ([`message_line`]); how the step looks — its
//! line, its dates, the excerpt ([`excerpt_line`]) — is in the interface's language ([`Words`]).
//! The formatters need no host: tested natively.

use std::collections::{BTreeMap, HashMap};

use flux_plugin_api::system::{self, LocalTime};
use flux_plugin_api::tools::Confirmation;
use flux_plugin_api::{log, service};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::chats::Chats;
use crate::model::{ChatKey, Note, Words, unix_now};
use crate::profile::{When, short_date, size_text};
use crate::service::{Asked, Cx, HostWords};
use crate::store::{ChatKind, Store, UserKind};
use crate::summary;

/// What a window asks of Claude's part: `Request::Claude`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "claude", rename_all = "kebab-case")]
pub enum ClaudeAsk {
    /// Available to Claude turned on or off (a ⋯ menu, the profile's switch).
    Set { key: ChatKey, on: bool },
    /// Claude called a tool: Flux's id of the call, the tool's name, its arguments (JSON).
    Call {
        call: u64,
        tool: String,
        arguments: String,
    },
    /// The call is gone (Claude was stopped), or its card was answered by the window (Edit,
    /// Don't Send): its paging stops, its card is forgotten.
    Cancel { call: u64 },
    /// Send on a `send_message` card: the service sends the message.
    Confirm { call: u64 },
    /// Reply with Claude: the chat on for Claude, and the new session's attachment to the window
    /// ([`Note::Session`]) — of a message (its id), or of the field (its unfinished text).
    Reply {
        key: ChatKey,
        message: Option<String>,
        draft: Option<String>,
    },
}

/// A call's answer as Flux's `tool-output` has it: from the service to the window that asked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// What Claude reads.
    pub text: String,
    pub error: bool,
    /// The step's line, in the interface's language.
    pub summary: Option<String>,
    /// Dim on the right of the line: the dates, a count.
    pub detail: Option<String>,
    /// Markdown under the line when the step is open: what Claude read, a message a line.
    pub excerpt: Option<String>,
}

impl Answer {
    pub fn error(text: &str) -> Self {
        Answer {
            text: text.to_string(),
            error: true,
            ..Answer::default()
        }
    }
}

/// A page of `read_chat`: default and most messages.
pub const READ_LIMIT: usize = 50;
const READ_MAX: usize = 100;
/// The matches of `search_chat`: default and most.
pub const SEARCH_LIMIT: usize = 20;
const SEARCH_MAX: usize = 50;
/// TDLib is asked at most this often for one page: a history that keeps giving one message at a
/// time still ends.
const ROUNDS: u32 = 24;
/// An excerpt's line is cut at this many characters.
const EXCERPT_CHARS: usize = 120;

/// What Claude is told when a chat isn't available (unknown, off, hidden, signed out alike).
pub fn not_available(item: &str) -> String {
    format!(
        "This chat isn't available to Claude: the user turns it on with «{item}» in the chat's ⋯ \
         menu in FluxGram."
    )
}

// --- The tools' calls, in the service ---

/// What a TDLib request of the tools asked: its answer comes to [`Tools::answer`].
#[derive(Debug, Clone)]
pub struct Ask {
    pub call: u64,
    pub chat: ChatKey,
}

/// Where a page of history starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    /// The newest messages.
    Newest,
    /// Older than this message.
    Before(i64),
    /// Centred on this message.
    Around(i64),
}

/// Which side of the page a request asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    First,
    Newer,
    Older,
}

/// A page of history being read.
#[derive(Debug)]
pub struct Reading {
    pub from: From,
    pub limit: usize,
    /// By id: the page's messages so far.
    pub messages: BTreeMap<i64, Value>,
    side: Side,
    /// The chat's first message is among them: nothing older.
    pub start: bool,
    /// Around a message: nothing newer, or the newer half is full.
    newer_done: bool,
    /// Around a message: the newest one is among them.
    pub newest: bool,
    rounds: u32,
}

impl Reading {
    pub fn new(from: From, limit: usize) -> Self {
        Reading {
            from,
            limit,
            messages: BTreeMap::new(),
            side: Side::First,
            start: false,
            newer_done: !matches!(from, From::Around(_)),
            newest: false,
            rounds: 0,
        }
    }

    /// The first request: `getChatHistory`'s `from_message_id`, `offset`, `limit` (the message
    /// `from_message_id` itself is among what TDLib gives).
    pub fn first(&self) -> (i64, i32, usize) {
        match self.from {
            From::Newest => (0, 0, self.limit),
            From::Before(id) => (id, 0, self.limit + 1),
            From::Around(id) => (id, -((self.limit / 2) as i32), self.limit),
        }
    }

    /// TDLib's answer: the messages it gave. The next request, or none — the page is done.
    pub fn take(&mut self, messages: &[Value]) -> Option<(i64, i32, usize)> {
        self.rounds += 1;
        let before = self.messages.len();
        let newest_before = self.messages.keys().next_back().copied();
        let oldest_before = self.messages.keys().next().copied();
        for message in messages {
            let Some(id) = message["id"].as_i64() else {
                continue;
            };
            if let From::Before(limit) = self.from
                && id >= limit
            {
                continue;
            }
            self.messages.entry(id).or_insert_with(|| message.clone());
        }
        let added = self.messages.len() > before;
        match self.side {
            // Nothing at all: an empty chat, or nothing older than `before`. (TDLib's first
            // answer may be `before` itself alone: the page is asked for again.)
            Side::First if messages.is_empty() => {
                self.start = true;
                self.newer_done = true;
                self.newest = true;
            }
            Side::Newer => {
                let grew = match newest_before {
                    Some(newest) => self.messages.keys().next_back().copied() > Some(newest),
                    None => added,
                };
                if !grew {
                    self.newer_done = true;
                    self.newest = true;
                }
            }
            Side::Older => {
                let grew = match oldest_before {
                    Some(oldest) => self.messages.keys().next().copied() < Some(oldest),
                    None => added,
                };
                if !grew {
                    self.start = true;
                }
            }
            Side::First => {}
        }
        if self.rounds >= ROUNDS {
            return None;
        }
        if let From::Around(around) = self.from
            && !self.newer_done
        {
            let newer = self.messages.range(around + 1..).count();
            let want = self.limit / 2;
            if newer < want
                && let Some(&newest) = self.messages.keys().next_back()
            {
                self.side = Side::Newer;
                let need = want - newer;
                return Some((newest, -(need as i32), need + 1));
            }
            self.newer_done = true;
        }
        if self.messages.len() < self.limit && !self.start {
            let from = match (self.messages.keys().next(), self.from) {
                (Some(&oldest), _) => oldest,
                (None, From::Before(id)) => id,
                (None, _) => return None,
            };
            self.side = Side::Older;
            let need = self.limit - self.messages.len();
            return Some((from, 0, need + 1));
        }
        None
    }

    /// The page, oldest first: at most `limit`, the older ones dropped (then it isn't the start).
    pub fn page(&mut self) -> Vec<Value> {
        while self.messages.len() > self.limit {
            self.messages.pop_first();
            self.start = false;
        }
        self.messages.values().cloned().collect()
    }
}

/// A search being made: one chat, or each available one.
#[derive(Debug)]
pub struct Searching {
    pub query: String,
    /// One chat's search; none — every available chat's.
    pub chat: Option<ChatKey>,
    pub limit: usize,
    /// The chats still to answer.
    pub waiting: usize,
    /// The matches, with their chat.
    pub found: Vec<(ChatKey, Value)>,
    /// The chats with more matches than given.
    pub more: Vec<ChatKey>,
    /// A chat's search failed: TDLib's message.
    pub failed: Option<String>,
}

/// A message Claude proposes (`send_message`): its card, then its sending.
#[derive(Debug)]
pub struct Sending {
    pub card: Card,
    /// The message answered: asked of TDLib for the card's quote, then sent with the reply.
    pub reply_to: Option<i64>,
    /// The message pending once TDLib took it: `updateMessageSendSucceeded` names it.
    pub pending: Option<i64>,
}

/// A call in progress.
#[derive(Debug)]
pub enum Job {
    Read { chat: ChatKey, reading: Reading },
    Search(Searching),
    Send(Sending),
}

/// Send on a card: what the feed sends (the chat, the text, the message answered), or the
/// call's end — the window to tell, the error.
pub type Confirmed = Result<(ChatKey, String, Option<String>), (u64, Answer)>;

/// A call in progress, with the window that answers it.
#[derive(Debug)]
pub struct Call {
    pub window: u64,
    pub job: Job,
}

/// The tools' calls in progress, by Flux's id.
#[derive(Debug, Default)]
pub struct Tools {
    pub calls: HashMap<u64, Call>,
}

/// `getChatHistory` of a page.
pub fn history_request(chat: i64, (from, offset, limit): (i64, i32, usize)) -> Value {
    json!({"@type": "getChatHistory", "chat_id": chat, "from_message_id": from,
        "offset": offset, "limit": limit.min(READ_MAX), "only_local": false})
}

/// `searchChatMessages` of a chat, older than `before` (0 — the newest).
pub fn search_request(chat: i64, query: &str, before: i64, limit: usize) -> Value {
    json!({"@type": "searchChatMessages", "chat_id": chat, "topic_id": null, "query": query,
        "sender_id": null, "from_message_id": before, "offset": 0, "limit": limit,
        "filter": null})
}

/// The arguments of `read_chat`: the chat's key, where the page starts, how many.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadArgs {
    pub chat: String,
    pub from: From,
    pub limit: usize,
}

/// The arguments of `search_chat`.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchArgs {
    pub query: String,
    pub chat: Option<String>,
    pub before: Option<i64>,
    pub limit: usize,
}

/// A message's id as Claude gives it: a string (as the tools show them) or a number.
fn id_arg(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().trim_matches(['[', ']', '#']).parse().ok(),
        _ => None,
    }
}

fn limit_arg(value: &Value, default: usize, max: usize) -> usize {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        .map_or(default, |n| (n as usize).clamp(1, max))
}

/// `read_chat`'s arguments; the error says what's wrong, for Claude.
pub fn read_args(arguments: &str) -> Result<ReadArgs, String> {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let chat = args["chat"].as_str().unwrap_or_default().trim().to_string();
    if chat.is_empty() {
        return Err("Give the chat's key (`chat`): the `chats` tool lists them.".into());
    }
    let id = |name: &str| -> Result<Option<i64>, String> {
        match &args[name] {
            Value::Null => Ok(None),
            Value::String(s) if s.trim().is_empty() => Ok(None),
            value => id_arg(value)
                .map(Some)
                .ok_or_else(|| format!("`{name}` is a message's id, as the lines show it: [id].")),
        }
    };
    let from = match (id("before")?, id("around")?) {
        (Some(_), Some(_)) => return Err("Give `before` or `around`, not both.".into()),
        (Some(before), None) => From::Before(before),
        (None, Some(around)) => From::Around(around),
        (None, None) => From::Newest,
    };
    Ok(ReadArgs {
        chat,
        from,
        limit: limit_arg(&args["limit"], READ_LIMIT, READ_MAX),
    })
}

/// `search_chat`'s arguments.
pub fn search_args(arguments: &str) -> Result<SearchArgs, String> {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let query = args["query"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("Give the words to look for (`query`).".into());
    }
    let chat = args["chat"]
        .as_str()
        .map(str::trim)
        .filter(|chat| !chat.is_empty())
        .map(str::to_string);
    let before = match &args["before"] {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        value => Some(id_arg(value).ok_or("`before` is a message's id, as the lines show it.")?),
    };
    if before.is_some() && chat.is_none() {
        return Err("`before` pages one chat's matches: give its `chat` too.".into());
    }
    Ok(SearchArgs {
        query,
        chat,
        before,
        limit: limit_arg(&args["limit"], SEARCH_LIMIT, SEARCH_MAX),
    })
}

// --- What Claude reads ---

/// The time the lines are told in: now, and the user's offset from UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub now: i64,
    pub offset: i32,
}

impl Clock {
    fn when(self) -> When {
        When {
            now: self.now,
            offset: self.offset,
        }
    }

    fn local(self, at: i64) -> LocalTime {
        LocalTime::at(at, self.offset)
    }

    fn same_day(self, a: i64, b: i64) -> bool {
        let (a, b) = (self.local(a), self.local(b));
        (a.year, a.month, a.day) == (b.year, b.month, b.day)
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A message's time for Claude: "17 Nov 14:32"; another year's "17 Nov 2025 14:32".
pub fn claude_time(at: i64, clock: Clock) -> String {
    let time = clock.local(at);
    let month = MONTHS[(time.month.clamp(1, 12) - 1) as usize];
    let year = match time.year == clock.local(clock.now).year {
        true => String::new(),
        false => format!(" {}", time.year),
    };
    format!(
        "{} {month}{year} {:02}:{:02}",
        time.day, time.hour, time.minute
    )
}

/// The user's offset for Claude: "UTC+3", "UTC−4:30".
pub fn utc_name(offset: i32) -> String {
    let sign = if offset < 0 { "−" } else { "+" };
    let minutes = offset.abs() / 60;
    match minutes % 60 {
        0 => format!("UTC{sign}{}", minutes / 60),
        rest => format!("UTC{sign}{}:{rest:02}", minutes / 60),
    }
}

/// A duration: "0:12", "1:02:03".
pub fn duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds / 3600 {
        0 => format!("{}:{:02}", seconds / 60, seconds % 60),
        hours => format!("{hours}:{:02}:{:02}", seconds / 60 % 60, seconds % 60),
    }
}

/// English for the shared formatters (a file's size, a deleted account) in Claude's lines.
struct Plain;

impl Words for Plain {
    fn tr(&self, text: &str) -> String {
        text.to_string()
    }

    fn trf(&self, template: &str, values: &[&str]) -> String {
        let mut text = template.to_string();
        for (index, value) in values.iter().enumerate() {
            text = text.replace(&format!("{{{index}}}"), value);
        }
        text
    }

    fn count(&self, n: u64, one: &str, other: &str) -> String {
        let template = if n == 1 { one } else { other };
        template.replace("{0}", &n.to_string())
    }
}

/// A `formattedText` as Markdown: its links as `[text](url)`, code in backticks; the rest as it
/// is. TDLib's offsets count UTF-16 units.
pub fn markdown(formatted: &Value) -> String {
    let text = formatted["text"].as_str().unwrap_or_default();
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut spans: Vec<(usize, usize, String, String)> = Vec::new();
    for entity in formatted["entities"].as_array().into_iter().flatten() {
        let (Some(offset), Some(length)) = (entity["offset"].as_u64(), entity["length"].as_u64())
        else {
            continue;
        };
        let (start, end) = (offset as usize, (offset + length) as usize);
        if end > units.len() || start >= end {
            continue;
        }
        let (open, close) = match entity["type"]["@type"].as_str().unwrap_or_default() {
            "textEntityTypeTextUrl" => (
                "[".to_string(),
                format!("]({})", entity["type"]["url"].as_str().unwrap_or_default()),
            ),
            "textEntityTypeCode" => ("`".to_string(), "`".to_string()),
            "textEntityTypePre" | "textEntityTypePreCode" => {
                ("```\n".to_string(), "\n```".to_string())
            }
            _ => continue,
        };
        // One at a time: an entity inside another keeps the outer one only.
        if spans.iter().any(|(s, e, ..)| start < *e && *s < end) {
            continue;
        }
        spans.push((start, end, open, close));
    }
    spans.sort_by_key(|(start, ..)| *start);
    let mut out = String::new();
    let mut at = 0;
    for (start, end, open, close) in spans {
        out.push_str(&String::from_utf16_lossy(&units[at..start]));
        out.push_str(&open);
        out.push_str(&String::from_utf16_lossy(&units[start..end]));
        out.push_str(&close);
        at = end;
    }
    out.push_str(&String::from_utf16_lossy(&units[at..]));
    out
}

/// The account's own message: "You".
fn own(store: &Store, message: &Value) -> bool {
    message["is_outgoing"] == true
        || (message["sender_id"]["@type"] == "messageSenderUser"
            && message["sender_id"]["user_id"].as_i64() == store.me
            && store.me.is_some())
}

/// Who wrote a message, for Claude: "You", a name, a chat's title.
fn author(store: &Store, message: &Value) -> String {
    if own(store, message) {
        return "You".to_string();
    }
    let name = summary::sender(store, message, &Plain).name;
    if name.is_empty() {
        "Unknown".to_string()
    } else {
        name
    }
}

/// A chat's title from the store: "a chat" when it isn't known.
fn chat_title(store: &Store, chat: i64) -> String {
    store
        .chats
        .get(&chat)
        .map(|chat| chat.title.clone())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "a chat".to_string())
}

/// Forwarded from whom: a user, a hidden user, a chat or a channel.
fn forwarded_from(store: &Store, message: &Value) -> Option<String> {
    let origin = &message["forward_info"]["origin"];
    let name = match origin["@type"].as_str()? {
        "messageOriginUser" => {
            let name = store.user_name(origin["sender_user_id"].as_i64().unwrap_or_default());
            if name.is_empty() {
                "a deleted account".to_string()
            } else {
                name
            }
        }
        "messageOriginHiddenUser" => origin["sender_name"].as_str().unwrap_or("someone").into(),
        "messageOriginChat" => {
            chat_title(store, origin["sender_chat_id"].as_i64().unwrap_or_default())
        }
        "messageOriginChannel" => chat_title(store, origin["chat_id"].as_i64().unwrap_or_default()),
        _ => return None,
    };
    Some(name)
}

/// The message answered, in the same chat.
pub fn reply_to(message: &Value) -> Option<i64> {
    let reply = &message["reply_to"];
    if reply["@type"] == "messageReplyToMessage" {
        return reply["message_id"].as_i64().filter(|id| *id != 0);
    }
    message["reply_to_message_id"]
        .as_i64()
        .filter(|id| *id != 0)
}

/// A photo's largest size: "1280×720".
fn photo_size(photo: &Value) -> Option<String> {
    let size = photo["sizes"]
        .as_array()?
        .iter()
        .max_by_key(|size| size["width"].as_i64().unwrap_or_default())?;
    Some(format!(
        "{}×{}",
        size["width"].as_i64()?,
        size["height"].as_i64()?
    ))
}

/// A file's size, from TDLib's `file`.
fn file_size(file: &Value) -> u64 {
    file["size"]
        .as_u64()
        .filter(|size| *size > 0)
        .or_else(|| file["expected_size"].as_u64())
        .unwrap_or_default()
}

/// A poll's options with their votes: "Pizza — 3, Sushi — 1".
fn poll_text(poll: &Value) -> String {
    let question = markdown_or_text(&poll["question"]);
    let options: Vec<String> = poll["options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|option| {
            format!(
                "{} — {}",
                markdown_or_text(&option["text"]),
                option["voter_count"].as_u64().unwrap_or_default()
            )
        })
        .collect();
    let kind = if poll["type"]["@type"] == "pollTypeQuiz" {
        "quiz"
    } else {
        "poll"
    };
    let closed = if poll["is_closed"] == true {
        ", closed"
    } else {
        ""
    };
    format!(
        "[{kind} «{question}»{closed}: {}]",
        if options.is_empty() {
            "no options".to_string()
        } else {
            options.join(", ")
        }
    )
}

/// A `formattedText` (newer TDLib) or a plain string (older).
fn markdown_or_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        value => markdown(value),
    }
}

/// The message's content for Claude: the text in full, media described; `true` — a service
/// message (an action, said without a colon: "joined").
pub fn content_text(store: &Store, message: &Value) -> (String, bool) {
    let content = &message["content"];
    let caption = || {
        let text = markdown(&content["caption"]);
        if text.trim().is_empty() {
            String::new()
        } else {
            format!(": {text}")
        }
    };
    let names = |ids: &Value| -> String {
        ids.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .map(|id| {
                let name = store.user_name(id);
                if name.is_empty() {
                    "a deleted account".to_string()
                } else {
                    name
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let sender = message["sender_id"]["user_id"].as_i64();
    let said = |text: String| (text, false);
    let action = |text: String| (text, true);
    match content["@type"].as_str().unwrap_or_default() {
        "messageText" => said(markdown(&content["text"])),
        "messagePhoto" => {
            let size = photo_size(&content["photo"]).map_or(String::new(), |s| format!(" {s}"));
            said(format!("[photo{size}{}]", caption()))
        }
        "messageVideo" => {
            let video = &content["video"];
            said(format!(
                "[video {} {}×{}{}]",
                duration(video["duration"].as_i64().unwrap_or_default()),
                video["width"].as_i64().unwrap_or_default(),
                video["height"].as_i64().unwrap_or_default(),
                caption()
            ))
        }
        "messageAnimation" => said(format!("[GIF{}]", caption())),
        "messageVoiceNote" => said(format!(
            "[voice {}{}]",
            duration(
                content["voice_note"]["duration"]
                    .as_i64()
                    .unwrap_or_default()
            ),
            caption()
        )),
        "messageVideoNote" => said(format!(
            "[video message {}]",
            duration(
                content["video_note"]["duration"]
                    .as_i64()
                    .unwrap_or_default()
            )
        )),
        "messageAudio" => {
            let audio = &content["audio"];
            let title = audio["title"].as_str().unwrap_or_default();
            let performer = audio["performer"].as_str().unwrap_or_default();
            let name = match (performer.is_empty(), title.is_empty()) {
                (false, false) => format!("{performer} – {title}"),
                (true, false) => title.to_string(),
                _ => audio["file_name"].as_str().unwrap_or("audio").to_string(),
            };
            said(format!(
                "[audio {name}, {}{}]",
                duration(audio["duration"].as_i64().unwrap_or_default()),
                caption()
            ))
        }
        "messageDocument" => {
            let document = &content["document"];
            let name = document["file_name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or("file");
            let size = size_text(file_size(&document["document"]), &Plain);
            let size = if size.is_empty() {
                String::new()
            } else {
                format!(", {size}")
            };
            said(format!("[file {name}{size}{}]", caption()))
        }
        "messageSticker" => said(format!(
            "[sticker {}]",
            content["sticker"]["emoji"].as_str().unwrap_or_default()
        )),
        "messageAnimatedEmoji" => said(content["emoji"].as_str().unwrap_or_default().to_string()),
        "messageDice" => said(format!(
            "[dice {} {}]",
            content["emoji"].as_str().unwrap_or("🎲"),
            content["value"].as_i64().unwrap_or_default()
        )),
        "messageLocation" => said("[location]".to_string()),
        "messageVenue" => said(format!(
            "[place {}, {}]",
            content["venue"]["title"].as_str().unwrap_or_default(),
            content["venue"]["address"].as_str().unwrap_or_default()
        )),
        "messageContact" => {
            let contact = &content["contact"];
            said(format!(
                "[contact {} {}, {}]",
                contact["first_name"].as_str().unwrap_or_default(),
                contact["last_name"].as_str().unwrap_or_default(),
                contact["phone_number"].as_str().unwrap_or_default()
            ))
        }
        "messagePoll" => said(poll_text(&content["poll"])),
        "messageCall" => said(format!(
            "[{} {}]",
            if content["is_video"] == true {
                "video call"
            } else {
                "call"
            },
            duration(content["duration"].as_i64().unwrap_or_default())
        )),
        "messageStory" => said("[story]".to_string()),
        "messageChatAddMembers" => {
            let ids = &content["member_user_ids"];
            let only_sender = ids
                .as_array()
                .is_some_and(|ids| ids.len() == 1 && ids[0].as_i64() == sender);
            match only_sender {
                true => action("joined".to_string()),
                false => action(format!("added {}", names(ids))),
            }
        }
        "messageChatJoinByLink" => action("joined by a link".to_string()),
        "messageChatJoinByRequest" => action("joined".to_string()),
        "messageChatDeleteMember" => {
            let user = content["user_id"].as_i64();
            match user == sender {
                true => action("left".to_string()),
                false => action(format!("removed {}", names(&json!([user])))),
            }
        }
        "messagePinMessage" => action(format!(
            "pinned [{}]",
            content["message_id"].as_i64().unwrap_or_default()
        )),
        "messageChatChangeTitle" => action(format!(
            "renamed the chat to «{}»",
            content["title"].as_str().unwrap_or_default()
        )),
        "messageChatChangePhoto" => action("changed the chat's photo".to_string()),
        "messageChatDeletePhoto" => action("removed the chat's photo".to_string()),
        "messageBasicGroupChatCreate" | "messageSupergroupChatCreate" => action(format!(
            "created the group «{}»",
            content["title"].as_str().unwrap_or_default()
        )),
        "messageChatUpgradeTo" | "messageChatUpgradeFrom" => {
            action("upgraded the group".to_string())
        }
        "messageScreenshotTaken" => action("took a screenshot".to_string()),
        "messageContactRegistered" => action("joined Telegram".to_string()),
        "messageUnsupported" => said("[a message this version of Telegram can't show]".into()),
        _ => said("[a message of a kind FluxGram doesn't show]".to_string()),
    }
}

/// A message as Claude reads it, on its line (a text's further lines indented):
/// `[id] 17 Nov 14:32 Petya Smirnov (reply to [id]) (forwarded from X) (edited): text`; a
/// service message `[id] 17 Nov 14:32 Anna Smirnova joined`.
pub fn message_line(store: &Store, message: &Value, clock: Clock) -> String {
    let id = message["id"].as_i64().unwrap_or_default();
    let date = message["date"].as_i64().unwrap_or_default();
    let mut line = format!(
        "[{id}] {} {}",
        claude_time(date, clock),
        author(store, message)
    );
    let (text, service) = content_text(store, message);
    if service {
        line.push(' ');
        line.push_str(&text);
        return line;
    }
    if let Some(reply) = reply_to(message) {
        line.push_str(&format!(" (reply to [{reply}])"));
    }
    if let Some(from) = forwarded_from(store, message) {
        line.push_str(&format!(" (forwarded from {from})"));
    }
    if message["edit_date"].as_i64().unwrap_or_default() > 0 {
        line.push_str(" (edited)");
    }
    line.push_str(": ");
    line.push_str(&text.replace('\n', "\n    "));
    line
}

/// What kind of chat it is, for Claude.
pub fn kind_name(store: &Store, chat: i64) -> &'static str {
    if store.is_saved_messages(chat) {
        return "saved messages";
    }
    match store.chats.get(&chat).map(|chat| chat.kind) {
        Some(ChatKind::Private { user } | ChatKind::Secret { user }) => {
            match store.users.get(&user).map(|user| user.kind) {
                Some(UserKind::Bot) => "bot",
                _ => "private chat",
            }
        }
        Some(ChatKind::Supergroup { channel: true, .. }) => "channel",
        Some(ChatKind::BasicGroup { .. } | ChatKind::Supergroup { .. }) => "group",
        _ => "chat",
    }
}

/// A chat's header for Claude: `"Backend · team" — group, 5 members (account Egor)`.
pub fn chat_header(store: &Store, chat: i64, account: Option<&str>) -> String {
    let kind = kind_name(store, chat);
    let members = match (kind, store.group(chat)) {
        ("group" | "channel", Some(group)) if group.member_count > 0 => {
            let noun = if kind == "channel" {
                "subscribers"
            } else {
                "members"
            };
            format!(", {} {noun}", group.member_count)
        }
        _ => String::new(),
    };
    let account = account.map_or(String::new(), |name| format!(" (account {name})"));
    format!("\"{}\" — {kind}{members}{account}", chat_title(store, chat))
}

// --- What the user sees of a step ---

/// Markdown's own characters in a message's text, escaped: the excerpt shows them as they are.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '#' | '|' | '~'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// One line of at most `max` characters, its spaces collapsed: "…" where it is cut.
fn cut(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
    cut = cut.trim_end().to_string();
    cut.push('…');
    cut
}

/// A media message's words for the user: "[Фото] the caption", "[Голосовое сообщение 0:12]";
/// none — a text or a service message.
fn media_words(store: &Store, message: &Value, words: &dyn Words) -> Option<String> {
    let content = &message["content"];
    let kind = content["@type"].as_str().unwrap_or_default();
    let seconds = |value: &Value| format!(" {}", duration(value.as_i64().unwrap_or_default()));
    let (word, extra) = match kind {
        "messageText" => return None,
        "messagePhoto" => (words.tr("Photo"), String::new()),
        "messageVideo" => (words.tr("Video"), seconds(&content["video"]["duration"])),
        "messageAnimation" => (words.tr("GIF"), String::new()),
        "messageVoiceNote" => (
            words.tr("Voice message"),
            seconds(&content["voice_note"]["duration"]),
        ),
        "messageVideoNote" => (
            words.tr("Video message"),
            seconds(&content["video_note"]["duration"]),
        ),
        "messageSticker" => (
            words.tr("Sticker"),
            format!(
                " {}",
                content["sticker"]["emoji"].as_str().unwrap_or_default()
            ),
        ),
        "messageDocument" => (
            words.tr("File"),
            format!(
                " {}",
                content["document"]["file_name"]
                    .as_str()
                    .unwrap_or_default()
            ),
        ),
        "messageAudio"
        | "messagePoll"
        | "messageLocation"
        | "messageVenue"
        | "messageContact"
        | "messageCall"
        | "messageDice"
        | "messageAnimatedEmoji"
        | "messageStory" => (summary::line(store, message, words), String::new()),
        _ => return None,
    };
    let caption = content["caption"]["text"].as_str().unwrap_or_default();
    let mut text = format!("[{}{}]", word, extra.trim_end());
    if !caption.trim().is_empty() {
        text.push(' ');
        text.push_str(caption);
    }
    Some(text)
}

/// A message on the excerpt's line, as the user would read it: `**Петя, 14:32:** текст` — cut to
/// one line, media in words; `with_chat` — the chat's title before the author (a search of all).
pub fn excerpt_line(
    store: &Store,
    message: &Value,
    clock: Clock,
    with_chat: Option<&str>,
    words: &dyn Words,
) -> String {
    let date = message["date"].as_i64().unwrap_or_default();
    let time = clock.local(date);
    let hour = format!("{:02}:{:02}", time.hour, time.minute);
    let when = if clock.same_day(date, clock.now) {
        hour
    } else {
        format!("{} {hour}", short_date(date, clock.when(), words))
    };
    let author = if own(store, message) {
        words.tr("You")
    } else {
        summary::sender(store, message, words).name
    };
    let who = match with_chat {
        Some(chat) => format!("{chat} · {author}"),
        None => author,
    };
    let content = &message["content"];
    let (_, service) = content_text(store, message);
    let text = if service {
        format!(
            "_{}_",
            escape(&cut(&summary::line(store, message, words), EXCERPT_CHARS))
        )
    } else if let Some(media) = media_words(store, message, words) {
        escape(&cut(&media, EXCERPT_CHARS))
    } else {
        let text = content["text"]["text"].as_str().unwrap_or_default();
        escape(&cut(text, EXCERPT_CHARS))
    };
    format!("**{}, {when}:** {text}", escape(&who))
}

/// The excerpt: its lines as lines (Markdown's hard breaks).
pub fn excerpt(lines: &[String]) -> String {
    lines.join("  \n")
}

/// A page's dates, dim on the step's line: "14:02–14:32" today, "14 Nov" one day, "15–17 Nov",
/// "30 Oct – 2 Nov".
pub fn date_range(first: i64, last: i64, clock: Clock, words: &dyn Words) -> String {
    if first <= 0 || last <= 0 {
        return String::new();
    }
    let (a, b) = (clock.local(first), clock.local(last));
    let hm = |t: &LocalTime| format!("{:02}:{:02}", t.hour, t.minute);
    if clock.same_day(first, clock.now) && clock.same_day(last, clock.now) {
        return match hm(&a) == hm(&b) {
            true => hm(&b),
            false => format!("{}–{}", hm(&a), hm(&b)),
        };
    }
    if clock.same_day(first, last) {
        return short_date(last, clock.when(), words);
    }
    if (a.year, a.month) == (b.year, b.month) {
        // "{0} Nov" with "15–17" for the day: the month's own word, as one date has it.
        let one = short_date(last, clock.when(), words);
        let day = b.day.to_string();
        return match one.find(&day) {
            Some(at) => format!("{}{}–{}{}", &one[..at], a.day, day, &one[at + day.len()..]),
            None => one,
        };
    }
    format!(
        "{} – {}",
        short_date(first, clock.when(), words),
        short_date(last, clock.when(), words)
    )
}

/// "50 messages" in the interface's language.
pub fn messages_count(n: usize, words: &dyn Words) -> String {
    words.count(n as u64, "{0} message", "{0} messages")
}

/// A read page's step: the line, the dates, the excerpt.
pub fn read_step(
    store: &Store,
    title: &str,
    from: From,
    page: &[Value],
    clock: Clock,
    words: &dyn Words,
) -> (String, Option<String>, Option<String>) {
    let count = messages_count(page.len(), words);
    let summary = match from {
        From::Newest => words.trf("{0} — {1}", &[title, &count]),
        From::Before(_) => words.count(page.len() as u64, "{0} more message", "{0} more messages"),
        From::Around(id) => {
            let at = page
                .iter()
                .find(|message| message["id"].as_i64() == Some(id))
                .or(page.last())
                .and_then(|message| message["date"].as_i64())
                .unwrap_or_default();
            let date = short_date(at, clock.when(), words);
            words.trf("{0} — {1} around {2}", &[title, &count, &date])
        }
    };
    let dates = |message: Option<&Value>| {
        message
            .and_then(|message| message["date"].as_i64())
            .unwrap_or_default()
    };
    let detail = Some(date_range(
        dates(page.first()),
        dates(page.last()),
        clock,
        words,
    ))
    .filter(|detail| !detail.is_empty());
    let lines: Vec<String> = page
        .iter()
        .map(|message| excerpt_line(store, message, clock, None, words))
        .collect();
    let excerpt = (!lines.is_empty()).then(|| excerpt(&lines));
    (summary, detail, excerpt)
}

/// A read page's text for Claude: the header, the messages oldest first, the cursor.
pub fn read_text(
    store: &Store,
    chat: &ChatKey,
    account: Option<&str>,
    reading: &mut Reading,
    clock: Clock,
) -> (String, Vec<Value>) {
    let page = reading.page();
    let mut text = format!(
        "Chat {} — key {}\n",
        chat_header(store, chat.chat, account),
        chat.tab()
    );
    if page.is_empty() {
        text.push_str(match reading.from {
            From::Newest => "The chat has no messages.",
            From::Before(_) => "Nothing older: this is the start of the chat.",
            From::Around(_) => "No such message in this chat.",
        });
        return (text, page);
    }
    text.push_str(&format!(
        "{} messages, oldest first; times in the user's time zone ({}):\n",
        page.len(),
        utc_name(clock.offset)
    ));
    for message in &page {
        text.push_str(&message_line(store, message, clock));
        text.push('\n');
    }
    match reading.start {
        true => text.push_str("This is the start of the chat."),
        false => {
            let oldest = page[0]["id"].as_i64().unwrap_or_default();
            text.push_str(&format!("Older: before={oldest}"));
        }
    }
    if let From::Around(_) = reading.from
        && !reading.newest
        && let Some(newest) = page.last().and_then(|message| message["id"].as_i64())
    {
        text.push_str(&format!("\nNewer: around={newest}"));
    }
    (text, page)
}

/// The name of the access's item in the interface's language: what Claude tells the user.
pub const ITEM: &str = "Available to Claude";

impl Tools {
    /// A call: the access at this moment, then the store's answer (`chats`) or TDLib's requests.
    pub fn call(
        &mut self,
        cx: &mut Cx,
        chats: &Chats,
        names: &HashMap<String, String>,
        window: u64,
        (call, tool, arguments): (u64, &str, &str),
    ) {
        let words = HostWords;
        let clock = now();
        let item = words.tr(ITEM);
        let available = |text: &str| {
            ChatKey::parse(text.trim())
                .filter(|key| cx.stores.contains_key(&key.account) && chats.claude_available(key))
        };
        match tool {
            "chats" => {
                let keys = available_chats(cx, chats, names);
                respond(
                    window,
                    call,
                    chats_answer(cx.stores, &keys, names, &item, clock, &words),
                );
            }
            "read_chat" => {
                let args = match read_args(arguments) {
                    Ok(args) => args,
                    Err(err) => return respond(window, call, Answer::error(&err)),
                };
                let Some(key) = available(&args.chat) else {
                    return respond(window, call, Answer::error(&not_available(&item)));
                };
                if !cx.td.running() {
                    return respond(window, call, Answer::error(OFFLINE));
                }
                let reading = Reading::new(args.from, args.limit);
                cx.td.send(
                    &key.account,
                    history_request(key.chat, reading.first()),
                    Asked::Claude(Ask {
                        call,
                        chat: key.clone(),
                    }),
                );
                self.calls.insert(
                    call,
                    Call {
                        window,
                        job: Job::Read { chat: key, reading },
                    },
                );
            }
            "search_chat" => {
                let args = match search_args(arguments) {
                    Ok(args) => args,
                    Err(err) => return respond(window, call, Answer::error(&err)),
                };
                let keys = match &args.chat {
                    Some(chat) => match available(chat) {
                        Some(key) => vec![key],
                        None => return respond(window, call, Answer::error(&not_available(&item))),
                    },
                    None => available_chats(cx, chats, names),
                };
                if keys.is_empty() {
                    return respond(
                        window,
                        call,
                        chats_answer(cx.stores, &keys, names, &item, clock, &words),
                    );
                }
                if !cx.td.running() {
                    return respond(window, call, Answer::error(OFFLINE));
                }
                for key in &keys {
                    cx.td.send(
                        &key.account,
                        search_request(
                            key.chat,
                            &args.query,
                            args.before.unwrap_or_default(),
                            args.limit,
                        ),
                        Asked::Claude(Ask {
                            call,
                            chat: key.clone(),
                        }),
                    );
                }
                let searching = Searching {
                    query: args.query,
                    chat: args.chat.and(keys.first().cloned()),
                    limit: args.limit,
                    waiting: keys.len(),
                    found: Vec::new(),
                    more: Vec::new(),
                    failed: None,
                };
                self.calls.insert(
                    call,
                    Call {
                        window,
                        job: Job::Search(searching),
                    },
                );
            }
            "send_message" => {
                let args = match send_args(arguments) {
                    Ok(args) => args,
                    Err(err) => return respond(window, call, Answer::error(&err)),
                };
                let Some(key) = available(&args.chat) else {
                    return respond(window, call, Answer::error(&not_available(&item)));
                };
                if !cx.td.running() {
                    return respond(window, call, Answer::error(OFFLINE));
                }
                let empty = Store::default();
                let store = cx.stores.get(&key.account).unwrap_or(&empty);
                let card = Card {
                    title: chat_title(store, key.chat),
                    account: names.get(&key.account).cloned(),
                    chat: key.clone(),
                    text: args.text,
                    reply: None,
                };
                // The card shows once the message answered is at hand: its quote.
                match args.reply_to {
                    Some(id) => cx.td.send(
                        &key.account,
                        json!({"@type": "getMessage", "chat_id": key.chat, "message_id": id}),
                        Asked::Claude(Ask {
                            call,
                            chat: key.clone(),
                        }),
                    ),
                    None => show_card(window, call, &card),
                }
                let sending = Sending {
                    card,
                    reply_to: args.reply_to,
                    pending: None,
                };
                self.calls.insert(
                    call,
                    Call {
                        window,
                        job: Job::Send(sending),
                    },
                );
            }
            other => respond(
                window,
                call,
                Answer::error(&format!("FluxGram has no tool {other}.")),
            ),
        }
    }

    /// TDLib's answer to a request of a call: the next page's request, or the answer. A call
    /// that is gone (cancelled) takes nothing.
    pub fn answer(
        &mut self,
        cx: &mut Cx,
        chats: &Chats,
        names: &HashMap<String, String>,
        ask: Ask,
        value: &Value,
    ) {
        let Some(entry) = self.calls.get_mut(&ask.call) else {
            return;
        };
        let words = HostWords;
        let problem = (value["@type"] == "error")
            .then(|| value["message"].as_str().unwrap_or_default().to_string());
        match &mut entry.job {
            Job::Read { chat, reading } => {
                // The access again: it may have been turned off while the pages came.
                let answer = if !chats.claude_available(chat) {
                    Some(Answer::error(&not_available(&words.tr(ITEM))))
                } else if let Some(problem) = problem {
                    Some(Answer::error(&format!(
                        "Telegram couldn't give the chat's messages: {problem}"
                    )))
                } else {
                    let messages = value["messages"].as_array().cloned().unwrap_or_default();
                    match reading.take(&messages) {
                        Some(next) => {
                            cx.td.send(
                                &chat.account,
                                history_request(chat.chat, next),
                                Asked::Claude(ask.clone()),
                            );
                            None
                        }
                        None => {
                            let clock = now();
                            let empty = Store::default();
                            let store = cx.stores.get(&chat.account).unwrap_or(&empty);
                            let account = names.get(&chat.account).map(String::as_str);
                            let (text, page) = read_text(store, chat, account, reading, clock);
                            let title = chat_title(store, chat.chat);
                            let (summary, detail, excerpt) =
                                read_step(store, &title, reading.from, &page, clock, &words);
                            Some(Answer {
                                text,
                                error: false,
                                summary: Some(summary),
                                detail,
                                excerpt,
                            })
                        }
                    }
                };
                if let Some(answer) = answer {
                    let window = entry.window;
                    self.calls.remove(&ask.call);
                    respond(window, ask.call, answer);
                }
            }
            Job::Search(searching) => {
                searching.waiting = searching.waiting.saturating_sub(1);
                match problem {
                    Some(problem) => searching.failed = Some(problem),
                    None => {
                        for message in value["messages"].as_array().into_iter().flatten() {
                            searching.found.push((ask.chat.clone(), message.clone()));
                        }
                        if value["next_from_message_id"].as_i64().unwrap_or_default() != 0 {
                            searching.more.push(ask.chat.clone());
                        }
                    }
                }
                if searching.waiting > 0 {
                    return;
                }
                let answer = match (&searching.failed, searching.found.is_empty()) {
                    (Some(problem), true) => {
                        Answer::error(&format!("Telegram couldn't search: {problem}"))
                    }
                    _ => search_answer(cx.stores, searching, names, now(), &words),
                };
                let window = entry.window;
                self.calls.remove(&ask.call);
                respond(window, ask.call, answer);
            }
            // The message answered (`getMessage`): the card's quote, then the card.
            Job::Send(sending) => {
                let id = sending.reply_to.unwrap_or_default();
                if let Some(problem) = problem {
                    let window = entry.window;
                    self.calls.remove(&ask.call);
                    let text = format!(
                        "There's no message [{id}] in this chat ({problem}): reply_to is the [id] \
                         of a message read_chat showed."
                    );
                    return respond(window, ask.call, Answer::error(&text));
                }
                let empty = Store::default();
                let store = cx.stores.get(&ask.chat.account).unwrap_or(&empty);
                sending.card.reply = Some(quoted(store, value, &words));
                show_card(entry.window, ask.call, &sending.card);
            }
        }
    }

    /// Send on a `send_message` card: what the feed sends — the chat, the text, the message
    /// answered —, if Claude may still write there. Turned off meanwhile: nothing goes, the call
    /// ends with the error for the window that got it (`Err`). None — no such card (cancelled).
    pub fn confirmed(&mut self, chats: &Chats, call: u64, item: &str) -> Option<Confirmed> {
        let entry = self.calls.get(&call)?;
        let Job::Send(sending) = &entry.job else {
            return None;
        };
        if !chats.claude_available(&sending.card.chat) {
            let window = entry.window;
            self.calls.remove(&call);
            let text = format!("Nothing was sent. {}", not_available(item));
            return Some(Err((window, Answer::error(&text))));
        }
        Some(Ok((
            sending.card.chat.clone(),
            sending.card.text.clone(),
            sending.reply_to.map(|id| id.to_string()),
        )))
    }

    /// TDLib's answer to Claude's message (`sendMessage`, [`crate::feed::Ask::Sent`]): an error
    /// ends the call; a pending message waits for Telegram ([`Tools::send_update`]); one sent at
    /// once answers now.
    pub fn sent(&mut self, call: u64, value: &Value) {
        let Some(entry) = self.calls.get_mut(&call) else {
            return;
        };
        let Job::Send(sending) = &mut entry.job else {
            return;
        };
        if value["@type"] == "message" && value["sending_state"].is_object() {
            sending.pending = value["id"].as_i64();
            return;
        }
        let window = entry.window;
        if let Some(Call {
            job: Job::Send(sending),
            ..
        }) = self.calls.remove(&call)
        {
            respond(
                window,
                call,
                sent_answer(&sending.card, value, now(), &HostWords),
            );
        }
    }

    /// `updateMessageSendSucceeded` / `updateMessageSendFailed` of an account: Claude's message
    /// pending among them answers its call.
    pub fn send_update(&mut self, account: &str, update: &Value) {
        let kind = update["@type"].as_str().unwrap_or_default();
        if !matches!(
            kind,
            "updateMessageSendSucceeded" | "updateMessageSendFailed"
        ) {
            return;
        }
        let old = update["old_message_id"].as_i64();
        let found = self.calls.iter().find_map(|(id, call)| match &call.job {
            Job::Send(sending)
                if sending.card.chat.account == account
                    && sending.pending.is_some()
                    && sending.pending == old =>
            {
                Some(*id)
            }
            _ => None,
        });
        let Some(Call {
            window,
            job: Job::Send(sending),
        }) = found.and_then(|id| self.calls.remove(&id))
        else {
            return;
        };
        let call = found.unwrap_or_default();
        let answer = match kind {
            "updateMessageSendFailed" => json!({"@type": "error",
                "message": update["error"]["message"].clone()}),
            _ => update["message"].clone(),
        };
        respond(
            window,
            call,
            sent_answer(&sending.card, &answer, now(), &HostWords),
        );
    }

    /// The call is gone (Claude was stopped): its next page isn't asked for.
    pub fn cancel(&mut self, call: u64) {
        self.calls.remove(&call);
    }

    /// The helper stopped, or an account is gone (`account`): their calls end with an error —
    /// their answers won't come.
    pub fn fail(&mut self, account: Option<&str>) {
        let gone: Vec<u64> = self
            .calls
            .iter()
            .filter(|(_, call)| {
                account.is_none_or(|account| match &call.job {
                    Job::Read { chat, .. } => chat.account == account,
                    Job::Send(sending) => sending.card.chat.account == account,
                    // A search of every chat may wait for the account's answer.
                    Job::Search(searching) => searching
                        .chat
                        .as_ref()
                        .is_none_or(|key| key.account == account),
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if let Some(call) = self.calls.remove(&id) {
                respond(call.window, id, Answer::error(OFFLINE));
            }
        }
    }
}

/// What Claude is told when TDLib can't be asked.
const OFFLINE: &str = "FluxGram isn't connected to Telegram right now: try again in a moment.";

/// The chats available to Claude across the signed-in accounts, in the accounts' order.
fn available_chats(cx: &Cx, chats: &Chats, names: &HashMap<String, String>) -> Vec<ChatKey> {
    let mut accounts: Vec<&String> = cx.stores.keys().collect();
    // The accounts' own order (the names come in it): stable between calls.
    accounts.sort_by_key(|account| {
        (
            names.get(*account).cloned().unwrap_or_default(),
            (*account).clone(),
        )
    });
    accounts
        .into_iter()
        .flat_map(|account| {
            chats
                .claude_chats(account)
                .into_iter()
                .map(move |chat| ChatKey::new(account, chat))
        })
        .collect()
}

/// Now, and the user's offset from UTC.
pub fn now() -> Clock {
    let now = unix_now() as i64;
    Clock {
        now,
        offset: system::utc_offset(now),
    }
}

/// The answer goes to the window that got the call.
pub fn respond(window: u64, call: u64, answer: Answer) {
    if let Err(err) = service::send_json_to(window, &Note::Tool { call, answer }) {
        log::warn(&format!("A tool's answer didn't reach its window: {err}"));
    }
}

/// A `send_message` card goes to the window that got the call: it shows it.
fn show_card(window: u64, call: u64, card: &Card) {
    let note = Note::Card {
        call,
        card: card.clone(),
    };
    if let Err(err) = service::send_json_to(window, &note) {
        log::warn(&format!("A card didn't reach its window: {err}"));
    }
}

/// A search's text for Claude and its step: the matches newest first, the cursor.
pub fn search_answer(
    stores: &HashMap<String, Store>,
    searching: &mut Searching,
    names: &HashMap<String, String>,
    clock: Clock,
    words: &dyn Words,
) -> Answer {
    let date = |value: &Value| value["date"].as_i64().unwrap_or_default();
    searching
        .found
        .sort_by_key(|(_, message)| std::cmp::Reverse(date(message)));
    if searching.found.len() > searching.limit {
        // The chats cut off have more.
        for (key, _) in searching.found.split_off(searching.limit) {
            if !searching.more.contains(&key) {
                searching.more.push(key);
            }
        }
    }
    let empty = Store::default();
    let store_of = |key: &ChatKey| stores.get(&key.account).unwrap_or(&empty);
    let title_of = |key: &ChatKey| chat_title(store_of(key), key.chat);
    let query = &searching.query;
    let count = searching.found.len();
    let mut text = match &searching.chat {
        Some(key) => format!(
            "Search \"{query}\" in {} — key {}: ",
            chat_header(
                store_of(key),
                key.chat,
                names.get(&key.account).map(String::as_str)
            ),
            key.tab()
        ),
        None => format!("Search \"{query}\" in every chat available to Claude: "),
    };
    if count == 0 {
        text.push_str("nothing found.");
    } else {
        text.push_str(&format!(
            "{count} messages, newest first; times in the user's time zone ({}):\n",
            utc_name(clock.offset)
        ));
    }
    let mut lines = Vec::new();
    for (key, message) in &searching.found {
        let store = store_of(key);
        let line = message_line(store, message, clock);
        match &searching.chat {
            Some(_) => text.push_str(&line),
            None => text.push_str(&format!("chat={} \"{}\" {line}", key.tab(), title_of(key))),
        }
        text.push('\n');
        let title = title_of(key);
        let with_chat = searching.chat.is_none().then_some(title.as_str());
        lines.push(excerpt_line(store, message, clock, with_chat, words));
    }
    // The cursor: the oldest match shown of each chat with more.
    let oldest = |key: &ChatKey| {
        searching
            .found
            .iter()
            .filter(|(of, _)| of == key)
            .filter_map(|(_, message)| message["id"].as_i64())
            .min()
    };
    for key in &searching.more {
        let Some(before) = oldest(key) else {
            continue;
        };
        match &searching.chat {
            Some(_) => text.push_str(&format!("More: before={before}\n")),
            None => text.push_str(&format!(
                "More in \"{}\": chat={} before={before}\n",
                title_of(key),
                key.tab()
            )),
        }
    }
    if let Some(problem) = &searching.failed {
        text.push_str(&format!("Some chats couldn't be searched: {problem}\n"));
    }
    let summary = match &searching.chat {
        Some(key) => words.trf("Search «{0}» in «{1}»", &[query, &title_of(key)]),
        None => words.trf("Search «{0}»", &[query]),
    };
    let detail = match count {
        0 => words.tr("nothing found"),
        n => messages_count(n, words),
    };
    Answer {
        text: text.trim_end().to_string(),
        error: false,
        summary: Some(summary),
        detail: Some(detail),
        excerpt: (!lines.is_empty()).then(|| excerpt(&lines)),
    }
}

/// The `chats` tool's text and step: the chats available to Claude across the accounts.
pub fn chats_answer(
    stores: &HashMap<String, Store>,
    keys: &[ChatKey],
    names: &HashMap<String, String>,
    item: &str,
    clock: Clock,
    words: &dyn Words,
) -> Answer {
    if keys.is_empty() {
        return Answer {
            text: format!(
                "No chats are available to Claude yet: the user turns a chat on with «{item}» in \
                 its ⋯ menu in FluxGram."
            ),
            summary: Some(words.trf("Chats for Claude — {0}", &["0"])),
            ..Answer::default()
        };
    }
    let mut text = format!(
        "Chats available to Claude ({}); read one with read_chat, its key as `chat`:\n",
        keys.len()
    );
    let mut titles = Vec::new();
    for key in keys {
        let Some(store) = stores.get(&key.account) else {
            continue;
        };
        let account = names.get(&key.account).map(String::as_str);
        text.push_str(&format!(
            "- {} — key {}",
            chat_header(store, key.chat, account),
            key.tab()
        ));
        if let Some(chat) = store.chats.get(&key.chat) {
            if chat.unread_count > 0 {
                text.push_str(&format!("; {} unread", chat.unread_count));
            }
            if let Some(last) = &chat.last_message {
                let line = message_line(store, last, clock);
                text.push_str(&format!("; last: {}", cut(&line, 200)));
            }
        }
        text.push('\n');
        titles.push(format!("**{}**", escape(&chat_title(store, key.chat))));
    }
    Answer {
        text: text.trim_end().to_string(),
        error: false,
        summary: Some(words.trf("Chats for Claude — {0}", &[&titles.len().to_string()])),
        detail: None,
        excerpt: Some(excerpt(&titles)),
    }
}

// --- send_message: the card, what Claude hears (no host: tested natively) ---

/// Telegram's longest message.
const TEXT_MAX: usize = 4096;
/// The quote's line on the card and the chip's text of Reply with Claude are cut at this many
/// characters.
const QUOTE_CHARS: usize = 100;
const LABEL_CHARS: usize = 40;

/// The arguments of `send_message`.
#[derive(Debug, Clone, PartialEq)]
pub struct SendArgs {
    pub chat: String,
    pub text: String,
    pub reply_to: Option<i64>,
}

/// `send_message`'s arguments; the error says what's wrong, for Claude.
pub fn send_args(arguments: &str) -> Result<SendArgs, String> {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let chat = args["chat"].as_str().unwrap_or_default().trim().to_string();
    if chat.is_empty() {
        return Err("Give the chat's key (`chat`): the `chats` tool lists them.".into());
    }
    let text = args["text"].as_str().unwrap_or_default().trim().to_string();
    if text.is_empty() {
        return Err("Give the message's text (`text`).".into());
    }
    if text.chars().count() > TEXT_MAX {
        return Err(format!(
            "The text is longer than Telegram's {TEXT_MAX} characters: make it shorter."
        ));
    }
    let reply_to = match &args["reply_to"] {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        value => {
            Some(id_arg(value).ok_or("`reply_to` is a message's id, as the lines show it: [id].")?)
        }
    };
    Ok(SendArgs {
        chat,
        text,
        reply_to,
    })
}

/// The message a card answers, quoted over its text (and over the chat's field after Edit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quoted {
    pub message: String,
    /// In the interface's language: the user's own is "You".
    pub author: String,
    /// The author's color (0–6).
    pub color: u8,
    /// One line of it, media in words.
    pub text: String,
}

/// The message answered, as the card quotes it.
pub fn quoted(store: &Store, message: &Value, words: &dyn Words) -> Quoted {
    let sender = summary::sender(store, message, words);
    let color = match sender.user {
        true => store.users.get(&sender.id).map(|user| user.color),
        false => store.chats.get(&sender.id).map(|chat| chat.color),
    };
    Quoted {
        message: message["id"].as_i64().unwrap_or_default().to_string(),
        author: match own(store, message) {
            true => words.tr("You"),
            false => sender.name,
        },
        color: color.unwrap_or_default(),
        text: cut(&summary::line(store, message, words), QUOTE_CHARS),
    }
}

/// A `send_message` call's card: the service makes it (the chat's title, the message answered),
/// the window that got the call shows it and acts on the user's choice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub chat: ChatKey,
    /// The chat's title.
    pub title: String,
    /// The account's name, with two accounts or more: dim after the title.
    pub account: Option<String>,
    /// What goes: Markdown, as the field sends it.
    pub text: String,
    pub reply: Option<Quoted>,
}

/// What became of a card's message: the step's line says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    /// Edit: the text went into the chat's field.
    Field,
    NotSent,
}

/// What Claude hears of Edit.
pub const EDITED: &str = "The user took the text into the chat's field to edit and send it \
                          themselves — don't send it again.";
/// What Claude hears of Don't Send.
pub const DECLINED: &str = "The user chose not to send it.";

impl Card {
    /// "Reply in «Backend · team»", "Message in «Anna Smirnova»".
    pub fn heading(&self, words: &dyn Words) -> String {
        match self.reply {
            Some(_) => words.trf("Reply in «{0}»", &[&self.title]),
            None => words.trf("Message in «{0}»", &[&self.title]),
        }
    }

    /// The step's line once the user chose: "Reply in «Backend · team» — sent".
    pub fn step(&self, outcome: Outcome, words: &dyn Words) -> String {
        let template = match (self.reply.is_some(), outcome) {
            (true, Outcome::Sent) => "Reply in «{0}» — sent",
            (false, Outcome::Sent) => "Message in «{0}» — sent",
            (true, Outcome::Field) => "Reply in «{0}» — in the chat's field",
            (false, Outcome::Field) => "Message in «{0}» — in the chat's field",
            (true, Outcome::NotSent) => "Reply in «{0}» — not sent",
            (false, Outcome::NotSent) => "Message in «{0}» — not sent",
        };
        words.trf(template, &[&self.title])
    }

    /// The open step: the text, its lines kept.
    pub fn excerpt(&self) -> String {
        let lines: Vec<String> = self.text.lines().map(escape).collect();
        excerpt(&lines)
    }

    /// The card as Flux shows it: Send, Edit, Don't Send.
    pub fn confirmation(&self, words: &dyn Words) -> Confirmation {
        let mut card = Confirmation::new(
            &self.heading(words),
            &self.text,
            &words.tr("Send"),
            &words.tr("Don't Send"),
        )
        .edit(&words.tr("Edit"))
        .hint(&words.tr("«Edit» puts the text into the chat's field"));
        if let Some(account) = &self.account {
            card = card.subtitle(account);
        }
        if let Some(reply) = &self.reply {
            card = card.quote(&reply.author, &reply.text);
        }
        card
    }

    /// Edit (`edit`) or Don't Send: what Claude hears, and the step.
    pub fn declined(&self, edit: bool, words: &dyn Words) -> Answer {
        let (text, outcome) = match edit {
            true => (EDITED, Outcome::Field),
            false => (DECLINED, Outcome::NotSent),
        };
        Answer {
            text: text.to_string(),
            error: false,
            summary: Some(self.step(outcome, words)),
            detail: None,
            excerpt: Some(self.excerpt()),
        }
    }
}

/// Telegram's answer to the message sent (or TDLib's error): what Claude hears — the message's
/// id —, the step with the time it went.
pub fn sent_answer(card: &Card, message: &Value, clock: Clock, words: &dyn Words) -> Answer {
    if message["@type"] == "error" || message["@type"].is_null() {
        let problem = message["message"].as_str().unwrap_or("no answer");
        return Answer {
            text: format!("Telegram couldn't send it: {problem}"),
            error: true,
            summary: Some(card.step(Outcome::NotSent, words)),
            detail: None,
            excerpt: Some(card.excerpt()),
        };
    }
    let id = message["id"].as_i64().unwrap_or_default();
    let date = message["date"].as_i64().unwrap_or(clock.now);
    let time = clock.local(date);
    Answer {
        text: format!("Sent to «{}» as [{id}].", card.title),
        error: false,
        summary: Some(card.step(Outcome::Sent, words)),
        detail: Some(format!("{:02}:{:02}", time.hour, time.minute)),
        excerpt: Some(card.excerpt()),
    }
}

// --- Reply with Claude (no host: tested natively) ---

/// The session Reply with Claude opens: its title, the chip, what Claude reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub title: String,
    pub label: String,
    pub markdown: String,
}

/// Reply with Claude's session: of a message (with the one it answers, when the feed has it),
/// or of the field (`message` none; `draft` — the field's unfinished text). The attachment says
/// which tools to call with which arguments: a model acts on it at once.
#[allow(clippy::too_many_arguments)]
pub fn reply_session(
    store: &Store,
    chat: &ChatKey,
    account: Option<&str>,
    message: Option<&Value>,
    replied: Option<&Value>,
    draft: Option<&str>,
    clock: Clock,
    words: &dyn Words,
) -> Reply {
    let title = chat_title(store, chat.chat);
    let key = chat.tab();
    let mut markdown = format!(
        "FluxGram chat {} — key {key}.\n\n",
        chat_header(store, chat.chat, account)
    );
    let tools = "mcp__flux__fluxgram_read_chat";
    let send = "mcp__flux__fluxgram_send_message";
    let label = match message {
        Some(message) => {
            let id = message["id"].as_i64().unwrap_or_default();
            markdown.push_str("The user asks for help replying to this message:\n");
            markdown.push_str(&message_line(store, message, clock));
            markdown.push('\n');
            if let Some(replied) = replied {
                markdown.push_str("It replies to:\n");
                markdown.push_str(&message_line(store, replied, clock));
                markdown.push('\n');
            }
            markdown.push_str(&format!(
                "\nTo understand the context, read the chat with {tools}: chat=\"{key}\", \
                 around=\"{id}\"; then page back with before=<the oldest id shown> until the \
                 context is clear.\nTo reply, call {send}: chat=\"{key}\", reply_to=\"{id}\", \
                 text=<the reply, Markdown as in Telegram>."
            ));
            let author = match own(store, message) {
                true => words.tr("You"),
                false => summary::sender(store, message, words).name,
            };
            let line = cut(&summary::line(store, message, words), LABEL_CHARS);
            words.trf("{0} — {1}: «{2}»", &[&title, &author, &line])
        }
        None => {
            match draft.map(str::trim).filter(|draft| !draft.is_empty()) {
                Some(draft) => {
                    markdown.push_str(
                        "The user is writing in this chat; their unfinished text in the field:\n",
                    );
                    for line in draft.lines() {
                        markdown.push_str(&format!("> {line}\n"));
                    }
                }
                None => markdown.push_str("The user wants to write in this chat.\n"),
            }
            markdown.push_str(&format!(
                "\nTo understand the context, read the latest messages with {tools}: \
                 chat=\"{key}\"; then page back with before=<the oldest id shown> until the \
                 context is clear.\nTo write, call {send}: chat=\"{key}\", text=<the message, \
                 Markdown as in Telegram> (reply_to=<a message's id> answers it)."
            ));
            title.clone()
        }
    };
    markdown.push_str(
        " The user sends it from a card in this conversation — or edits it, or declines it: \
         propose the text with the tool rather than asking in words.",
    );
    Reply {
        title: words.trf("Reply: {0}", &[&title]),
        label,
        markdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::English;

    /// 2026-11-17 12:00 UTC.
    const NOW: i64 = 1_794_916_800;
    const MSK: Clock = Clock {
        now: NOW,
        offset: 3 * 3600,
    };

    fn store() -> Store {
        let mut store = Store::default();
        for (id, first, last) in [(103, "Petya", "Smirnov"), (101, "Anna", "Smirnova")] {
            store.apply(
                &json!({"@type": "updateUser", "user": {"@type": "user", "id": id,
                "first_name": first, "last_name": last, "type": {"@type": "userTypeRegular"}}}),
            );
        }
        store.apply(
            &json!({"@type": "updateNewChat", "chat": {"@type": "chat", "id": -5,
            "type": {"@type": "chatTypeBasicGroup", "basic_group_id": 5}, "title": "Backend · team",
            "positions": []}}),
        );
        store.me = Some(1);
        store
    }

    fn message(id: i64, sender: i64, date: i64, content: Value) -> Value {
        json!({"@type": "message", "id": id, "chat_id": -5, "date": date,
            "sender_id": {"@type": "messageSenderUser", "user_id": sender},
            "is_outgoing": sender == 1, "content": content})
    }

    fn text(value: &str) -> Value {
        json!({"@type": "messageText", "text": {"@type": "formattedText", "text": value,
            "entities": []}})
    }

    #[test]
    fn times_say_the_year_only_when_it_is_another() {
        assert_eq!(claude_time(NOW - 86_400, MSK), "16 Nov 15:00");
        assert_eq!(claude_time(NOW - 365 * 86_400, MSK), "17 Nov 2025 15:00");
        assert_eq!(utc_name(3 * 3600), "UTC+3");
        assert_eq!(utc_name(-(4 * 3600 + 1800)), "UTC−4:30");
        assert_eq!(duration(12), "0:12");
        assert_eq!(duration(3723), "1:02:03");
    }

    #[test]
    fn lines_tell_who_when_what() {
        let store = store();
        let mut said = message(
            2048,
            103,
            NOW - 3600,
            text("Release on Friday?\nOr Thursday?"),
        );
        said["reply_to"] = json!({"@type": "messageReplyToMessage", "chat_id": -5,
            "message_id": 1024});
        said["edit_date"] = json!(NOW);
        said["forward_info"] = json!({"origin": {"@type": "messageOriginHiddenUser",
            "sender_name": "Ghost"}});
        assert_eq!(
            message_line(&store, &said, MSK),
            "[2048] 17 Nov 14:00 Petya Smirnov (reply to [1024]) (forwarded from Ghost) \
             (edited): Release on Friday?\n    Or Thursday?"
        );
        let mine = message(3072, 1, NOW, text("Sure"));
        assert_eq!(
            message_line(&store, &mine, MSK),
            "[3072] 17 Nov 15:00 You: Sure"
        );
        let joined = message(
            4096,
            101,
            NOW,
            json!({"@type": "messageChatAddMembers", "member_user_ids": [101]}),
        );
        assert_eq!(
            message_line(&store, &joined, MSK),
            "[4096] 17 Nov 15:00 Anna Smirnova joined"
        );
        let pinned = message(
            5120,
            101,
            NOW,
            json!({"@type": "messagePinMessage", "message_id": 2048}),
        );
        assert_eq!(
            message_line(&store, &pinned, MSK),
            "[5120] 17 Nov 15:00 Anna Smirnova pinned [2048]"
        );
    }

    #[test]
    fn media_is_described() {
        let store = store();
        let line = |content: Value| content_text(&store, &message(1, 103, NOW, content)).0;
        let caption = |text: &str| json!({"@type": "formattedText", "text": text, "entities": []});
        assert_eq!(
            line(
                json!({"@type": "messagePhoto", "caption": caption("The office"),
                "photo": {"sizes": [{"width": 320, "height": 180}, {"width": 1280, "height": 720}]}})
            ),
            "[photo 1280×720: The office]"
        );
        assert_eq!(
            line(json!({"@type": "messageVoiceNote", "caption": caption(""),
                "voice_note": {"duration": 12}})),
            "[voice 0:12]"
        );
        assert_eq!(
            line(json!({"@type": "messageVideoNote", "video_note": {"duration": 31}})),
            "[video message 0:31]"
        );
        assert_eq!(
            line(json!({"@type": "messageVideo", "caption": caption(""),
                "video": {"duration": 42, "width": 1920, "height": 1080}})),
            "[video 0:42 1920×1080]"
        );
        assert_eq!(
            line(json!({"@type": "messageSticker", "sticker": {"emoji": "😂"}})),
            "[sticker 😂]"
        );
        assert_eq!(
            line(json!({"@type": "messageDocument", "caption": caption(""),
                "document": {"file_name": "report.pdf", "document": {"size": 2_516_582}}})),
            "[file report.pdf, 2.4 MB]"
        );
        assert_eq!(
            line(
                json!({"@type": "messagePoll", "poll": {"question": caption("Lunch?"),
                "options": [{"text": caption("Pizza"), "voter_count": 3},
                    {"text": caption("Sushi"), "voter_count": 1}],
                "type": {"@type": "pollTypeRegular"}}})
            ),
            "[poll «Lunch?»: Pizza — 3, Sushi — 1]"
        );
        assert_eq!(line(json!({"@type": "messageLocation"})), "[location]");
        assert_eq!(
            line(json!({"@type": "messageAnimation", "caption": caption("")})),
            "[GIF]"
        );
        assert_eq!(
            line(json!({"@type": "messageSomethingNew"})),
            "[a message of a kind FluxGram doesn't show]"
        );
    }

    #[test]
    fn links_and_code_are_markdown() {
        // "Мы 🚀 тут": the rocket is two UTF-16 units.
        let formatted = json!({"text": "Мы 🚀 тут: docs and cargo build", "entities": [
            {"offset": 11, "length": 4, "type": {"@type": "textEntityTypeTextUrl",
                "url": "https://example.com"}},
            {"offset": 20, "length": 11, "type": {"@type": "textEntityTypeCode"}},
            {"offset": 0, "length": 2, "type": {"@type": "textEntityTypeBold"}}
        ]});
        assert_eq!(
            markdown(&formatted),
            "Мы 🚀 тут: [docs](https://example.com) and `cargo build`"
        );
    }

    #[test]
    fn excerpt_lines_read_as_the_chat() {
        let store = store();
        let long = "a ".repeat(100);
        let said = message(1, 103, NOW - 3600, text(&long));
        let line = excerpt_line(&store, &said, MSK, None, &English);
        assert!(line.starts_with("**Petya Smirnov, 14:00:** a a"));
        assert!(line.ends_with('…'));
        assert!(!line.contains('\n'));
        let yesterday = message(
            2,
            1,
            NOW - 86_400,
            json!({"@type": "messageVoiceNote", "voice_note": {"duration": 12},
                "caption": {"text": ""}}),
        );
        assert_eq!(
            excerpt_line(&store, &yesterday, MSK, Some("Backend · team"), &English),
            "**Backend · team · You, 16 Nov 15:00:** \\[Voice message 0:12\\]"
        );
        let stars = message(3, 101, NOW, text("2*2 = 4"));
        assert_eq!(
            excerpt_line(&store, &stars, MSK, None, &English),
            "**Anna Smirnova, 15:00:** 2\\*2 = 4"
        );
        assert_eq!(excerpt(&["a".into(), "b".into()]), "a  \nb");
    }

    #[test]
    fn date_ranges() {
        let day = 86_400;
        assert_eq!(date_range(NOW - 600, NOW, MSK, &English), "14:50–15:00");
        assert_eq!(
            date_range(NOW - 3 * day, NOW - 3 * day + 60, MSK, &English),
            "14 Nov"
        );
        assert_eq!(date_range(NOW - 2 * day, NOW, MSK, &English), "15–17 Nov");
        assert_eq!(
            date_range(NOW - 20 * day, NOW, MSK, &English),
            "28 Oct – 17 Nov"
        );
    }

    fn page(ids: std::ops::RangeInclusive<i64>) -> Vec<Value> {
        ids.rev()
            .map(|id| message(id, 103, NOW - 1000 + id, text("x")))
            .collect()
    }

    #[test]
    fn the_newest_page_is_asked_until_full_or_the_start() {
        let mut reading = Reading::new(From::Newest, 50);
        assert_eq!(reading.first(), (0, 0, 50));
        // TDLib's first answer: one message at hand.
        assert_eq!(reading.take(&page(200..=200)), Some((200, 0, 50)));
        assert_eq!(reading.take(&page(170..=200)), Some((170, 0, 20)));
        assert_eq!(reading.take(&page(151..=170)), None);
        let messages = reading.page();
        assert_eq!(messages.len(), 50);
        assert!(!reading.start);
        assert_eq!(messages[0]["id"], 151);
        // A short chat: the start.
        let mut short = Reading::new(From::Newest, 50);
        assert_eq!(short.take(&page(1..=10)), Some((1, 0, 41)));
        assert_eq!(short.take(&page(1..=1)), None);
        assert!(short.start);
    }

    #[test]
    fn a_page_before_a_message_leaves_it_out() {
        let mut reading = Reading::new(From::Before(100), 10);
        assert_eq!(reading.first(), (100, 0, 11));
        assert_eq!(reading.take(&page(90..=100)), None);
        let messages = reading.page();
        assert_eq!(messages.len(), 10);
        assert_eq!(messages.last().unwrap()["id"], 99);
        // TDLib's first answer: the message itself alone — asked again.
        let mut again = Reading::new(From::Before(100), 10);
        assert_eq!(again.take(&page(100..=100)), Some((100, 0, 11)));
        assert_eq!(again.take(&page(90..=100)), None);
        assert!(!again.start);
        assert_eq!(again.page().len(), 10);
        // Nothing older: the start.
        let mut first = Reading::new(From::Before(1), 10);
        assert_eq!(first.take(&page(1..=1)), Some((1, 0, 11)));
        assert_eq!(first.take(&page(1..=1)), None);
        assert!(first.start);
        assert!(first.page().is_empty());
    }

    #[test]
    fn a_page_around_a_message_has_both_sides() {
        let mut reading = Reading::new(From::Around(100), 10);
        assert_eq!(reading.first(), (100, -5, 10));
        // At hand: only the message itself; then the newer half, then the older one.
        assert_eq!(reading.take(&page(100..=100)), Some((100, -5, 6)));
        assert_eq!(reading.take(&page(100..=105)), Some((100, 0, 5)));
        assert_eq!(reading.take(&page(96..=100)), None);
        let ids: Vec<i64> = reading
            .page()
            .iter()
            .map(|m| m["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, (96..=105).collect::<Vec<_>>());
        assert!(!reading.newest);
        // The newest message: nothing newer comes.
        let mut last = Reading::new(From::Around(100), 10);
        assert_eq!(last.take(&page(96..=100)), Some((100, -5, 6)));
        assert_eq!(last.take(&page(96..=100)), Some((96, 0, 6)));
        assert_eq!(last.take(&page(91..=96)), None);
        assert!(last.newest);
    }

    #[test]
    fn arguments_are_read_kindly() {
        let read = read_args(r#"{"chat": "a1:-5", "before": "2048", "limit": 500}"#).unwrap();
        assert_eq!(read.from, From::Before(2048));
        assert_eq!(read.limit, READ_MAX);
        let around = read_args(r#"{"chat": "a1:-5", "around": 1024}"#).unwrap();
        assert_eq!(around.from, From::Around(1024));
        assert_eq!(around.limit, READ_LIMIT);
        assert!(read_args(r#"{"chat": "a1:-5", "before": "1", "around": "2"}"#).is_err());
        assert!(read_args("{}").is_err());
        let search = search_args(r#"{"query": " стейдж "}"#).unwrap();
        assert_eq!(search.query, "стейдж");
        assert_eq!(search.limit, SEARCH_LIMIT);
        assert!(search_args(r#"{"query": "x", "before": "5"}"#).is_err());
        assert!(search_args(r#"{"query": ""}"#).is_err());
    }

    #[test]
    fn read_text_ends_with_the_cursor() {
        let store = store();
        let key = ChatKey::new("a1", -5);
        let mut reading = Reading::new(From::Newest, 2);
        reading.take(&page(10..=12));
        let (text, page) = read_text(&store, &key, None, &mut reading, MSK);
        assert_eq!(page.len(), 2);
        assert!(text.starts_with("Chat \"Backend · team\" — group — key a1:-5\n"));
        assert!(text.ends_with("Older: before=11"), "{text}");
        let (summary, detail, excerpt) =
            read_step(&store, "Backend · team", From::Newest, &page, MSK, &English);
        assert_eq!(summary, "Backend · team — 2 messages");
        assert_eq!(detail.as_deref(), Some("14:43"));
        assert_eq!(excerpt.unwrap().lines().count(), 2);
        let (more, ..) = read_step(
            &store,
            "Backend · team",
            From::Before(9),
            &page,
            MSK,
            &English,
        );
        assert_eq!(more, "2 more messages");
    }

    // --- send_message, Reply with Claude (part 6, agent S) ---

    fn card(reply: bool) -> Card {
        Card {
            chat: ChatKey::new("a1", -5),
            title: "Backend · team".into(),
            account: None,
            text: "**Yes**, at 3.\nSee you".into(),
            reply: reply.then(|| Quoted {
                message: "42".into(),
                author: "Petya Smirnov".into(),
                color: 2,
                text: "Are we on?".into(),
            }),
        }
    }

    #[test]
    fn send_arguments_are_read_kindly() {
        let args = send_args(r#"{"chat": " a1:-5 ", "text": " Hi ", "reply_to": "[42]"}"#);
        assert_eq!(
            args,
            Ok(SendArgs {
                chat: "a1:-5".into(),
                text: "Hi".into(),
                reply_to: Some(42),
            })
        );
        let no_reply = send_args(r#"{"chat": "a1:-5", "text": "Hi", "reply_to": ""}"#);
        assert_eq!(no_reply.unwrap().reply_to, None);
        let number = send_args(r#"{"chat": "a1:-5", "text": "Hi", "reply_to": 7}"#);
        assert_eq!(number.unwrap().reply_to, Some(7));
        let empty = send_args(r#"{"chat": "a1:-5", "text": "  "}"#);
        assert!(empty.unwrap_err().contains("`text`"));
        assert!(
            send_args(r#"{"text": "Hi"}"#)
                .unwrap_err()
                .contains("`chat`")
        );
        let wrong = send_args(r#"{"chat": "a1:-5", "text": "Hi", "reply_to": "soon"}"#);
        assert!(wrong.unwrap_err().contains("reply_to"));
        let long = json!({"chat": "a1:-5", "text": "a".repeat(4097)}).to_string();
        assert!(send_args(&long).unwrap_err().contains("4096"));
    }

    #[test]
    fn the_card_says_where_it_goes() {
        let card_of_reply = card(true).confirmation(&English);
        assert_eq!(card_of_reply.title, "Reply in «Backend · team»");
        assert_eq!(card_of_reply.subtitle, None);
        assert_eq!(
            card_of_reply.quote,
            Some(("Petya Smirnov".into(), "Are we on?".into()))
        );
        assert_eq!(card_of_reply.text, "**Yes**, at 3.\nSee you");
        assert_eq!(card_of_reply.confirm, "Send");
        assert_eq!(card_of_reply.edit.as_deref(), Some("Edit"));
        assert_eq!(card_of_reply.reject, "Don't Send");
        assert_eq!(
            card_of_reply.hint.as_deref(),
            Some("«Edit» puts the text into the chat's field")
        );
        // A new message; the account named with two of them.
        let mut message = card(false);
        message.account = Some("Egor Ageev".into());
        let card_of_message = message.confirmation(&English);
        assert_eq!(card_of_message.title, "Message in «Backend · team»");
        assert_eq!(card_of_message.subtitle.as_deref(), Some("Egor Ageev"));
        assert_eq!(card_of_message.quote, None);
    }

    #[test]
    fn claude_hears_what_the_user_chose() {
        let reply = card(true);
        let message = json!({"@type": "message", "id": 77, "date": NOW});
        let sent = sent_answer(&reply, &message, MSK, &English);
        assert_eq!(sent.text, "Sent to «Backend · team» as [77].");
        assert!(!sent.error);
        assert_eq!(
            sent.summary.as_deref(),
            Some("Reply in «Backend · team» — sent")
        );
        assert_eq!(sent.detail.as_deref(), Some("15:00"));
        assert_eq!(
            sent.excerpt.as_deref(),
            Some("\\*\\*Yes\\*\\*, at 3.  \nSee you")
        );
        let edit = reply.declined(true, &English);
        assert_eq!(
            edit.text,
            "The user took the text into the chat's field to edit and send it themselves — \
             don't send it again."
        );
        assert_eq!(
            edit.summary.as_deref(),
            Some("Reply in «Backend · team» — in the chat's field")
        );
        assert!(!edit.error);
        // Don't Send isn't an error: the user decided.
        let kept = card(false).declined(false, &English);
        assert_eq!(kept.text, "The user chose not to send it.");
        assert_eq!(
            kept.summary.as_deref(),
            Some("Message in «Backend · team» — not sent")
        );
        assert!(!kept.error);
        // Telegram refused it.
        let refused = json!({"@type": "error", "message": "CHAT_WRITE_FORBIDDEN"});
        let failed = sent_answer(&reply, &refused, MSK, &English);
        assert!(failed.error);
        assert_eq!(
            failed.text,
            "Telegram couldn't send it: CHAT_WRITE_FORBIDDEN"
        );
        assert_eq!(
            failed.summary.as_deref(),
            Some("Reply in «Backend · team» — not sent")
        );
    }

    #[test]
    fn nothing_goes_once_the_chat_is_turned_off() {
        let sending = || Call {
            window: 3,
            job: Job::Send(Sending {
                card: card(true),
                reply_to: Some(42),
                pending: None,
            }),
        };
        let mut tools = Tools::default();
        tools.calls.insert(9, sending());
        // On: the feed sends it, with the reply.
        let on = Chats::with_list("a1", vec![-5, 101], vec![-5]);
        assert_eq!(
            tools.confirmed(&on, 9, ITEM),
            Some(Ok((
                ChatKey::new("a1", -5),
                "**Yes**, at 3.\nSee you".to_string(),
                Some("42".to_string())
            )))
        );
        // Turned off between the card and Send: nothing goes, Claude hears why, the call ends.
        let off = Chats::with_list("a1", vec![-5, 101], vec![]);
        let Some(Err((window, answer))) = tools.confirmed(&off, 9, ITEM) else {
            panic!("sent to a chat turned off");
        };
        assert_eq!(window, 3);
        assert!(answer.error);
        assert!(
            answer
                .text
                .starts_with("Nothing was sent. This chat isn't available to Claude")
        );
        assert!(tools.calls.is_empty());
        // Hidden meanwhile (out of the list): the same.
        tools.calls.insert(9, sending());
        let hidden = Chats::with_list("a1", vec![101], vec![-5]);
        assert!(matches!(tools.confirmed(&hidden, 9, ITEM), Some(Err(_))));
        // A call that is gone has no card.
        assert_eq!(tools.confirmed(&on, 9, ITEM), None);
    }

    #[test]
    fn reply_with_claude_attaches_the_message_and_the_way_to_answer() {
        let store = store();
        let key = ChatKey::new("a1", -5);
        let asked = message(
            42,
            103,
            NOW - 600,
            text("Are we still on for the deploy at 3? Need to know before the standup."),
        );
        let mut ping = message(43, 101, NOW - 300, text("Ping"));
        ping["reply_to"] =
            json!({"@type": "messageReplyToMessage", "chat_id": -5, "message_id": 42});
        let reply = reply_session(
            &store,
            &key,
            None,
            Some(&ping),
            Some(&asked),
            None,
            MSK,
            &English,
        );
        assert_eq!(reply.title, "Reply: Backend · team");
        assert_eq!(reply.label, "Backend · team — Anna Smirnova: «Ping»");
        assert!(
            reply
                .markdown
                .starts_with("FluxGram chat \"Backend · team\" — group — key a1:-5.\n\n")
        );
        assert!(reply.markdown.contains(
            "[43] 17 Nov 14:55 Anna Smirnova (reply to [42]): Ping\nIt replies to:\n[42] 17 Nov \
             14:50 Petya Smirnov: Are we still on"
        ));
        assert!(
            reply
                .markdown
                .contains("mcp__flux__fluxgram_read_chat: chat=\"a1:-5\", around=\"43\"")
        );
        assert!(
            reply.markdown.contains(
                "mcp__flux__fluxgram_send_message: chat=\"a1:-5\", reply_to=\"43\", text="
            )
        );
        assert!(reply.markdown.contains("The user sends it from a card"));
        // A long text is cut on the chip; media in words.
        let long = reply_session(&store, &key, None, Some(&asked), None, None, MSK, &English);
        assert_eq!(
            long.label,
            "Backend · team — Petya Smirnov: «Are we still on for the deploy at 3? Ne…»"
        );
        let photo = message(
            44,
            1,
            NOW,
            json!({"@type": "messagePhoto", "photo": {"sizes": []},
                "caption": {"@type": "formattedText", "text": "The view", "entities": []}}),
        );
        let photo = reply_session(&store, &key, None, Some(&photo), None, None, MSK, &English);
        assert!(photo.label.starts_with("Backend · team — You: «Photo"));
        assert!(photo.markdown.contains("[44] 17 Nov 15:00 You: [photo"));
        // The field's button: the chat, the unfinished text.
        let field = reply_session(
            &store,
            &key,
            Some("Egor"),
            None,
            None,
            Some("I think\nwe should"),
            MSK,
            &English,
        );
        assert_eq!(field.label, "Backend · team");
        assert!(field.markdown.contains("(account Egor) — key a1:-5."));
        assert!(field.markdown.contains("> I think\n> we should\n"));
        assert!(
            field
                .markdown
                .contains("mcp__flux__fluxgram_read_chat: chat=\"a1:-5\";")
        );
        assert!(
            field
                .markdown
                .contains("mcp__flux__fluxgram_send_message: chat=\"a1:-5\", text=")
        );
        let empty = reply_session(&store, &key, None, None, None, Some(" "), MSK, &English);
        assert!(
            empty
                .markdown
                .contains("The user wants to write in this chat.")
        );
    }
}
