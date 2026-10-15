//! Tracker: the window on the right (lists of tasks; a task's card with its fields, links,
//! comments and history, and a button per Claude action of the settings), taking a task to work and
//! the active task, Open Task (⌥⇧N), news by polling, links to tasks.
//!
//! The author's concept (2026-10-14): the card shows links, comments and history; the actions of
//! the `actions` setting are buttons of the card; an action with `start` takes the task to work
//! first in one click — the status, the branch, the task's tabs, the timer, as the `work-*`
//! settings say, no dialog — then Claude gets its prompt filled from the card at once.
//!
//! - [`window`] — the window: what it shows and its input; the filter and status pickers.
//! - [`edit`] — changing the task's fields from its card: pickers under the properties, forms.
//! - [`work`] — the actions, taking to work, the active task and its timer, Close and Log Time.
//! - [`poll`] — news by polling; [`links`] — `TRACK-142` in code, commits and the chat.
//! - [`api`] and [`model`] — Tracker's API; [`actions`] — the actions and their prompts.

pub mod actions;
pub mod api;
mod edit;
pub mod facts;
mod links;
pub mod model;
mod poll;
mod window;
mod work;

use std::collections::HashMap;
use std::time::Duration;

use flux_plugin_api::{CommandContext, Event, timers};

use crate::auth::Auth;
use crate::client;
use model::{Comment, Component, Issue, IssueLink, Myself, Named, Priority, Queue, User};

/// The window of the manifest.
pub const WINDOW: &str = "tracker";
/// Network work after the window opens waits a moment: the window's start isn't held up.
const STARTUP_DELAY: Duration = Duration::from_millis(1500);

/// What the list shows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Filter {
    /// The `my-query` setting.
    Mine,
    Following,
    /// A queue's open tasks.
    Queue(String),
    /// The user's own query.
    Query(String),
}

/// What a picker of the Tracker is for.
enum PickerPurpose {
    OpenTask,
    Filter,
    /// The transitions of a task's status.
    Status(String),
    /// The active task's menu.
    TaskMenu,
    /// A property of the card being changed.
    Field { task: String, field: edit::EditField },
}

/// What a form or a text question of the Tracker is for.
enum Asked {
    CustomQuery,
    Close(String),
    LogTime(String),
    /// A deadline of one's own for the task.
    Deadline(String),
    /// Story points of one's own.
    StoryPoints(String),
    Tags(String),
    /// The title and the description.
    EditTask(String),
}

pub struct Tracker {
    // The window.
    shown: bool,
    card: Option<window::Card>,
    filter: Filter,
    list: window::List,
    /// The comment being typed in the card.
    draft: String,
    // Background knowledge.
    me: Option<Myself>,
    queues: Vec<Queue>,
    /// Tooltips of links by key.
    tooltips: HashMap<String, String>,
    // What's asked of the user.
    pickers: HashMap<u64, PickerPurpose>,
    asked: HashMap<u64, Asked>,
    /// The Open Task picker's initial items: my tasks.
    open_task_items: Vec<Issue>,
    // The work.
    work: work::Work,
    poll: poll::Poll,
    startup_timer: Option<u64>,
    /// Tasks changed here since the board last heard.
    changed: Vec<String>,
    // What the fields take, read once.
    users: Option<Vec<User>>,
    priorities: Vec<Priority>,
    /// A queue's issue types, by its key.
    issue_types: HashMap<String, Vec<Named>>,
    /// A queue's components, by its key.
    components: HashMap<String, Vec<Component>>,
}

impl Tracker {
    pub fn new() -> Self {
        Tracker {
            shown: false,
            card: None,
            filter: flux_plugin_api::storage::get("filter").unwrap_or(Filter::Mine),
            list: window::List::default(),
            draft: String::new(),
            me: None,
            queues: Vec::new(),
            tooltips: HashMap::new(),
            pickers: HashMap::new(),
            asked: HashMap::new(),
            open_task_items: Vec::new(),
            work: work::Work::default(),
            poll: poll::Poll::default(),
            startup_timer: None,
            changed: Vec::new(),
            users: None,
            priorities: Vec::new(),
            issue_types: HashMap::new(),
            components: HashMap::new(),
        }
    }

    pub fn activate(&mut self, auth: &Auth) {
        let _ = auth;
        self.work.restore();
        self.startup_timer = Some(timers::after(STARTUP_DELAY));
    }

    pub fn deactivate(&mut self) {
        self.work.save_time();
    }

    pub fn settings_changed(&mut self, auth: &Auth) {
        self.poll.restart();
        self.work.show_status_item();
        if self.filter == Filter::Mine {
            self.list.stale = true;
        }
        if self.shown {
            self.load_list(auth);
        }
    }

    pub fn auth_changed(&mut self, auth: &Auth) {
        self.me = None;
        self.queues.clear();
        self.tooltips.clear();
        self.users = None;
        self.priorities.clear();
        self.issue_types.clear();
        self.components.clear();
        self.poll.reset();
        self.list = window::List {
            stale: true,
            ..window::List::default()
        };
        self.card = None;
        if auth.headers().is_some() {
            self.learn(auth);
            // What's there now isn't news.
            self.poll_now(auth);
        } else {
            links::clear();
        }
        if self.shown {
            self.load_list(auth);
            self.render(auth);
        }
    }

    pub fn run_command(&mut self, command: &str, context: &CommandContext, auth: &Auth) {
        match command {
            "open-task" => self.open_task_picker(auth),
            "close-task" => self.close_active(auth),
            "show-task" => match self.work.active_key() {
                Some(key) => self.open_card(&key, auth),
                None => self.no_active_task(),
            },
            "open-in-browser" => {
                let key = self.work.active_key().or_else(|| self.card_key());
                match key {
                    Some(key) => open_in_browser(&key),
                    None => self.no_active_task(),
                }
            }
            "log-time" => match self.work.active_key() {
                Some(key) => self.ask_log_time(&key, auth),
                None => self.no_active_task(),
            },
            "toggle-timer" => self.work.toggle_timer(),
            "discuss-task" => {
                let key = self.work.active_key().or_else(|| self.card_key());
                match key {
                    Some(key) => self.discuss(&key, auth),
                    None => self.no_active_task(),
                }
            }
            "refresh-tasks" => {
                self.list.stale = true;
                self.load_list(auth);
                if let Some(key) = self.card_key() {
                    self.load_card(&key, auth);
                }
                self.render(auth);
            }
            "task-menu" => self.task_menu(),
            "open-notified-task" => {
                if let Some(key) = context.argument.as_deref() {
                    self.open_card(key, auth);
                }
            }
            _ => {}
        }
    }

    /// Everything the sign-in, the board and the calendar don't claim: the window, links, Git, its
    /// own timers, dialogs and pickers.
    pub fn on_event(&mut self, event: Event, auth: &Auth) {
        match event {
            Event::ToolWindowShown(window) if window == WINDOW => {
                self.shown = true;
                if self.list.stale || self.list.loaded_at.is_none() {
                    self.load_list(auth);
                }
                self.render(auth);
            }
            Event::ToolWindowHidden(window) if window == WINDOW => self.shown = false,
            Event::Ui(input) if input.window == WINDOW => self.on_ui(input, auth),
            Event::PickerQuery((id, query)) => {
                if matches!(self.pickers.get(&id), Some(PickerPurpose::OpenTask)) {
                    self.open_task_query(id, &query, auth);
                }
            }
            Event::PickerChosen((id, key)) => {
                if let Some(purpose) = self.pickers.remove(&id) {
                    self.picked(purpose, key, auth);
                }
            }
            Event::FormAnswered((id, values)) => {
                if let Some(asked) = self.asked.remove(&id) {
                    self.form_answered(asked, values, auth);
                }
            }
            Event::TextAnswered((id, text)) => {
                if let (Some(Asked::CustomQuery), Some(text)) = (self.asked.remove(&id), text) {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        self.set_filter(Filter::Query(text), auth);
                    }
                }
            }
            Event::GitDone((id, result)) => self.git_done(id, result, auth),
            Event::Timer(id) if self.startup_timer == Some(id) => {
                self.startup_timer = None;
                self.poll.restart();
                if auth.headers().is_some() {
                    self.learn(auth);
                    // What's there at the start isn't news.
                    self.poll_now(auth);
                }
            }
            Event::Timer(id) if self.poll.timer() == Some(id) => self.poll_now(auth),
            Event::Timer(id) if self.work.tick_timer() == Some(id) => {
                self.work.tick();
                if self.shown {
                    self.render(auth);
                }
            }
            Event::LinkActivated((_, text)) => self.open_card(&text.to_ascii_uppercase(), auth),
            Event::LinkHovered((request, _, text)) => self.describe_link(request, &text, auth),
            _ => {}
        }
    }

    /// The task's card in the Tracker window, shown (the board's click, a link, a notification).
    pub fn open_card(&mut self, key: &str, auth: &Auth) {
        self.card = Some(window::Card::new(key));
        self.draft.clear();
        flux_plugin_api::host::ui::show(WINDOW);
        self.shown = true;
        self.render(auth);
        self.load_card(key, auth);
        self.render(auth);
    }

    /// The favorite action (`actions`, `favorite`; else the first one) on the task: the board's
    /// button.
    pub fn run_favorite(&mut self, key: &str, auth: &Auth) {
        let actions = actions::actions();
        if let Some(action) = actions::favorite(&actions).cloned() {
            self.run_action(key, action, auth);
        }
    }

    /// The favorite action's name, for the board's button ("Take to Work").
    pub fn favorite_action_name(&self) -> String {
        actions::favorite(&actions::actions())
            .map(|action| action.title())
            .unwrap_or_default()
    }

    /// The keys of the tasks the Tracker changed since the last call (a transition, taking to
    /// work, a comment): the board refreshes their cards.
    pub fn take_changed_tasks(&mut self) -> Vec<String> {
        std::mem::take(&mut self.changed)
    }

    /// Who the token is and the queues (for links), after a sign-in or at the start.
    fn learn(&mut self, auth: &Auth) {
        if auth.organization().is_none() {
            return;
        }
        if self.me.is_none() {
            self.me = api::myself(auth).ok();
        }
        if self.queues.is_empty() {
            match api::queues(auth) {
                Ok(queues) => {
                    self.queues = queues;
                    links::set(&self.queues);
                }
                Err(err) => flux_plugin_api::log::warn(&format!("Queues: {}", err.message())),
            }
        }
    }

    fn card_key(&self) -> Option<String> {
        self.card.as_ref().map(|card| card.key.clone())
    }

    fn no_active_task(&self) {
        flux_plugin_api::notify::info(&flux_plugin_api::tr("No task is in work: open one with ⌥⇧N"));
    }

    /// The task with what a prompt needs: from the open card when it's the task, else fetched.
    fn task_data(
        &mut self,
        key: &str,
        auth: &Auth,
    ) -> Result<(Issue, Vec<IssueLink>, Vec<Comment>), client::ClientError> {
        if let Some(card) = self.card.as_ref().filter(|card| card.key == key)
            && let Some(issue) = &card.issue
        {
            return Ok((issue.clone(), card.links.clone(), card.comments.clone()));
        }
        let issue = api::issue(auth, key)?;
        let links = api::links(auth, key).unwrap_or_default();
        let comments = api::comments(auth, key).unwrap_or_default();
        Ok((issue, links, comments))
    }

    fn mark_changed(&mut self, key: &str) {
        if !self.changed.iter().any(|known| known == key) {
            self.changed.push(key.to_string());
        }
        self.list.stale = true;
        self.tooltips.remove(key);
        self.poll.quiet(key);
    }
}

/// The task's page in Tracker, in the browser.
fn open_in_browser(key: &str) {
    if let Err(err) = flux_plugin_api::system::open_url(&client::web_url(key)) {
        flux_plugin_api::log::warn(&err);
    }
}
