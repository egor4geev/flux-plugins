//! The service's bot: the state for the whole app and the routing — Telegram's updates and
//! answers, Claude's events, the timers and the windows' messages go to the module that owns them.
//! Nothing here calls the SDK: everything goes through [`Host`], so the whole bot runs in native
//! tests ([`crate::testing`]).
//!
//! - [`crate::binding`] — the token, `getMe`, the one-time code, the owner; the connection's
//!   state in the settings;
//! - [`crate::general`] — «Общее»: the topic, the status message of every session;
//! - [`crate::handoff`] — «Продолжить в Telegram» (the Claude console's action) and «Продолжить
//!   здесь»: a topic for a session that has none;
//! - [`crate::awake`] — keeping the computer awake;
//! - [`crate::newchat`] — the user's message outside topics → the project → a new session in its
//!   topic;
//! - [`crate::conversation`] — a session's topic both ways: the user's messages, Claude's answer as
//!   a draft, the steps, the turn's end, «Stop»;
//! - [`crate::requests`] — permissions, Claude's questions and the plan as buttons; while the Mac
//!   is locked, every session's in «Общее».
//!
//! The bot answers its owner only: anything from anyone else is dropped unread.

use std::collections::BTreeMap;

use flux_plugin_api::remote::{
    RegionState, Request, Session, Steps, SystemEvent, SystemState, TextUpdate, TurnSummary,
    UsageLimits,
};
use flux_plugin_api::settings::Tone;
use serde_json::Value;

use crate::host::Host;
use crate::keys::{self, Kind};
use crate::model::{Connection, ToService, ToWindow};
use crate::store::Store;
use crate::telegram::{self, Call, CallbackQuery, Me, Message, TgError, Update};
use crate::transport::{Incoming, Lane, Module, Purpose, Transport};
use crate::{awake, binding, conversation, general, handoff, newchat, requests};

/// Claude's events, as the SDK brings them (`lib.rs` turns `Event`s into these).
#[derive(Clone, Debug)]
pub enum ClaudeEvent {
    Session(Session),
    Gone(String),
    Text(TextUpdate),
    Steps(String, Steps),
    Request(String, Request),
    Resolved(String, String),
    TurnEnded(String, TurnSummary),
    Failed(String, String),
    // Part 3: `/usage`, the country's block.
    #[allow(dead_code)]
    Limits(UsageLimits),
    #[allow(dead_code)]
    Region(RegionState),
    System(SystemEvent),
    /// The plugin's action in the Claude console: (the action's id, the session).
    Action(String, String),
}

pub struct Bot {
    pub tg: Transport,
    pub store: Store,
    /// The bot itself, once `getMe` answered.
    pub me: Option<Me>,
    pub connection: Connection,
    /// Every session of Flux by id, the latest of each.
    pub sessions: BTreeMap<String, Session>,
    pub system: SystemState,
    pub binding: binding::State,
    pub general: general::State,
    pub handoff: handoff::State,
    pub awake: awake::State,
    pub newchat: newchat::State,
    pub conversation: conversation::State,
    pub requests: requests::State,
}

impl Bot {
    pub fn new(host: &dyn Host) -> Self {
        Bot {
            tg: Transport::new(),
            store: Store::load(host),
            me: None,
            connection: Connection::NoToken,
            sessions: BTreeMap::new(),
            system: SystemState {
                screen_locked: false,
                on_power: true,
            },
            binding: Default::default(),
            general: Default::default(),
            handoff: Default::default(),
            awake: Default::default(),
            newchat: Default::default(),
            conversation: Default::default(),
            requests: Default::default(),
        }
    }

    /// The service started: watch Claude, read the sessions and the computer, connect the bot.
    pub fn start(&mut self, host: &mut dyn Host) {
        if let Err(err) = host.watch(true) {
            host.log(&format!("Can't watch Claude's sessions: {err}"));
        }
        if let Ok(sessions) = host.sessions() {
            self.sessions = sessions
                .into_iter()
                .map(|session| (session.id.clone(), session))
                .collect();
        }
        if let Ok(system) = host.system_status() {
            self.system = system;
        }
        binding::start(self, host);
        awake::update(self, host);
    }

    /// The owner's private chat; none — not bound yet.
    pub fn chat(&self) -> Option<i64> {
        self.store.chat()
    }

    /// The bot's name for the settings: "@flux_bot".
    pub fn bot_name(&self) -> String {
        self.me
            .as_ref()
            .map(|me| format!("@{}", me.username))
            .unwrap_or_default()
    }

    pub fn call(&mut self, host: &mut dyn Host, call: Call, lane: Lane, purpose: Purpose) {
        self.tg.send(host, call, lane, purpose);
    }

    /// Answers a button's press at once (a short note over the chat, or nothing): the module that
    /// handles the press calls it first.
    pub fn answer_press(&mut self, host: &mut dyn Host, query: &CallbackQuery, text: Option<&str>) {
        self.call(
            host,
            telegram::answer_press(&query.id, text),
            Lane::Urgent,
            Purpose::none(),
        );
    }

    pub fn save(&mut self, host: &mut dyn Host) {
        self.store.offset = self.tg.offset().or(self.store.offset);
        self.store.save(host);
    }

    /// The connection changed: the windows and the settings' status line hear of it.
    pub fn set_connection(&mut self, host: &mut dyn Host, connection: Connection) {
        if self.connection == connection {
            return;
        }
        self.connection = connection.clone();
        host.broadcast(&ToWindow::Connection(connection.clone()));
        let line = binding::status_line(host, &connection);
        match &line {
            Some((text, tone, busy)) => host.status(binding::STATUS, Some((text, *tone, *busy))),
            None => host.status(binding::STATUS, None),
        }
    }

    pub fn settings_changed(&mut self, host: &mut dyn Host) {
        binding::settings_changed(self, host);
        awake::update(self, host);
    }

    pub fn on_window(&mut self, host: &mut dyn Host, window: Option<u64>, message: ToService) {
        binding::on_window(self, host, window, &message);
    }

    // --- Telegram ---

    pub fn on_http_head(&mut self, request: u64, status: u16) {
        self.tg.on_head(request, status);
    }

    pub fn on_http_body(&mut self, request: u64, chunk: &[u8]) {
        self.tg.on_body(request, chunk);
    }

    pub fn on_http_done(&mut self, host: &mut dyn Host, request: u64, error: Option<String>) {
        let incoming = self.tg.on_done(host, request, error);
        self.incoming(host, incoming);
    }

    fn incoming(&mut self, host: &mut dyn Host, incoming: Vec<Incoming>) {
        for item in incoming {
            match item {
                Incoming::Updates(updates) => {
                    binding::on_polled(self, host, None);
                    let had = !updates.is_empty();
                    for update in updates {
                        self.on_update(host, update);
                    }
                    if had {
                        self.save(host);
                    }
                }
                Incoming::PollFailed(error) => binding::on_polled(self, host, Some(error)),
                Incoming::Answer(purpose, result) => self.on_answer(host, purpose, result),
            }
        }
    }

    pub fn on_update(&mut self, host: &mut dyn Host, update: Update) {
        if let Some(stopped) = update.stopped_message_generation {
            if self.chat() == Some(stopped.chat.id) {
                conversation::on_stop(self, host, &stopped);
            }
        } else if let Some(query) = update.callback_query {
            self.on_press(host, query);
        } else if let Some(message) = update.message {
            self.on_message(host, message);
        }
    }

    fn on_press(&mut self, host: &mut dyn Host, query: CallbackQuery) {
        if self.chat().is_none() || self.chat() != Some(query.from.id) {
            self.answer_press(host, &query, None);
            return;
        }
        let Some((kind, key, choice)) = query.data.as_deref().and_then(keys::decode) else {
            self.answer_press(host, &query, None);
            return;
        };
        match kind {
            Kind::Request => requests::on_press(self, host, &query, key, &choice),
            Kind::NewChat => newchat::on_press(self, host, &query, key, &choice),
            Kind::General => general::on_press(self, host, &query, key, &choice),
            Kind::Handoff => handoff::on_press(self, host, &query, key, &choice),
            Kind::Noop => self.answer_press(host, &query, None),
        }
    }

    fn on_message(&mut self, host: &mut dyn Host, message: Message) {
        // The bot's own service messages (its topics, its pins) come back too.
        if message.is_from_bot() {
            return;
        }
        if binding::on_message(self, host, &message) {
            return;
        }
        let Some(chat) = self.chat() else {
            return;
        };
        if message.chat.id != chat || message.sender() != Some(chat) {
            return;
        }
        if message.forum_topic_created.is_some() {
            newchat::on_topic_created(self, host, &message);
            return;
        }
        if message.is_service() {
            return;
        }
        let thread = message.thread();
        if thread.is_some() && thread == self.store.general {
            general::on_message(self, host, &message);
        } else if requests::on_user_message(self, host, &message) {
            // The words of «Other…» or of the plan's «Keep planning».
        } else if let Some(topic) = thread.and_then(|thread| self.store.topic_by_thread(thread)) {
            let session = topic.session.clone();
            conversation::on_user_message(self, host, &message, &session);
        } else {
            newchat::on_message(self, host, &message);
        }
    }

    fn on_answer(&mut self, host: &mut dyn Host, purpose: Purpose, result: Result<Value, TgError>) {
        match purpose.module {
            Module::None => {
                if let Err(err) = &result
                    && !err.not_modified()
                    && !err.query_too_old()
                {
                    host.log(&format!("Telegram refused a call: {err:?}"));
                }
            }
            Module::Binding => binding::on_answer(self, host, &purpose.tag, result),
            Module::General => general::on_answer(self, host, &purpose.tag, result),
            Module::Handoff => handoff::on_answer(self, host, &purpose.tag, result),
            Module::NewChat => newchat::on_answer(self, host, &purpose.tag, result),
            Module::Conversation => conversation::on_answer(self, host, &purpose.tag, result),
            Module::Requests => requests::on_answer(self, host, &purpose.tag, result),
        }
    }

    // --- Claude ---

    pub fn on_claude(&mut self, host: &mut dyn Host, event: ClaudeEvent) {
        match event {
            ClaudeEvent::Session(session) => {
                self.sessions.insert(session.id.clone(), session.clone());
                newchat::on_session(self, host, &session);
                conversation::on_session(self, host, &session);
                general::sessions_changed(self, host);
                awake::update(self, host);
            }
            ClaudeEvent::Gone(session) => {
                self.sessions.remove(&session);
                conversation::on_gone(self, host, &session);
                general::sessions_changed(self, host);
                awake::update(self, host);
            }
            ClaudeEvent::Text(update) => conversation::on_text(self, host, &update),
            ClaudeEvent::Steps(session, steps) => {
                conversation::on_steps(self, host, &session, &steps)
            }
            ClaudeEvent::Request(session, request) => {
                requests::on_request(self, host, &session, &request)
            }
            ClaudeEvent::Resolved(session, request) => {
                requests::on_resolved(self, host, &session, &request)
            }
            ClaudeEvent::TurnEnded(session, summary) => {
                conversation::on_turn_end(self, host, &session, &summary);
                general::sessions_changed(self, host);
            }
            ClaudeEvent::Failed(session, error) => {
                conversation::on_failed(self, host, &session, &error)
            }
            // Part 3: `/usage`, the country's block.
            ClaudeEvent::Limits(_) | ClaudeEvent::Region(_) => {}
            ClaudeEvent::System(event) => {
                let locked = self.system.screen_locked;
                match event {
                    SystemEvent::ScreenLocked => self.system.screen_locked = true,
                    SystemEvent::ScreenUnlocked => self.system.screen_locked = false,
                    SystemEvent::OnPower => self.system.on_power = true,
                    SystemEvent::OnBattery => self.system.on_power = false,
                    // Part 3: «going to sleep» / «awake» in «Общее».
                    SystemEvent::WillSleep | SystemEvent::DidWake => {}
                }
                if locked != self.system.screen_locked {
                    requests::on_lock_changed(self, host);
                    general::sessions_changed(self, host);
                }
                awake::update(self, host);
            }
            ClaudeEvent::Action(action, session) => {
                if action == handoff::ACTION {
                    handoff::continue_in_telegram(self, host, &session);
                }
            }
        }
    }

    pub fn on_timer(&mut self, host: &mut dyn Host, timer: u64) {
        if self.tg.on_timer(host, timer) {
            return;
        }
        let _ = binding::on_timer(self, host, timer)
            || general::on_timer(self, host, timer)
            || conversation::on_timer(self, host, timer);
    }
}

/// A status line of the settings page: (text, tone, busy).
pub type StatusLine = (String, Tone, bool);
