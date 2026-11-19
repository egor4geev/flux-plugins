//! The Telegram Bot API as Flux Remote uses it: the updates the bot reads and the calls it makes,
//! as plain values — no host here (tested natively). A call is a method and its JSON parameters
//! ([`Call`]); the transport ([`crate::transport`]) sends it and brings the answer back.
//!
//! What a live bot showed (wiki «Telegram APIs» → «Живая проверка», 2026-11-19): topics in the
//! private chat with the bot (Bot API 9.3–9.4) — a message the user writes outside topics (the
//! app's «New chat» tab) opens a topic with an implicit name (the text's first 16 characters,
//! `is_name_implicit`), then the message itself; the user can neither create a named topic nor
//! delete one — the bot can do both; pins are the chat's, not a topic's. A draft
//! (`sendMessageDraft`, ≈0.6 s a call) with `can_stop` gets the user's «Stop» as
//! `stopped_message_generation` with the draft's id as a **string** and not always the topic; the
//! text is gone then even with `keep_on_stop`. `sendRichMessage` takes Markdown with
//! `<details>`; buttons have a `style`. No limit was hit at four drafts a second.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

/// Telegram's address; tests and scenarios send it elsewhere with Flux's
/// `FLUX_PLUGIN_HTTP_OVERRIDE` (the plugin still asks for this one).
pub const API: &str = "https://api.telegram.org";

/// How long `getUpdates` waits for news, in seconds (the request's own timeout is longer).
pub const POLL_SECONDS: u32 = 30;

/// Why a call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum TgError {
    /// 401: the token is wrong or revoked.
    Unauthorized,
    /// 409 on `getUpdates`: another program polls this bot (another Flux, a script).
    Conflict,
    /// 429: try again in this many seconds.
    Limited(u64),
    /// Telegram refused the call (400, 403…): its code and description ("Bad Request:
    /// TOPIC_ID_INVALID").
    Refused(i64, String),
    /// No answer: the network, a timeout, a body that isn't Telegram's.
    Network(String),
}

impl TgError {
    /// The topic is gone (the bot deleted it): `TOPIC_ID_INVALID`, "message thread not found".
    pub fn topic_gone(&self) -> bool {
        matches!(self, TgError::Refused(_, text)
            if text.contains("TOPIC_ID_INVALID") || text.contains("thread not found"))
    }

    /// An edit that changes nothing: fine.
    pub fn not_modified(&self) -> bool {
        matches!(self, TgError::Refused(_, text) if text.contains("message is not modified"))
    }

    /// The button's answer came too late (the user pressed it long ago): fine.
    pub fn query_too_old(&self) -> bool {
        matches!(self, TgError::Refused(_, text) if text.contains("query is too old"))
    }
}

/// Every method's answer.
#[derive(Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    error_code: Option<i64>,
    #[serde(default)]
    parameters: Option<Parameters>,
}

#[derive(Deserialize, Default)]
struct Parameters {
    #[serde(default)]
    retry_after: Option<u64>,
}

/// An answer's body as its result, or why the call failed.
pub fn parse(status: u16, body: &[u8]) -> Result<Value, TgError> {
    let envelope: Envelope = match serde_json::from_slice(body) {
        Ok(envelope) => envelope,
        Err(_) if status == 401 => return Err(TgError::Unauthorized),
        Err(_) => return Err(TgError::Network(format!("HTTP {status}"))),
    };
    if envelope.ok {
        return Ok(envelope.result.unwrap_or(Value::Null));
    }
    let code = envelope.error_code.unwrap_or(i64::from(status));
    let text = envelope.description.unwrap_or_default();
    Err(match code {
        401 => TgError::Unauthorized,
        409 => TgError::Conflict,
        429 => TgError::Limited(
            envelope
                .parameters
                .and_then(|parameters| parameters.retry_after)
                .unwrap_or(1),
        ),
        _ => TgError::Refused(code, text),
    })
}

/// A result as a type: a shape Telegram didn't promise is a network error.
pub fn result<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, TgError> {
    serde_json::from_value(value)
        .map_err(|err| TgError::Network(format!("Unexpected answer: {err}")))
}

// --- What the bot reads ---

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Update {
    pub update_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_message: Option<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_query: Option<CallbackQuery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_message_generation: Option<StoppedGeneration>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

impl User {
    /// How the settings and the bot name the person: "Egor Ageev (@egor)".
    pub fn display(&self) -> String {
        let mut name = self.first_name.clone();
        if let Some(last) = self.last_name.as_deref().filter(|last| !last.is_empty()) {
            name = format!("{name} {last}");
        }
        match self.username.as_deref() {
            Some(username) if name.is_empty() => format!("@{username}"),
            Some(username) => format!("{name} (@{username})"),
            None => name,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Chat {
    pub id: i64,
    #[serde(default, rename = "type")]
    pub kind: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Message {
    pub message_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_topic_message: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<User>,
    pub chat: Chat,
    #[serde(default)]
    pub date: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// The service message of a new topic: the user's message outside topics, or the bot's
    /// `createForumTopic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forum_topic_created: Option<TopicCreated>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forum_topic_edited: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_message: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<Vec<PhotoSize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<Document>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message: Option<Box<Message>>,
}

impl Message {
    /// The topic the message is in; none — outside topics.
    pub fn thread(&self) -> Option<i64> {
        self.message_thread_id
    }

    pub fn is_from_bot(&self) -> bool {
        self.from.as_ref().is_some_and(|from| from.is_bot)
    }

    pub fn sender(&self) -> Option<i64> {
        self.from.as_ref().map(|from| from.id)
    }

    /// The text, or a photo's or a file's caption; empty for a service message.
    pub fn words(&self) -> &str {
        self.text
            .as_deref()
            .or(self.caption.as_deref())
            .unwrap_or_default()
    }

    /// A service message (a topic created or renamed, a pin): not the user's words.
    pub fn is_service(&self) -> bool {
        self.forum_topic_created.is_some()
            || self.forum_topic_edited.is_some()
            || self.pinned_message.is_some()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct TopicCreated {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<i64>,
    /// Telegram named it (the user's message outside topics): the bot renames it.
    #[serde(default)]
    pub is_name_implicit: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct PhotoSize {
    pub file_id: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Document {
    pub file_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// The user pressed «Stop» under a draft.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct StoppedGeneration {
    pub chat: Chat,
    /// Not always there: find the draft by its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    /// Sent as a number, comes back as a string.
    #[serde(deserialize_with = "number_or_string")]
    pub draft_id: i64,
}

fn number_or_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("not an integer")),
        Value::String(text) => text.trim().parse().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom("not a number")),
    }
}

/// The bot itself (`getMe`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Me {
    pub id: i64,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub username: String,
    /// Topics in private chats, turned on in @BotFather.
    #[serde(default)]
    pub has_topics_enabled: bool,
    #[serde(default)]
    pub allows_users_to_create_topics: bool,
}

/// A topic the bot created (`createForumTopic`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ForumTopic {
    pub message_thread_id: i64,
    pub name: String,
}

// --- What the bot sends ---

/// A button under a message: a press comes back as a callback query with `callback_data`
/// (at most 64 bytes — [`crate::keys`]), or a link.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Button {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// "success" (green), "danger" (red), "primary".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

impl Button {
    pub fn press(text: &str, data: String) -> Self {
        Button {
            text: text.into(),
            callback_data: Some(data),
            ..Default::default()
        }
    }

    pub fn green(mut self) -> Self {
        self.style = Some("success".into());
        self
    }

    pub fn red(mut self) -> Self {
        self.style = Some("danger".into());
        self
    }

    pub fn primary(mut self) -> Self {
        self.style = Some("primary".into());
        self
    }
}

/// Rows of buttons.
pub type Keyboard = Vec<Vec<Button>>;

fn markup(keyboard: &Keyboard) -> Value {
    json!({ "inline_keyboard": keyboard })
}

/// The colors Telegram allows for a topic's icon.
pub const COLORS: [i64; 6] = [0x6FB9F0, 0xFFD67E, 0xCB86DB, 0x8EEE98, 0xFF93B2, 0xFB6F5F];

/// A call: the method and its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub method: &'static str,
    pub params: Value,
}

impl Call {
    fn new(method: &'static str, params: Value) -> Self {
        Call { method, params }
    }

    fn with(mut self, key: &str, value: Value) -> Self {
        if let Value::Object(map) = &mut self.params {
            map.insert(key.into(), value);
        }
        self
    }

    fn in_thread(self, thread: Option<i64>) -> Self {
        match thread {
            Some(thread) => self.with("message_thread_id", json!(thread)),
            None => self,
        }
    }

    fn with_keyboard(self, keyboard: Option<&Keyboard>) -> Self {
        match keyboard {
            Some(keyboard) => self.with("reply_markup", markup(keyboard)),
            None => self,
        }
    }
}

pub fn get_me() -> Call {
    Call::new("getMe", json!({}))
}

/// News since `offset`, waiting up to [`POLL_SECONDS`] for them.
pub fn get_updates(offset: Option<i64>, timeout: u32) -> Call {
    let call = Call::new(
        "getUpdates",
        json!({
            "timeout": timeout,
            "allowed_updates": ["message", "edited_message", "callback_query", "stopped_message_generation"],
        }),
    );
    match offset {
        Some(offset) => call.with("offset", json!(offset)),
        None => call,
    }
}

/// A message in HTML (`parse_mode`: `<b>`, `<i>`, `<code>`, `<pre>`, `<a>`, `<blockquote
/// expandable>`), links without previews.
pub fn send_message(
    chat: i64,
    thread: Option<i64>,
    html: &str,
    keyboard: Option<&Keyboard>,
) -> Call {
    Call::new(
        "sendMessage",
        json!({
            "chat_id": chat,
            "text": html,
            "parse_mode": "HTML",
            "link_preview_options": { "is_disabled": true },
        }),
    )
    .in_thread(thread)
    .with_keyboard(keyboard)
}

/// A rich message (Bot API 10.1): Markdown — tables, code, `<details>` — up to 32 768 characters.
pub fn send_rich(
    chat: i64,
    thread: Option<i64>,
    markdown: &str,
    keyboard: Option<&Keyboard>,
) -> Call {
    Call::new(
        "sendRichMessage",
        json!({ "chat_id": chat, "rich_message": { "markdown": markdown } }),
    )
    .in_thread(thread)
    .with_keyboard(keyboard)
}

/// A draft as it is written (up to 4096 characters, HTML; empty — «Thinking…»); `can_stop` gives the
/// user «Stop». A message sent to the topic ends it.
pub fn send_draft(chat: i64, thread: Option<i64>, draft: i64, html: &str, can_stop: bool) -> Call {
    let call = Call::new(
        "sendMessageDraft",
        json!({ "chat_id": chat, "draft_id": draft, "text": html, "can_stop": can_stop }),
    )
    .in_thread(thread);
    if html.is_empty() {
        call
    } else {
        call.with("parse_mode", json!("HTML"))
    }
}

pub fn edit_text(chat: i64, message: i64, html: &str, keyboard: Option<&Keyboard>) -> Call {
    Call::new(
        "editMessageText",
        json!({
            "chat_id": chat,
            "message_id": message,
            "text": html,
            "parse_mode": "HTML",
            "link_preview_options": { "is_disabled": true },
        }),
    )
    .with_keyboard(keyboard)
}

/// New buttons under a message; none — no buttons.
pub fn edit_keyboard(chat: i64, message: i64, keyboard: Option<&Keyboard>) -> Call {
    let empty = Keyboard::new();
    Call::new(
        "editMessageReplyMarkup",
        json!({ "chat_id": chat, "message_id": message }),
    )
    .with_keyboard(Some(keyboard.unwrap_or(&empty)))
}

/// The answer to a button's press: at once, or the button spins and the answer comes too late.
pub fn answer_press(query: &str, text: Option<&str>) -> Call {
    let call = Call::new("answerCallbackQuery", json!({ "callback_query_id": query }));
    match text {
        Some(text) => call.with("text", json!(text)),
        None => call,
    }
}

pub fn create_topic(chat: i64, name: &str, color: i64) -> Call {
    Call::new(
        "createForumTopic",
        json!({ "chat_id": chat, "name": topic_name(name), "icon_color": color }),
    )
}

pub fn rename_topic(chat: i64, thread: i64, name: &str) -> Call {
    Call::new(
        "editForumTopic",
        json!({ "chat_id": chat, "message_thread_id": thread, "name": topic_name(name) }),
    )
}

pub fn delete_topic(chat: i64, thread: i64) -> Call {
    Call::new(
        "deleteForumTopic",
        json!({ "chat_id": chat, "message_thread_id": thread }),
    )
}

/// The bot's commands in the «/» menu: (command, description). Part 3.
#[allow(dead_code)]
pub fn set_commands(commands: &[(&str, &str)]) -> Call {
    let commands: Vec<Value> = commands
        .iter()
        .map(|(command, description)| json!({ "command": command, "description": description }))
        .collect();
    Call::new("setMyCommands", json!({ "commands": commands }))
}

/// A topic's name: 1–128 characters.
pub fn topic_name(name: &str) -> String {
    let name = name.trim();
    let name = if name.is_empty() { "…" } else { name };
    name.chars().take(128).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_and_errors() {
        assert_eq!(
            parse(200, br#"{"ok":true,"result":{"a":1}}"#),
            Ok(json!({"a":1}))
        );
        assert_eq!(
            parse(429, br#"{"ok":false,"error_code":429,"description":"Too Many Requests: retry after 7","parameters":{"retry_after":7}}"#),
            Err(TgError::Limited(7))
        );
        assert_eq!(
            parse(409, br#"{"ok":false,"error_code":409,"description":"Conflict: terminated by other getUpdates request"}"#),
            Err(TgError::Conflict)
        );
        assert_eq!(parse(401, b"Unauthorized"), Err(TgError::Unauthorized));
        assert_eq!(
            parse(502, b"<html>"),
            Err(TgError::Network("HTTP 502".into()))
        );
        let gone = parse(
            400,
            br#"{"ok":false,"error_code":400,"description":"Bad Request: TOPIC_ID_INVALID"}"#,
        );
        assert!(gone.unwrap_err().topic_gone());
    }

    #[test]
    fn a_stop_names_its_draft_by_a_string() {
        let update: Update = serde_json::from_str(
            r#"{"update_id":5,"stopped_message_generation":{"chat":{"id":1,"type":"private"},"draft_id":"112701"}}"#,
        )
        .unwrap();
        assert_eq!(update.stopped_message_generation.unwrap().draft_id, 112701);
    }

    #[test]
    fn a_new_chat_is_an_implicit_topic() {
        let update: Update = serde_json::from_str(
            r#"{"update_id":6,"message":{"message_id":44,"message_thread_id":1407099,"is_topic_message":true,
            "from":{"id":1,"is_bot":false,"first_name":"Egor"},"chat":{"id":1,"type":"private"},"date":1,
            "forum_topic_created":{"name":"ПТема 1","icon_color":7322096,"is_name_implicit":true}}}"#,
        )
        .unwrap();
        let message = update.message.unwrap();
        assert!(message.is_service());
        assert_eq!(message.thread(), Some(1407099));
        assert!(message.forum_topic_created.unwrap().is_name_implicit);
    }

    #[test]
    fn calls_carry_their_topic_and_buttons() {
        let keyboard = vec![vec![Button::press("Allow", "r:1:a".into()).green()]];
        let call = send_message(1, Some(7), "<b>x</b>", Some(&keyboard));
        assert_eq!(call.method, "sendMessage");
        assert_eq!(call.params["message_thread_id"], 7);
        assert_eq!(
            call.params["reply_markup"]["inline_keyboard"][0][0]["style"],
            "success"
        );
        let draft = send_draft(1, Some(7), 9, "", true);
        assert!(draft.params.get("parse_mode").is_none());
        assert_eq!(
            edit_keyboard(1, 2, None).params["reply_markup"],
            json!({"inline_keyboard": []})
        );
        assert_eq!(topic_name("  "), "…");
    }
}
