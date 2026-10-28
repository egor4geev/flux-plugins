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
pub mod fields;
mod links;
pub mod model;
mod poll;
mod sync;
mod window;
mod work;

use std::collections::HashMap;

use flux_plugin_api::{CommandContext, Event};

use crate::auth::Auth;
use crate::cache::Cache;
use crate::client;
use model::{
    Comment, Component, FieldMeta, Issue, IssueLink, Myself, Named, Priority, Queue, User,
};

/// The window of the manifest.
pub const WINDOW: &str = "tracker";
/// Storage: the card shows every field of the task (true by default) or only the main ones.
const ALL_FIELDS: &str = "card.all-fields";

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
    Field {
        task: String,
        field: edit::EditField,
    },
    /// One of the other fields of the card being changed: its key in the task.
    OtherField {
        task: String,
        field: String,
    },
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
    /// A value of one's own for one of the other fields: the task, the field's key, how it reads.
    OtherField {
        task: String,
        field: String,
        kind: fields::EditKind,
    },
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
    /// When `me` was read (unix seconds).
    me_saved: i64,
    queues: Vec<Queue>,
    queues_saved: i64,
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
    // The cache, the start, the windows.
    /// The account's cache ([`crate::cache`]).
    cache: Cache,
    /// The account the cache and what's known are of; `started` — it's been taken.
    account: Option<String>,
    started: bool,
    /// The background work: one request a step.
    chain: sync::Chain,
    /// This window polls for all ([`crate::lead`]).
    leading: bool,
    /// The leader's news file as last seen, and the news it told.
    news_stamp: Option<std::time::SystemTime>,
    news_seen: i64,
    /// Tasks changed here since the board last heard.
    changed: Vec<String>,
    // What the fields take, read once.
    users: Option<Vec<User>>,
    priorities: Vec<Priority>,
    /// A queue's issue types, by its key.
    issue_types: HashMap<String, Vec<Named>>,
    /// A queue's components, by its key.
    components: HashMap<String, Vec<Component>>,
    /// The system fields (`GET /v3/fields`): their names, types and categories, read once.
    field_meta: Option<Vec<FieldMeta>>,
    /// A queue's own fields, by its key.
    local_fields: HashMap<String, Vec<FieldMeta>>,
    /// The card shows every field of the task, not only the main ones (`card.all-fields`).
    all_fields: bool,
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
            me_saved: 0,
            queues: Vec::new(),
            queues_saved: 0,
            tooltips: HashMap::new(),
            pickers: HashMap::new(),
            asked: HashMap::new(),
            open_task_items: Vec::new(),
            work: work::Work::default(),
            poll: poll::Poll::default(),
            cache: Cache::none(),
            account: None,
            started: false,
            chain: sync::Chain::default(),
            leading: false,
            news_stamp: None,
            news_seen: 0,
            changed: Vec::new(),
            users: None,
            priorities: Vec::new(),
            issue_types: HashMap::new(),
            components: HashMap::new(),
            field_meta: None,
            local_fields: HashMap::new(),
            all_fields: flux_plugin_api::storage::get(ALL_FIELDS).unwrap_or(true),
        }
    }

    /// The start: what the cache has shows at once — the window's content is sent before the
    /// window shows — then the network work runs in the background ([`sync`]).
    pub fn activate(&mut self, auth: &Auth) {
        self.work.restore();
        self.poll.read_interval();
        self.use_account(auth);
        self.draw(auth);
        if auth.headers().is_some() {
            self.start();
        }
    }

    pub fn deactivate(&mut self) {
        self.work.save_time();
        self.chain.stop();
    }

    /// New settings: the poll's interval; `my-query` — My Tasks of the new query (cached, then
    /// read). The account's settings come as [`Tracker::auth_changed`].
    pub fn settings_changed(&mut self, auth: &Auth) {
        self.poll.read_interval();
        self.work.show_status_item();
        if self.filter == Filter::Mine && self.filter_query() != self.list.query {
            if !self.load_cached_list() {
                self.list.issues.clear();
                self.list.total = 0;
            }
            self.list.stale = true;
            self.list.next_try = 0;
            if self.shown {
                self.render(auth);
                self.queue_list_first();
            }
        }
    }

    /// A sign-in, a sign-out, another organization (and the start, which has taken the account
    /// already): another account's cache shows, then its network work.
    pub fn auth_changed(&mut self, auth: &Auth) {
        if !self.use_account(auth) {
            return;
        }
        self.chain.stop();
        if auth.headers().is_some() {
            self.start();
        } else {
            links::clear();
        }
        self.render(auth);
    }

    /// Forgets what was known of the account.
    fn forget(&mut self) {
        self.me = None;
        self.me_saved = 0;
        self.queues.clear();
        self.queues_saved = 0;
        self.tooltips.clear();
        self.users = None;
        self.priorities.clear();
        self.issue_types.clear();
        self.components.clear();
        self.field_meta = None;
        self.local_fields.clear();
        self.poll.reset();
        self.list = window::List {
            stale: true,
            generation: self.list.generation + 1,
            ..window::List::default()
        };
        self.card = None;
        self.news_stamp = None;
        self.news_seen = 0;
        links::clear();
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
                self.list.next_try = 0;
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

    /// Everything the sign-in and the board don't claim: the window, links, Git, its
    /// own timers, dialogs and pickers.
    pub fn on_event(&mut self, event: Event, auth: &Auth) {
        match event {
            // Shown (again — Flux repeats it for a window open at the start): what's there shows
            // at once; the list is read in the background only if it isn't fresh.
            Event::ToolWindowShown(window) if window == WINDOW => {
                self.shown = true;
                self.render(auth);
                if self.card.is_none() && self.list_due(crate::time::now()) {
                    self.queue_list_first();
                }
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
            Event::Timer(id) if self.chain.timer() == Some(id) => self.run_step(auth),
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

    fn card_key(&self) -> Option<String> {
        self.card.as_ref().map(|card| card.key.clone())
    }

    fn no_active_task(&self) {
        flux_plugin_api::notify::info(&flux_plugin_api::tr(
            "No task is in work: open one with ⌥⇧N",
        ));
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
        self.list.next_try = 0;
        self.tooltips.remove(key);
        self.quiet(key);
    }
}

/// The task's page in Tracker, in the browser.
fn open_in_browser(key: &str) {
    if let Err(err) = flux_plugin_api::system::open_url(&client::web_url(key)) {
        flux_plugin_api::log::warn(&err);
    }
}
