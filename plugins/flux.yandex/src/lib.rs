//! Yandex Tracker for Flux (`flux.yandex`), part 10.3; the calendar left it on 2026-10-28 (a
//! plugin of its own later).
//!
//! - [`auth`] — the sign-in: Yandex ID's OAuth with PKCE through the browser and the plugin's
//!   server on 127.0.0.1 (fixed ports — Yandex checks the redirect's port), tokens in Flux's
//!   secrets; a pasted OAuth or IAM token instead; the account's line on the settings page.
//! - [`tracker`] — the Tracker window on the right (lists of tasks; a task's card with its fields,
//!   links, comments and history, and a button per Claude action of the settings), taking a task to
//!   work (the status, the branch, the task's tabs, the timer — as the settings say — then Claude
//!   gets the action's prompt filled from the card), the active task in the status bar, Close Active
//!   Task (the work log), Open Task (⌥⇧N), links to tasks, news by polling.
//! - [`board`] — the board of tasks in an editor tab: Tracker's boards, their columns, the cards; a
//!   chosen card opens in the Tracker window, the favorite action's button takes it to work.
//! - [`cache`] — what the windows show, in files: a start shows it at once, before any request;
//!   the project windows share it.
//! - [`lead`] — one window polls Tracker for all of them (Flux runs the plugin once per window).
//!
//! Every event goes to the part that owns its id (a timer, a request, a dialog, a picker); the
//! window's events go to the Tracker.

mod auth;
mod board;
mod cache;
mod client;
mod lead;
mod pkce;
mod settings;
mod text;
mod time;
mod tracker;

use flux_plugin_api::{CommandContext, Event, Plugin, register_plugin, storage, timers};

use auth::Auth;
use board::Board;
use tracker::Tracker;

/// The storage keys of the calendar, a plugin of its own now: removed once.
const OLD_KEYS: [&str; 2] = ["calendar.calendars", "calendar.reminded"];

/// What one part asks of another: the board shows a card in the Tracker window, takes a task to
/// work with the favorite action.
pub enum Intent {
    /// The task's card in the Tracker window (shown).
    OpenCard(String),
    /// The favorite action of the settings on the task.
    RunFavorite(String),
}

struct Yandex {
    auth: Auth,
    tracker: Tracker,
    board: Board,
    /// The tick of the poller's lease ([`lead`]) and the cache's files.
    tick: Option<u64>,
}

impl Plugin for Yandex {
    fn new() -> Self {
        Yandex {
            auth: Auth::new(),
            tracker: Tracker::new(),
            board: Board::new(),
            tick: None,
        }
    }

    fn activate(&mut self) {
        self.auth.activate();
        lead::start(time::now());
        self.tick = Some(timers::every(lead::TICK));
        self.tracker.activate(&self.auth);
        self.board.activate(&self.auth);
        for key in OLD_KEYS {
            if storage::get::<serde_json::Value>(key).is_some() {
                storage::remove(key);
            }
        }
    }

    fn deactivate(&mut self) {
        if let Some(tick) = self.tick.take() {
            timers::cancel(tick);
        }
        lead::release();
        self.tracker.deactivate();
        self.board.deactivate();
        self.auth.deactivate();
    }

    fn run_command(&mut self, command: &str, context: &CommandContext) {
        match command {
            auth::SIGN_IN | auth::SIGN_OUT | auth::CHECK_CONNECTION => {
                self.auth.run_command(command, context)
            }
            board::OPEN_BOARD => {
                let intents = self.board.run_command(command, context, &self.auth);
                self.carry_out(intents);
            }
            _ => self.tracker.run_command(command, context, &self.auth),
        }
        self.after();
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::SettingsChanged => {
                self.auth.settings_changed();
                self.tracker.settings_changed(&self.auth);
                self.board.settings_changed(&self.auth);
            }
            Event::Timer(id) if self.tick == Some(id) => {
                let leading = lead::tick(time::now());
                self.tracker.tick(leading, &self.auth);
            }
            event if self.auth.claims(&event) => self.auth.on_event(event),
            event if self.board.claims(&event) => {
                let intents = self.board.on_event(event, &self.auth);
                self.carry_out(intents);
            }
            event => self.tracker.on_event(event, &self.auth),
        }
        self.after();
    }
}

impl Yandex {
    /// After every call: a sign-in or a sign-out, a new organization reaches the parts that use
    /// them; the board learns the favorite action's name and the tasks the Tracker changed.
    fn after(&mut self) {
        if self.auth.take_changed() {
            self.tracker.auth_changed(&self.auth);
            self.board.auth_changed(&self.auth);
        }
        let changed = self.tracker.take_changed_tasks();
        self.board
            .tracker_changed(&self.tracker.favorite_action_name(), &changed, &self.auth);
    }

    fn carry_out(&mut self, intents: Vec<Intent>) {
        for intent in intents {
            match intent {
                Intent::OpenCard(key) => self.tracker.open_card(&key, &self.auth),
                Intent::RunFavorite(key) => self.tracker.run_favorite(&key, &self.auth),
            }
        }
    }
}

register_plugin!(Yandex);
