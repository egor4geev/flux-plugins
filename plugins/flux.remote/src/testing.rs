//! A fake Flux for the native tests: [`FakeHost`] records what the bot asks for — Telegram's calls
//! (their method and JSON), Claude's calls as readable lines, timers, the storage — and answers
//! from what the test put in. [`reply`] finishes a Telegram call as Telegram would; [`settle`]
//! answers every call in flight.

use std::collections::BTreeMap;

use flux_plugin_api::remote::{
    Awake, Mode, Project, Response, Session, SessionState, SystemState, TranscriptItem,
};
use flux_plugin_api::settings::Tone;
use serde_json::{Value, json};

use crate::bot::Bot;
use crate::host::{Host, NewSession};
use crate::model::ToWindow;

pub struct FakeHost {
    next: u64,
    /// Started requests: (id, URL, body).
    pub http: Vec<(u64, String, Vec<u8>)>,
    pub cancelled: Vec<u64>,
    /// Started timers: (id, ms).
    pub timers: Vec<(u64, u32)>,
    pub now: u64,
    pub storage: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, String>,
    pub settings: BTreeMap<String, Value>,
    pub statuses: BTreeMap<String, String>,
    pub logs: Vec<String>,
    /// Flux's notifications: their titles.
    pub notices: Vec<String>,
    pub windows: Vec<ToWindow>,
    /// What `sessions()` returns.
    pub sessions: Vec<Session>,
    pub projects: Vec<Project>,
    pub transcript: Vec<TranscriptItem>,
    pub system: SystemState,
    /// Claude's calls, as lines: "send <session> <text>", "answer <session> <request> <response>",
    /// "interrupt <session>", "mode <session> <mode>", "new <project> <prompt>", "awake <mode>".
    pub claude: Vec<String>,
    /// The next new session's id.
    pub new_session_id: String,
    /// The next Claude call fails with this.
    pub fail: Option<String>,
}

impl Default for FakeHost {
    fn default() -> Self {
        FakeHost {
            next: 0,
            http: Vec::new(),
            cancelled: Vec::new(),
            timers: Vec::new(),
            now: 1_795_000_000_000,
            storage: BTreeMap::new(),
            secrets: BTreeMap::new(),
            settings: BTreeMap::new(),
            statuses: BTreeMap::new(),
            logs: Vec::new(),
            notices: Vec::new(),
            windows: Vec::new(),
            sessions: Vec::new(),
            projects: Vec::new(),
            transcript: Vec::new(),
            system: SystemState {
                screen_locked: false,
                on_power: true,
            },
            claude: Vec::new(),
            new_session_id: "11111111-2222-3333-4444-555555555555".into(),
            fail: None,
        }
    }
}

impl FakeHost {
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn claude_call(&mut self, line: String) -> Result<(), String> {
        self.claude.push(line);
        match self.fail.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Telegram's calls so far: (request id, method, parameters).
    pub fn tg_calls(&self) -> Vec<(u64, String, Value)> {
        self.http
            .iter()
            .map(|(id, url, body)| {
                let method = url.rsplit('/').next().unwrap_or_default().to_string();
                (
                    *id,
                    method,
                    serde_json::from_slice(body).unwrap_or(Value::Null),
                )
            })
            .collect()
    }

    /// The last call of a method: (request id, parameters).
    pub fn last_call(&self, method: &str) -> Option<(u64, Value)> {
        self.tg_calls()
            .into_iter()
            .rev()
            .find(|(_, name, _)| name == method)
            .map(|(id, _, params)| (id, params))
    }
}

/// Finishes a request with Telegram's `ok` and this result.
pub fn reply(bot: &mut Bot, host: &mut FakeHost, request: u64, result: Value) {
    let body = serde_json::to_vec(&json!({ "ok": true, "result": result })).unwrap();
    bot.on_http_head(request, 200);
    bot.on_http_body(request, &body);
    bot.on_http_done(host, request, None);
}

/// Answers every Telegram call in flight but the poll, as Telegram would — `getMe` with a bot whose
/// topics are on, a topic (threads 500, 501… in the order they were asked for), a message (its id is
/// 600 + the request's), `true` — until none is left. Timers aren't fired.
pub fn settle(bot: &mut Bot, host: &mut FakeHost) {
    let mut answered = std::collections::BTreeSet::new();
    loop {
        let calls = host.tg_calls();
        let open = calls
            .iter()
            .find(|(id, method, _)| method != "getUpdates" && !answered.contains(id));
        let Some((id, method, params)) = open.cloned() else {
            return;
        };
        answered.insert(id);
        let result = match method.as_str() {
            "getMe" => json!({
                "id": 77, "is_bot": true, "first_name": "Flux", "username": "flux_bot",
                "has_topics_enabled": true
            }),
            "createForumTopic" => {
                let before = calls
                    .iter()
                    .filter(|(other, method, _)| method == "createForumTopic" && *other < id)
                    .count();
                json!({ "message_thread_id": 500 + before as i64, "name": params["name"] })
            }
            "sendMessage" | "sendRichMessage" | "editMessageText" => json!({
                "message_id": 600 + id as i64,
                "chat": { "id": params["chat_id"], "type": "private" },
            }),
            _ => json!(true),
        };
        reply(bot, host, id, result);
    }
}

/// A session as Flux tells of it.
pub fn session(id: &str, project: &str, state: SessionState) -> Session {
    Session {
        id: id.into(),
        project: project.into(),
        title: String::new(),
        state,
        activity: String::new(),
        mode: Mode::Ask,
        window: Some(1),
        updated: 1_795_000_000,
        request: None,
        unseen: false,
    }
}

/// Finishes a request with Telegram's error.
pub fn reply_error(bot: &mut Bot, host: &mut FakeHost, request: u64, code: u16, description: &str) {
    let body =
        serde_json::to_vec(&json!({ "ok": false, "error_code": code, "description": description }))
            .unwrap();
    bot.on_http_head(request, code);
    bot.on_http_body(request, &body);
    bot.on_http_done(host, request, None);
}

impl Host for FakeHost {
    fn http_post(&mut self, url: &str, body: Vec<u8>, _timeout_ms: u32) -> u64 {
        let id = self.id();
        self.http.push((id, url.into(), body));
        id
    }

    fn http_cancel(&mut self, request: u64) {
        self.cancelled.push(request);
    }

    fn timer(&mut self, ms: u32) -> u64 {
        let id = self.id();
        self.timers.push((id, ms));
        id
    }

    fn cancel_timer(&mut self, timer: u64) {
        self.timers.retain(|(id, _)| *id != timer);
    }

    fn now_ms(&self) -> u64 {
        self.now
    }

    fn random(&mut self, bytes: usize) -> Vec<u8> {
        // Another value each call (a new code differs from the old one), the same in every run.
        let salt = self.id() as u8;
        (0..bytes)
            .map(|index| {
                (index as u8)
                    .wrapping_mul(37)
                    .wrapping_add(11)
                    .wrapping_add(salt)
            })
            .collect()
    }

    fn load(&self, key: &str) -> Option<Value> {
        self.storage.get(key).cloned()
    }

    fn save(&mut self, key: &str, value: Option<&Value>) {
        match value {
            Some(value) => self.storage.insert(key.into(), value.clone()),
            None => self.storage.remove(key),
        };
    }

    fn secret(&self, key: &str) -> Option<String> {
        self.secrets.get(key).cloned()
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.settings.get(key).cloned()
    }

    fn status(&mut self, key: &str, line: Option<(&str, Tone, bool)>) {
        match line {
            Some((text, _, _)) => self.statuses.insert(key.into(), text.into()),
            None => self.statuses.remove(key),
        };
    }

    fn tr(&self, text: &str) -> String {
        text.into()
    }

    fn log(&mut self, text: &str) {
        self.logs.push(text.into());
    }

    fn notify(&mut self, title: &str) {
        self.notices.push(title.into());
    }

    fn utc_offset(&self, _at: i64) -> i32 {
        0
    }

    fn broadcast(&mut self, message: &ToWindow) {
        self.windows.push(message.clone());
    }

    fn watch(&mut self, on: bool) -> Result<(), String> {
        self.claude_call(format!("watch {on}"))
    }

    fn sessions(&mut self) -> Result<Vec<Session>, String> {
        Ok(self.sessions.clone())
    }

    fn transcript(&mut self, session: &str, count: u32) -> Result<Vec<TranscriptItem>, String> {
        self.claude_call(format!("transcript {session} {count}"))?;
        Ok(self.transcript.clone())
    }

    fn send(&mut self, session: &str, text: &str, images: &[String]) -> Result<(), String> {
        let images = if images.is_empty() {
            String::new()
        } else {
            format!(" +{}", images.join(","))
        };
        self.claude_call(format!("send {session} {text}{images}"))
    }

    fn answer(&mut self, session: &str, request: &str, response: &Response) -> Result<(), String> {
        self.claude_call(format!("answer {session} {request} {response:?}"))
    }

    fn interrupt(&mut self, session: &str) -> Result<(), String> {
        self.claude_call(format!("interrupt {session}"))
    }

    fn set_mode(&mut self, session: &str, mode: Mode) -> Result<(), String> {
        self.claude_call(format!("mode {session} {mode:?}"))
    }

    fn new_session(&mut self, spec: &NewSession) -> Result<String, String> {
        self.claude_call(format!(
            "new {} {}",
            spec.project,
            spec.prompt.as_deref().unwrap_or_default()
        ))?;
        Ok(self.new_session_id.clone())
    }

    fn projects(&mut self) -> Result<Vec<Project>, String> {
        Ok(self.projects.clone())
    }

    fn set_awake(&mut self, awake: Awake) -> Result<(), String> {
        self.claude_call(format!("awake {awake:?}"))
    }

    fn system_status(&mut self) -> Result<SystemState, String> {
        Ok(self.system)
    }
}

// --- Agent B: a bound bot, Telegram answering in order, the user's updates ---

/// The owner of [`bound_bot`] (and so the private chat).
pub const OWNER: i64 = 5;
/// «Общее» of [`bound_bot`].
pub const GENERAL: i64 = 900;

/// A bot bound to [`OWNER`] with «Общее» made, its calls going (no polling).
pub fn bound_bot(host: &mut FakeHost) -> Bot {
    let mut bot = Bot::new(host);
    bot.store.owner = Some(crate::store::Owner {
        id: OWNER,
        name: "Egor".into(),
    });
    bot.store.general = Some(GENERAL);
    bot.tg.set_token(host, Some("1:T".into()));
    bot
}

/// The transport's pauses [`Telegram::flush`] lets pass (longer ones are the modules': the
/// thinking draft's refresh, the binding code's end).
pub const SHORT_TIMER_MS: u32 = 3_000;

/// Telegram answering the bot's calls one by one, as it would: a message gets a new id.
#[derive(Default)]
pub struct Telegram {
    answered: std::collections::BTreeSet<u64>,
    messages: i64,
}

impl Telegram {
    /// Answers every call until none waits — the transport's own pauses (a draft's pace, a
    /// `Latest` call's, a retry: up to [`SHORT_TIMER_MS`]) pass as their time comes: the calls, as
    /// (method, parameters).
    pub fn flush(&mut self, bot: &mut Bot, host: &mut FakeHost) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        loop {
            let Some((id, method, params)) = host
                .tg_calls()
                .into_iter()
                .find(|(id, method, _)| method != "getUpdates" && !self.answered.contains(id))
            else {
                let Some(&(timer, ms)) = host.timers.iter().find(|(_, ms)| *ms <= SHORT_TIMER_MS)
                else {
                    break;
                };
                host.timers.retain(|(id, _)| *id != timer);
                host.now += u64::from(ms);
                bot.on_timer(host, timer);
                continue;
            };
            self.answered.insert(id);
            let result = match method.as_str() {
                "sendMessage" | "sendRichMessage" | "editMessageText" => {
                    self.messages += 1;
                    let id = params["message_id"].as_i64().unwrap_or(100 + self.messages);
                    json!({ "message_id": id, "chat": { "id": OWNER, "type": "private" },
                            "message_thread_id": params["message_thread_id"] })
                }
                _ => json!(true),
            };
            out.push((method, params));
            reply(bot, host, id, result);
        }
        out
    }

    /// Answers the next call with Telegram's error instead.
    pub fn refuse(&mut self, bot: &mut Bot, host: &mut FakeHost, description: &str) -> String {
        let (id, method, _) = host
            .tg_calls()
            .into_iter()
            .find(|(id, method, _)| method != "getUpdates" && !self.answered.contains(id))
            .expect("a call waits");
        self.answered.insert(id);
        reply_error(bot, host, id, 400, description);
        method
    }
}

/// The owner's message in a topic (none — outside topics).
pub fn user_message(update: i64, thread: Option<i64>, text: &str) -> crate::telegram::Update {
    let mut message = json!({
        "message_id": 10_000 + update, "from": { "id": OWNER, "is_bot": false, "first_name": "Egor" },
        "chat": { "id": OWNER, "type": "private" }, "date": 1, "text": text,
    });
    if let Some(thread) = thread {
        message["message_thread_id"] = json!(thread);
        message["is_topic_message"] = json!(true);
    }
    crate::telegram::Update {
        update_id: update,
        message: Some(serde_json::from_value(message).unwrap()),
        ..Default::default()
    }
}

/// The service message of the topic the user's message outside topics opens.
pub fn topic_created(update: i64, thread: i64, name: &str) -> crate::telegram::Update {
    let message = json!({
        "message_id": 10_000 + update, "message_thread_id": thread, "is_topic_message": true,
        "from": { "id": OWNER, "is_bot": false, "first_name": "Egor" },
        "chat": { "id": OWNER, "type": "private" }, "date": 1,
        "forum_topic_created": { "name": name, "icon_color": 7322096, "is_name_implicit": true },
    });
    crate::telegram::Update {
        update_id: update,
        message: Some(serde_json::from_value(message).unwrap()),
        ..Default::default()
    }
}

/// The owner's press of a button with `data` under the bot's message.
pub fn press(update: i64, message: i64, thread: i64, data: &str) -> crate::telegram::Update {
    let query = json!({
        "id": format!("q{update}"), "from": { "id": OWNER, "is_bot": false, "first_name": "Egor" },
        "message": { "message_id": message, "message_thread_id": thread, "chat": { "id": OWNER, "type": "private" },
                     "from": { "id": 77, "is_bot": true, "first_name": "Flux" } },
        "data": data,
    });
    crate::telegram::Update {
        update_id: update,
        callback_query: Some(serde_json::from_value(query).unwrap()),
        ..Default::default()
    }
}

/// A bound bot with a topic `thread` for `session`.
pub fn bot_with_topic(host: &mut FakeHost, thread: i64, session: &str) -> Bot {
    let mut bot = bound_bot(host);
    bot.store.add_topic(crate::store::Topic {
        thread,
        session: session.into(),
        project: "/work/project".into(),
        title: String::new(),
    });
    bot
}
