//! Yandex Tracker and Calendar for Flux (`flux.yandex`), part 10.3.
//!
//! - [`auth`] — the sign-in: Yandex ID's OAuth with PKCE through the browser and the plugin's
//!   server on 127.0.0.1 (fixed ports — Yandex checks the redirect's port), tokens in the keychain;
//!   a pasted OAuth or IAM token instead; the account's line on the settings page.
//! - [`tracker`] — the Tracker window on the right (lists of tasks; a task's card with its fields,
//!   links, comments and history, and a button per Claude action of the settings), taking a task to
//!   work (the status, the branch, the task's tabs, the timer — as the settings say — then Claude
//!   gets the action's prompt filled from the card), the active task in the status bar, Close Active
//!   Task (the work log), Open Task (⌥⇧N), links to tasks, news by polling.
//! - [`board`] — the board of tasks in an editor tab: Tracker's boards, their columns, the cards; a
//!   chosen card opens in the Tracker window, the favorite action's button takes it to work.
//! - [`calendar`] — CalDAV: the next meeting in the status bar, the day's agenda in a popup, a
//!   reminder with Join (Telemost).
//!
//! Every event goes to the part that owns its id (a timer, a request, a dialog, a picker); the
//! window's events go to the Tracker.

mod auth;
mod board;
mod calendar;
mod client;
mod pkce;
mod settings;
mod text;
mod time;
mod tracker;

use flux_plugin_api::{CommandContext, Event, Plugin, register_plugin};

use auth::Auth;
use board::Board;
use calendar::Calendar;
use tracker::Tracker;

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
    calendar: Calendar,
}

impl Plugin for Yandex {
    fn new() -> Self {
        Yandex {
            auth: Auth::new(),
            tracker: Tracker::new(),
            board: Board::new(),
            calendar: Calendar::new(),
        }
    }

    fn activate(&mut self) {
        self.auth.activate();
        self.tracker.activate(&self.auth);
        self.board.activate(&self.auth);
        self.calendar.activate(&self.auth);
    }

    fn deactivate(&mut self) {
        self.tracker.deactivate();
        self.board.deactivate();
        self.calendar.deactivate();
        self.auth.deactivate();
    }

    fn run_command(&mut self, command: &str, context: &CommandContext) {
        match command {
            auth::SIGN_IN | auth::SIGN_OUT | auth::CHECK_CONNECTION => {
                self.auth.run_command(command, context)
            }
            command if command.starts_with(calendar::COMMAND_PREFIX) => {
                self.calendar.run_command(command, context, &self.auth)
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
                self.calendar.settings_changed(&self.auth);
            }
            event if self.auth.claims(&event) => self.auth.on_event(event),
            event if self.calendar.claims(&event) => self.calendar.on_event(event, &self.auth),
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
            self.calendar.auth_changed(&self.auth);
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
