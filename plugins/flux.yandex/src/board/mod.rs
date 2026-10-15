//! The board of tasks (agent F2), an editor tab (`board`, `location = "editor"`), as boards are in
//! Tracker: the user's boards (`GET /v3/boards`, the last one remembered), the board's columns —
//! the statuses each takes — side by side, the cards of the board's tasks (key, title, type,
//! priority, assignee; the active task marked); the toolbar: the board (a menu badge → a picker),
//! Only Mine, the favorite action's button (the selected card, as the Tracker says it is named),
//! Refresh, Open in Browser; the tab's title "Board: <name>" (`ui.set-title`). A chosen card opens
//! in the Tracker window ([`crate::Intent::OpenCard`]); the button takes it to work
//! ([`crate::Intent::RunFavorite`]). Scrum boards — the current sprint.
//!
//! The board loads when its tab shows (`open-board` opens it), refreshes every two minutes while
//! it's shown and when the Tracker changes a task on it; requests go one at a time (Tracker's
//! limits) and block only the plugin. Only Mine filters here, by the user's ids from
//! `GET /v3/myself`; the board's choice, the favorite boards (☆ in the toolbar: first in the
//! board's menu) and Only Mine are kept in the plugin's storage. The tab opens the board chosen
//! last, else the first favorite, else the most recently changed one.

mod api;
mod model;
mod view;

use std::time::Duration;

use flux_plugin_api::host::{i18n, system, ui};
use flux_plugin_api::picker::{self, Picker};
use flux_plugin_api::{CommandContext, Event, UiEvent, UiInput, log, storage, timers, tr, trf};
use serde_json::Value;

use crate::Intent;
use crate::auth::Auth;
use crate::client::{self, ClientError};
use crate::time;
use crate::tracker::model::Issue;

use model::{BoardInfo, Column, Lane, Me, Sprint};
use view::{Content, Screen};

/// Opens (or brings forward) the board's tab.
pub const OPEN_BOARD: &str = "open-board";

/// The board's window (the manifest's `[[tool-windows]]`, in the editor).
const WINDOW: &str = "board";
/// Storage: the board shown last, the favorite boards (keys, in the order added), Only Mine.
const LAST_BOARD: &str = "board.last";
const FAVORITES: &str = "board.favorites";
const ONLY_MINE: &str = "board.only-mine";
/// Storage: the active task, as the Tracker keeps it (its key, or an object with `key`).
const ACTIVE_TASK: &str = "active-task";
/// While the tab shows, the board refreshes this often.
const REFRESH_EVERY: Duration = Duration::from_secs(120);

/// What a refresh reads.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    /// The tasks of the shown board.
    Tasks,
    /// The board's columns and sprint too (another board, Refresh).
    Board,
    /// The list of boards too (the first load, a sign-in, Try Again).
    All,
}

/// Where the board is.
enum Phase {
    /// Not loaded yet.
    Idle,
    SignedOut,
    Loading,
    Failed(String),
    NoBoards,
    Ready,
}

pub struct Board {
    shown: bool,
    phase: Phase,
    boards: Option<Vec<BoardInfo>>,
    current: Option<BoardInfo>,
    columns: Vec<Column>,
    /// The board `columns` and `sprint` are of.
    columns_of: Option<String>,
    sprint: Option<Sprint>,
    tasks: Vec<Issue>,
    /// How many tasks the board has: more than `tasks` when only the first pages were read.
    total: usize,
    me: Option<Me>,
    /// The favorite boards' keys, in the order added.
    favorites: Vec<String>,
    only_mine: bool,
    selected: Option<String>,
    /// The favorite action's name, as the Tracker says.
    favorite: String,
    picker: Option<u64>,
    timer: Option<u64>,
    /// A refresh over the shown columns runs, or failed (why).
    refreshing: bool,
    stale: Option<String>,
}

impl Board {
    pub fn new() -> Self {
        Board {
            shown: false,
            phase: Phase::Idle,
            boards: None,
            current: None,
            columns: Vec::new(),
            columns_of: None,
            sprint: None,
            tasks: Vec::new(),
            total: 0,
            me: None,
            favorites: Vec::new(),
            only_mine: false,
            selected: None,
            favorite: String::new(),
            picker: None,
            timer: None,
            refreshing: false,
            stale: None,
        }
    }

    pub fn activate(&mut self, auth: &Auth) {
        let _ = auth;
        self.only_mine = storage::get(ONLY_MINE).unwrap_or(false);
        self.favorites = storage::get(FAVORITES).unwrap_or_default();
    }

    pub fn deactivate(&mut self) {
        self.stop_timer();
        if let Some(picker) = self.picker.take() {
            picker::close(picker);
        }
    }

    pub fn settings_changed(&mut self, auth: &Auth) {
        let _ = auth;
    }

    /// A sign-in, a sign-out, another organization: the boards are someone else's now.
    pub fn auth_changed(&mut self, auth: &Auth) {
        self.boards = None;
        self.current = None;
        self.columns.clear();
        self.columns_of = None;
        self.sprint = None;
        self.tasks.clear();
        self.me = None;
        self.selected = None;
        self.stale = None;
        self.phase = Phase::Idle;
        if self.shown {
            self.refresh(auth, Scope::All);
        }
    }

    pub fn run_command(
        &mut self,
        command: &str,
        context: &CommandContext,
        auth: &Auth,
    ) -> Vec<Intent> {
        let _ = context;
        if command == OPEN_BOARD {
            ui::show(WINDOW);
            // A tab already open doesn't hear "shown" again: the command refreshes it.
            if self.shown {
                self.refresh(auth, Scope::Tasks);
            }
        }
        Vec::new()
    }

    /// The event is the board's: its window's input and visibility, its timer and picker.
    pub fn claims(&self, event: &Event) -> bool {
        match event {
            Event::Ui(input) => input.window == WINDOW,
            Event::ToolWindowShown(window) | Event::ToolWindowHidden(window) => window == WINDOW,
            Event::Timer(timer) => Some(*timer) == self.timer,
            Event::PickerChosen((picker, _)) | Event::PickerQuery((picker, _)) => {
                Some(*picker) == self.picker
            }
            _ => false,
        }
    }

    pub fn on_event(&mut self, event: Event, auth: &Auth) -> Vec<Intent> {
        match event {
            Event::ToolWindowShown(_) => {
                self.shown = true;
                self.start_timer();
                let scope = if self.current.is_none() {
                    Scope::All
                } else {
                    Scope::Tasks
                };
                self.refresh(auth, scope);
                Vec::new()
            }
            Event::ToolWindowHidden(_) => {
                self.shown = false;
                self.stop_timer();
                if let Some(picker) = self.picker.take() {
                    picker::close(picker);
                }
                Vec::new()
            }
            Event::Timer(_) => {
                if self.shown && matches!(self.phase, Phase::Ready) {
                    self.refresh(auth, Scope::Tasks);
                }
                Vec::new()
            }
            Event::PickerChosen((_, choice)) => {
                self.picker = None;
                if let Some(key) = choice {
                    self.choose_board(&key, auth);
                }
                Vec::new()
            }
            Event::Ui(UiInput { element, event, .. }) => self.on_ui(&element, event, auth),
            _ => Vec::new(),
        }
    }

    /// After every call of the plugin: the favorite action's name for the button, and the tasks
    /// the Tracker changed — the board refreshes when one of them is on it.
    pub fn tracker_changed(&mut self, favorite: &str, changed: &[String], auth: &Auth) {
        let mut redraw = false;
        if favorite != self.favorite {
            self.favorite = favorite.to_string();
            redraw = true;
        }
        let on_board = changed
            .iter()
            .any(|key| self.tasks.iter().any(|task| &task.key == key));
        if self.shown && matches!(self.phase, Phase::Ready) && on_board {
            self.refresh(auth, Scope::Tasks);
        } else if redraw && self.shown {
            self.render();
        }
    }

    fn on_ui(&mut self, element: &str, event: UiEvent, auth: &Auth) -> Vec<Intent> {
        match (element, event) {
            (view::BOARD_PICKER, UiEvent::Clicked) => self.open_picker(),
            (view::STAR, UiEvent::Clicked) => self.toggle_favorite(),
            (view::ONLY_MINE, UiEvent::Toggled(on)) => {
                self.only_mine = on;
                storage::set(ONLY_MINE, &on);
                self.after_filter();
            }
            // The favorite action's button on a card: that card's task, selected or not.
            (cards, UiEvent::RowAction((key, action)))
                if cards.starts_with(view::CARDS) && action == view::TAKE =>
            {
                return vec![Intent::RunFavorite(key)];
            }
            (view::REFRESH, UiEvent::Clicked) => self.refresh(auth, Scope::Board),
            (view::RETRY, UiEvent::Clicked) => self.refresh(auth, Scope::All),
            (view::BROWSER, UiEvent::Clicked) => {
                let url = match &self.current {
                    Some(board) => format!("{}agile/board/{}", client::WEB, board.key()),
                    None => client::WEB.to_string(),
                };
                if let Err(err) = system::open_url(&url) {
                    log::warn(&err);
                }
            }
            (view::SETTINGS, UiEvent::Clicked) => system::open_settings(),
            (view::CREATE_BOARD, UiEvent::Clicked) => {
                if let Err(err) = system::open_url(client::WEB) {
                    log::warn(&err);
                }
            }
            (cards, UiEvent::Selected(key)) if cards.starts_with(view::CARDS) => {
                if self.selected.as_deref() != Some(key.as_str()) {
                    self.selected = Some(key);
                    self.render();
                }
            }
            (cards, UiEvent::Activated(key)) if cards.starts_with(view::CARDS) => {
                if self.selected.as_deref() != Some(key.as_str()) {
                    self.selected = Some(key.clone());
                    self.render();
                }
                return vec![Intent::OpenCard(key)];
            }
            _ => {}
        }
        Vec::new()
    }

    /// The boards' menu under the board's badge: the favorites first (in the order added), then
    /// all the others by name; the shown one ✓.
    fn open_picker(&mut self) {
        let Some(boards) = &self.boards else {
            return;
        };
        let current = self.current.as_ref().map(BoardInfo::key);
        let item = |board: &BoardInfo, group: &str| {
            let item = picker::item(&board.key(), &board.name).group(group);
            if current.as_deref() == Some(board.key().as_str()) {
                item.badge("✓")
            } else {
                item
            }
        };
        let favorites = tr("Favorites");
        let all = tr("All Boards");
        let mut items: Vec<picker::Item> = self
            .favorites
            .iter()
            .filter_map(|key| boards.iter().find(|board| &board.key() == key))
            .map(|board| item(board, &favorites).icon("star-filled"))
            .collect();
        let mut others: Vec<&BoardInfo> = boards
            .iter()
            .filter(|board| !self.favorites.contains(&board.key()))
            .collect();
        others.sort_by_key(|board| board.name.to_lowercase());
        items.extend(others.into_iter().map(|board| item(board, &all)));
        let picker = Picker::new(&tr("Find a board"))
            .items(items)
            .empty_text(&tr("No boards"))
            .under(WINDOW, view::BOARD_PICKER);
        self.picker = Some(picker.open());
    }

    /// ☆ in the toolbar: the shown board joins the favorite boards, or leaves them.
    fn toggle_favorite(&mut self) {
        let Some(key) = self.current.as_ref().map(BoardInfo::key) else {
            return;
        };
        match self.favorites.iter().position(|favorite| *favorite == key) {
            Some(index) => {
                self.favorites.remove(index);
            }
            None => self.favorites.push(key),
        }
        storage::set(FAVORITES, &self.favorites);
        self.render();
    }

    fn choose_board(&mut self, key: &str, auth: &Auth) {
        let Some(board) = self
            .boards
            .as_ref()
            .and_then(|boards| boards.iter().find(|board| board.key() == key))
            .cloned()
        else {
            return;
        };
        storage::set(LAST_BOARD, &board.key());
        if self.current.as_ref().map(BoardInfo::key).as_deref() == Some(key) {
            return;
        }
        // The tab says which board at once; its columns follow when they're read.
        ui::set_title(WINDOW, &trf("Board: {0}", &[&board.name]));
        self.current = Some(board);
        self.selected = None;
        self.tasks.clear();
        self.phase = Phase::Loading;
        self.refresh(auth, Scope::Board);
    }

    /// Reads what `scope` says and draws the result. Over shown columns a refresh of the tasks
    /// keeps them while it runs (a thin progress bar) and if it fails (the error over them).
    fn refresh(&mut self, auth: &Auth, scope: Scope) {
        if auth.headers().is_none() {
            self.phase = Phase::SignedOut;
            self.render();
            return;
        }
        let over_columns = matches!(self.phase, Phase::Ready) && scope == Scope::Tasks;
        if over_columns {
            self.refreshing = true;
        } else {
            self.phase = Phase::Loading;
        }
        self.render();
        let result = self.load(auth, scope);
        self.refreshing = false;
        match result {
            Ok(()) => {
                self.stale = None;
                self.phase = if self.current.is_some() {
                    Phase::Ready
                } else {
                    Phase::NoBoards
                };
            }
            Err(ClientError::SignedOut) => self.phase = Phase::SignedOut,
            Err(err) if over_columns => self.stale = Some(err.message()),
            Err(err) => self.phase = Phase::Failed(err.message()),
        }
        self.render();
    }

    fn load(&mut self, auth: &Auth, scope: Scope) -> Result<(), ClientError> {
        if self.me.is_none() {
            match api::myself(auth) {
                Ok(me) => self.me = Some(me),
                Err(err @ (ClientError::SignedOut | ClientError::Unauthorized)) => return Err(err),
                // Only Mine waits for the next refresh; the board shows.
                Err(err) => log::warn(&format!("Couldn't learn who the user is: {}", err.message())),
            }
        }
        if scope == Scope::All || self.boards.is_none() {
            self.boards = Some(api::boards(auth)?);
        }
        let boards = self.boards.as_deref().unwrap_or_default();
        let wanted = self
            .current
            .as_ref()
            .map(BoardInfo::key)
            .or_else(|| storage::get::<String>(LAST_BOARD));
        let Some(board) = opening_board(boards, wanted.as_deref(), &self.favorites).cloned()
        else {
            self.current = None;
            return Ok(());
        };
        if scope >= Scope::Board || self.columns_of.as_deref() != Some(board.key().as_str()) {
            self.columns = api::columns(auth, &board.key())?;
            // A kanban board has no sprints (the API answers so); a failure here only loses the
            // sprint.
            let sprints = api::sprints(auth, &board.key()).unwrap_or_else(|err| {
                log::warn(&format!("Couldn't read the board's sprints: {}", err.message()));
                Vec::new()
            });
            self.sprint = model::current_sprint(&sprints, &today());
            self.columns_of = Some(board.key());
        }
        let (tasks, total) = api::tasks(auth, &board, self.sprint.as_ref())?;
        self.tasks = tasks;
        self.total = total;
        ui::set_title(WINDOW, &trf("Board: {0}", &[&board.name]));
        self.current = Some(board);
        Ok(())
    }

    /// Only Mine changed: the cards change, the selection may go.
    fn after_filter(&mut self) {
        self.render();
    }

    /// The tasks the columns show: the sprint's, the user's with Only Mine.
    fn visible(&self) -> Vec<Issue> {
        visible_tasks(&self.tasks, self.sprint.as_ref(), self.only_mine, self.me.as_ref())
    }

    fn render(&mut self) {
        let lanes: Vec<Lane> = match self.phase {
            Phase::Ready => model::arrange(&self.columns, &self.visible(), &tr("Other statuses")),
            _ => Vec::new(),
        };
        self.selected = view::keep_selection(self.selected.as_deref(), &lanes);
        let sprint = self.sprint.as_ref().map(sprint_line);
        let active = active_task();
        let content = match &self.phase {
            Phase::Idle | Phase::Loading => Content::Loading,
            Phase::SignedOut => Content::SignedOut,
            Phase::Failed(problem) => Content::Failed(problem),
            Phase::NoBoards => Content::NoBoards,
            Phase::Ready => Content::Lanes(&lanes),
        };
        let favorite_board = self
            .current
            .as_ref()
            .is_some_and(|board| self.favorites.contains(&board.key()));
        let now = crate::time::now();
        let language = flux_plugin_api::host::i18n::language();
        let screen = Screen {
            board: self.current.as_ref().map(|board| board.name.as_str()),
            favorite_board,
            only_mine: self.only_mine,
            favorite: &self.favorite,
            me: self.me.as_ref(),
            today: crate::tracker::facts::today(now, crate::time::offset_at(now)),
            language: &language,
            active: active.as_deref(),
            sprint: sprint.as_deref(),
            truncated: (self.total > self.tasks.len()).then_some((self.tasks.len(), self.total)),
            refreshing: self.refreshing,
            stale: self.stale.as_deref(),
            content,
        };
        flux_plugin_api::view::set_view(WINDOW, view::render(&screen));
    }

    fn start_timer(&mut self) {
        if self.timer.is_none() {
            self.timer = Some(timers::every(REFRESH_EVERY));
        }
    }

    fn stop_timer(&mut self) {
        if let Some(timer) = self.timer.take() {
            timers::cancel(timer);
        }
    }
}

/// The board the tab opens: the one asked for (shown, chosen last) while it exists, else the first
/// favorite, else the most recently changed board, else the first.
fn opening_board<'a>(
    boards: &'a [BoardInfo],
    wanted: Option<&str>,
    favorites: &[String],
) -> Option<&'a BoardInfo> {
    let find = |key: &str| boards.iter().find(|board| board.key() == key);
    wanted
        .and_then(find)
        .or_else(|| favorites.iter().find_map(|key| find(key)))
        .or_else(|| {
            boards
                .iter()
                .filter(|board| board.updated_at.is_some())
                .max_by(|a, b| a.updated_at.cmp(&b.updated_at))
        })
        .or_else(|| boards.first())
}

/// The tasks a board shows: of the sprint (when the tasks say which — a server that filtered by
/// the sprint may leave the field out), and the user's with Only Mine (when the user is known).
fn visible_tasks(
    tasks: &[Issue],
    sprint: Option<&Sprint>,
    only_mine: bool,
    me: Option<&Me>,
) -> Vec<Issue> {
    let sprints_known = tasks.iter().any(|task| task.sprint.is_some());
    tasks
        .iter()
        .filter(|task| match sprint {
            Some(sprint) if sprints_known => model::in_sprint(task, sprint),
            _ => true,
        })
        .filter(|task| match (only_mine, me) {
            (true, Some(me)) => me.owns(task),
            _ => true,
        })
        .cloned()
        .collect()
}

/// The active task's key, as the Tracker keeps it in the storage (a key, or `{"key": …}`).
fn active_task() -> Option<String> {
    match storage::get::<Value>(ACTIVE_TASK)? {
        Value::String(key) => Some(key),
        Value::Object(task) => task.get("key")?.as_str().map(str::to_string),
        _ => None,
    }
}

/// Today in the user's time: "2026-10-14".
fn today() -> String {
    let now = time::now();
    let date = time::civil(now, time::offset_at(now));
    format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
}

/// "Sprint 8 · 7 Oct – 20 Oct".
fn sprint_line(sprint: &Sprint) -> String {
    let language = i18n::language();
    let this_year = {
        let now = time::now();
        time::civil(now, time::offset_at(now)).year
    };
    let date = |text: &Option<String>| {
        text.as_deref()
            .and_then(time::parse_date)
            .map(|(year, month, day)| time::day_month(year, month, day, this_year, &language))
    };
    match (date(&sprint.start_date), date(&sprint.end_date)) {
        (Some(start), Some(end)) => format!("{} · {start} – {end}", sprint.name),
        _ => sprint.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::model::Named;

    fn issue(key: &str, assignee: Option<&str>, sprint: Option<Value>) -> Issue {
        Issue {
            key: key.into(),
            assignee: assignee.map(|id| Named {
                id: Value::String(id.into()),
                key: None,
                display: id.into(),
            }),
            sprint,
            ..Issue::default()
        }
    }

    fn keys(tasks: &[Issue]) -> Vec<&str> {
        tasks.iter().map(|task| task.key.as_str()).collect()
    }

    #[test]
    fn only_mine_and_the_sprint() {
        let me = Me::from_myself(&serde_json::json!({"uid": 7, "login": "egor"}));
        let sprint = Sprint {
            id: Value::from(8),
            ..Sprint::default()
        };
        let in_8 = Some(serde_json::json!([{"id": "8"}]));
        let in_7 = Some(serde_json::json!([{"id": "7"}]));
        let tasks = vec![
            issue("A-1", Some("7"), in_8.clone()),
            issue("A-2", Some("9"), in_8),
            issue("A-3", Some("7"), in_7),
            issue("A-4", None, None),
        ];
        assert_eq!(keys(&visible_tasks(&tasks, None, false, Some(&me))), ["A-1", "A-2", "A-3", "A-4"]);
        assert_eq!(keys(&visible_tasks(&tasks, Some(&sprint), false, Some(&me))), ["A-1", "A-2"]);
        assert_eq!(keys(&visible_tasks(&tasks, Some(&sprint), true, Some(&me))), ["A-1"]);
        // The user unknown: Only Mine can't filter.
        assert_eq!(visible_tasks(&tasks, None, true, None).len(), 4);
    }

    #[test]
    fn the_board_the_tab_opens() {
        let board = |id: i64, updated: Option<&str>| BoardInfo {
            id: Value::from(id),
            name: format!("Board {id}"),
            updated_at: updated.map(str::to_string),
            ..BoardInfo::default()
        };
        let boards = vec![
            board(1, Some("2022-04-06T10:24:26.329+0000")),
            board(85, Some("2026-10-06T08:00:00.000+0000")),
            board(65, Some("2026-08-13T08:00:00.000+0000")),
        ];
        let opened = |wanted: Option<&str>, favorites: &[&str]| {
            let favorites: Vec<String> = favorites.iter().map(|key| key.to_string()).collect();
            opening_board(&boards, wanted, &favorites).map(BoardInfo::key)
        };
        // Nothing chosen, no favorites: the most recently changed, not the first.
        assert_eq!(opened(None, &[]).as_deref(), Some("85"));
        assert_eq!(opened(None, &["65"]).as_deref(), Some("65"));
        assert_eq!(opened(Some("1"), &["65"]).as_deref(), Some("1"));
        // A board gone from the list: the favorites decide.
        assert_eq!(opened(Some("404"), &["404", "65"]).as_deref(), Some("65"));
    }

    #[test]
    fn a_server_filtered_sprint_without_the_field_keeps_the_tasks() {
        let sprint = Sprint {
            id: Value::from(8),
            ..Sprint::default()
        };
        let tasks = vec![issue("A-1", None, None), issue("A-2", None, None)];
        assert_eq!(visible_tasks(&tasks, Some(&sprint), false, None).len(), 2);
    }
}
