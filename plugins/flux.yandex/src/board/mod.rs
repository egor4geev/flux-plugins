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
//!
//! The filters (part 10.5, [`filter`]): a menu badge per facet opens the values the board's tasks
//! have, with how many tasks have each; a choice adds the value or takes it away, and the menu
//! opens again for the next one (Esc closes it), as a dropdown of checkboxes; words of the key or
//! the title filter as they're typed. Each board keeps its filter, and its saved filters — named
//! (Save Current Filter…), chosen or deleted in the saved filters' menu — in the plugin's storage.
//!
//! The cache ([`crate::cache`]): the boards, and each board read — its columns, its sprint, its
//! tasks (only the fields a card and the filters use). The tab shows the cached board at once and
//! refreshes it over the columns; a board another window read a moment ago isn't read again.

mod api;
mod filter;
mod model;
mod view;

use std::collections::HashMap;
use std::time::Duration;

use flux_plugin_api::host::{i18n, system, ui};
use flux_plugin_api::picker::{self, Picker};
use flux_plugin_api::{CommandContext, Event, UiEvent, UiInput, log, storage, timers, tr, trf};
use serde_json::Value;

use crate::Intent;
use crate::auth::Auth;
use crate::cache::{self, Cache, MAX_FILES};
use crate::client::{self, ClientError};
use crate::time;
use crate::tracker::model::Issue;

use filter::{BoardFilter, Facet, SavedFilter};
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
/// Storage: each board's filter, and its saved filters, by the board's key.
const FILTERS: &str = "board.filters";
const SAVED_FILTERS: &str = "board.saved-filters";
/// Storage: the active task, as the Tracker keeps it (its key, or an object with `key`).
const ACTIVE_TASK: &str = "active-task";
/// While the tab shows, the board refreshes this often.
const REFRESH_EVERY: Duration = Duration::from_secs(120);
/// A board read this recently (seconds; by another window) isn't read again.
const FRESH: i64 = 90;
/// The cache's files: the boards, who the user is (shared with the Tracker), a board read.
const CACHED_BOARDS: &str = "boards";
const CACHED_MYSELF: &str = "myself";

/// A board as the cache keeps it.
#[derive(serde::Serialize, serde::Deserialize)]
struct CachedBoard {
    board: BoardInfo,
    columns: Vec<Column>,
    sprint: Option<Sprint>,
    tasks: Vec<Issue>,
    total: usize,
}

/// The file of a board: `board-<its key>`.
fn board_name(key: &str) -> String {
    format!("board-{}", cache::hash(key))
}

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
    /// Each board's filter, by its key.
    filters: HashMap<String, BoardFilter>,
    /// Each board's saved filters, by its key.
    saved: HashMap<String, Vec<SavedFilter>>,
    /// The open menu of a facet's values.
    facet_picker: Option<(u64, Facet)>,
    /// The open menu of the saved filters.
    saved_picker: Option<u64>,
    /// The question of a name for the filter being saved.
    naming: Option<u64>,
    /// The account's cache; the account — taken at the start (`started`).
    cache: Cache,
    account: Option<String>,
    started: bool,
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
            filters: HashMap::new(),
            saved: HashMap::new(),
            facet_picker: None,
            saved_picker: None,
            naming: None,
            cache: Cache::none(),
            account: None,
            started: false,
        }
    }

    pub fn activate(&mut self, auth: &Auth) {
        self.only_mine = storage::get(ONLY_MINE).unwrap_or(false);
        self.favorites = storage::get(FAVORITES).unwrap_or_default();
        self.filters = storage::get(FILTERS).unwrap_or_default();
        self.saved = storage::get(SAVED_FILTERS).unwrap_or_default();
        self.use_account(auth);
    }

    /// Takes the account of `auth` (the Tracker deletes the other accounts' files): its cache.
    /// Returns whether it changed.
    fn use_account(&mut self, auth: &Auth) -> bool {
        let account = cache::account(auth);
        if self.started && account == self.account {
            return false;
        }
        self.started = true;
        self.cache = match &account {
            Some(name) => Cache::at(&cache::root(), name),
            None => Cache::none(),
        };
        self.account = account;
        true
    }

    /// The board the tab opens, from the cache: shown at once, then refreshed over its columns.
    /// Returns whether it was there, and fresh (read by a window a moment ago).
    fn load_cached(&mut self) -> Option<bool> {
        if self.me.is_none() {
            self.me = self
                .cache
                .load::<Value>(CACHED_MYSELF)
                .map(|saved| Me::from_myself(&saved.data));
        }
        if self.boards.is_none() {
            self.boards = self
                .cache
                .load::<Vec<BoardInfo>>(CACHED_BOARDS)
                .map(|saved| saved.data);
        }
        let boards = self.boards.as_deref()?;
        let wanted = self
            .current
            .as_ref()
            .map(BoardInfo::key)
            .or_else(|| storage::get::<String>(LAST_BOARD));
        let key = opening_board(boards, wanted.as_deref(), &self.favorites)?.key();
        let saved = self.cache.load::<CachedBoard>(&board_name(&key))?;
        let fresh = time::now() - saved.saved <= FRESH;
        self.apply(saved.data);
        Some(fresh)
    }

    /// The shown board as a window read it a moment ago (this one or another): drawn from the
    /// cache, no request. False — it wants reading.
    fn fresh_from_cache(&mut self) -> bool {
        let key = self
            .current
            .as_ref()
            .map(BoardInfo::key)
            .unwrap_or_default();
        match self.cache.load::<CachedBoard>(&board_name(&key)) {
            Some(saved) if time::now() - saved.saved <= FRESH => {
                self.apply(saved.data);
                self.render();
                true
            }
            _ => false,
        }
    }

    fn apply(&mut self, cached: CachedBoard) {
        ui::set_title(WINDOW, &trf("Board: {0}", &[&cached.board.name]));
        self.columns_of = Some(cached.board.key());
        self.current = Some(cached.board);
        self.columns = cached.columns;
        self.sprint = cached.sprint;
        self.tasks = cached.tasks;
        self.total = cached.total;
        self.phase = Phase::Ready;
    }

    /// The board just read, into the cache (the newest boards kept).
    fn save(&self) {
        let Some(board) = &self.current else {
            return;
        };
        let cached = CachedBoard {
            board: board.clone(),
            columns: self.columns.clone(),
            sprint: self.sprint.clone(),
            tasks: self.tasks.clone(),
            total: self.total,
        };
        if self
            .cache
            .save(&board_name(&board.key()), &cached, time::now())
        {
            self.cache.prune("board-", MAX_FILES);
        }
    }

    pub fn deactivate(&mut self) {
        self.stop_timer();
        for picker in self.open_pickers() {
            picker::close(picker);
        }
    }

    /// The board's pickers that are open.
    fn open_pickers(&mut self) -> Vec<u64> {
        [
            self.picker.take(),
            self.facet_picker.take().map(|(id, _)| id),
            self.saved_picker.take(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    pub fn settings_changed(&mut self, auth: &Auth) {
        let _ = auth;
    }

    /// A sign-in, a sign-out, another organization: the boards are someone else's now. (The
    /// start has taken the account already: nothing changes.)
    pub fn auth_changed(&mut self, auth: &Auth) {
        if !self.use_account(auth) {
            return;
        }
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
                    || Some(*picker) == self.facet_picker.map(|(id, _)| id)
                    || Some(*picker) == self.saved_picker
            }
            Event::TextAnswered((question, _)) => Some(*question) == self.naming,
            _ => false,
        }
    }

    pub fn on_event(&mut self, event: Event, auth: &Auth) -> Vec<Intent> {
        match event {
            Event::ToolWindowShown(_) => {
                self.shown = true;
                self.start_timer();
                if self.current.is_none() && auth.headers().is_some() {
                    // The cached board at once; refreshed over its columns unless it's fresh.
                    match self.load_cached() {
                        Some(true) => {
                            self.render();
                            return Vec::new();
                        }
                        Some(false) => {
                            self.render();
                            self.refresh(auth, Scope::Tasks);
                            return Vec::new();
                        }
                        None => {}
                    }
                }
                if matches!(self.phase, Phase::Ready) && self.fresh_from_cache() {
                    return Vec::new();
                }
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
                for picker in self.open_pickers() {
                    picker::close(picker);
                }
                Vec::new()
            }
            Event::Timer(_) => {
                if self.shown && matches!(self.phase, Phase::Ready) && !self.fresh_from_cache() {
                    self.refresh(auth, Scope::Tasks);
                }
                Vec::new()
            }
            Event::PickerChosen((picker, choice)) => {
                if let Some((_, facet)) = self.facet_picker.filter(|(id, _)| *id == picker) {
                    self.facet_picker = None;
                    if let Some(choice) = choice {
                        self.facet_chosen(facet, &choice);
                    }
                } else if self.saved_picker == Some(picker) {
                    self.saved_picker = None;
                    if let Some(choice) = choice {
                        self.saved_chosen(&choice);
                    }
                } else {
                    self.picker = None;
                    if let Some(key) = choice {
                        self.choose_board(&key, auth);
                    }
                }
                Vec::new()
            }
            Event::TextAnswered((_, name)) => {
                self.naming = None;
                if let Some(name) = name {
                    self.save_filter(&name);
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
            // A scenario's `ui:` step can only click.
            (view::ONLY_MINE, UiEvent::Clicked) => {
                self.only_mine = !self.only_mine;
                storage::set(ONLY_MINE, &self.only_mine);
                self.after_filter();
            }
            (facet, UiEvent::Clicked) if Facet::of(facet).is_some() => {
                if let Some(facet) = Facet::of(facet) {
                    self.open_facet_picker(facet);
                }
            }
            (view::SEARCH, UiEvent::Changed(text) | UiEvent::Submitted(text)) => {
                let mut filter = self.filter();
                if filter.text != text {
                    filter.text = text;
                    self.set_filter(filter);
                }
            }
            (view::SAVED, UiEvent::Clicked) => self.open_saved_picker(),
            (view::CLEAR, UiEvent::Clicked) => self.set_filter(BoardFilter::default()),
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
                self.save();
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
                Ok(value) => {
                    self.cache.save(CACHED_MYSELF, &value, time::now());
                    self.me = Some(Me::from_myself(&value));
                }
                Err(err @ (ClientError::SignedOut | ClientError::Unauthorized)) => return Err(err),
                // Only Mine waits for the next refresh; the board shows.
                Err(err) => log::warn(&format!(
                    "Couldn't learn who the user is: {}",
                    err.message()
                )),
            }
        }
        if scope == Scope::All || self.boards.is_none() {
            let boards = api::boards(auth)?;
            self.cache.save(CACHED_BOARDS, &boards, time::now());
            self.boards = Some(boards);
        }
        let boards = self.boards.as_deref().unwrap_or_default();
        let wanted = self
            .current
            .as_ref()
            .map(BoardInfo::key)
            .or_else(|| storage::get::<String>(LAST_BOARD));
        let Some(board) = opening_board(boards, wanted.as_deref(), &self.favorites).cloned() else {
            self.current = None;
            return Ok(());
        };
        if scope >= Scope::Board || self.columns_of.as_deref() != Some(board.key().as_str()) {
            self.columns = api::columns(auth, &board.key())?;
            // A kanban board has no sprints (the API answers so); a failure here only loses the
            // sprint.
            let sprints = api::sprints(auth, &board.key()).unwrap_or_else(|err| {
                log::warn(&format!(
                    "Couldn't read the board's sprints: {}",
                    err.message()
                ));
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

    /// Only Mine or a filter changed: the cards change, the selection may go.
    fn after_filter(&mut self) {
        self.render();
    }

    /// The shown board's filter.
    fn filter(&self) -> BoardFilter {
        self.current
            .as_ref()
            .and_then(|board| self.filters.get(&board.key()))
            .cloned()
            .unwrap_or_default()
    }

    /// Sets the shown board's filter, keeps it, shows the cards it lets through.
    fn set_filter(&mut self, filter: BoardFilter) {
        let Some(board) = self.current.as_ref().map(BoardInfo::key) else {
            return;
        };
        if filter.is_empty() {
            self.filters.remove(&board);
        } else {
            self.filters.insert(board, filter);
        }
        storage::set(FILTERS, &self.filters);
        self.after_filter();
    }

    /// The shown board's saved filters.
    fn saved_filters(&self) -> &[SavedFilter] {
        self.current
            .as_ref()
            .and_then(|board| self.saved.get(&board.key()))
            .map_or(&[], Vec::as_slice)
    }

    /// The tasks of the board before the filters: the sprint's.
    fn board_tasks(&self) -> Vec<Issue> {
        visible_tasks(
            &self.tasks,
            self.sprint.as_ref(),
            false,
            None,
            &BoardFilter::default(),
        )
    }

    /// The tasks a facet's values are counted on: those the other filters (and Only mine) let
    /// through — a value's count is how many tasks choosing it shows, as faceted search counts.
    fn facet_tasks(&self, facet: Facet) -> Vec<Issue> {
        let mut others = self.filter();
        others.clear(facet);
        visible_tasks(
            &self.tasks,
            self.sprint.as_ref(),
            self.only_mine,
            self.me.as_ref(),
            &others,
        )
    }

    /// The tasks the columns show: the sprint's, the user's with Only Mine, those the filter lets
    /// through.
    fn visible(&self) -> Vec<Issue> {
        visible_tasks(
            &self.tasks,
            self.sprint.as_ref(),
            self.only_mine,
            self.me.as_ref(),
            &self.filter(),
        )
    }

    /// A facet's menu under its badge: the values the board's tasks have, with how many tasks have
    /// each (the chosen ones ✓), Show All while it filters.
    fn open_facet_picker(&mut self, facet: Facet) {
        let filter = self.filter();
        let chosen = filter.chosen(facet);
        let values = filter::options(&self.facet_tasks(facet), facet, chosen, self.me.as_ref());
        let mut items = Vec::new();
        if !chosen.is_empty() {
            items.push(picker::item("clear", &tr("Show All")).icon("close"));
        }
        for value in &values {
            let mut item = picker::item(&format!("value:{}", value.key), &value.name)
                .detail(&value.count.to_string());
            if value.mine {
                item = item.spans([
                    flux_plugin_api::view::span(&value.name),
                    flux_plugin_api::view::SpanStyle::tone(
                        flux_plugin_api::view::span(&format!("  {}", tr("you"))),
                        flux_plugin_api::view::Tone::Dim,
                    ),
                ]);
            }
            if chosen.iter().any(|choice| choice.key == value.key) {
                item = item.icon("check");
            }
            items.push(item);
        }
        if values.is_empty() {
            items.push(picker::item("nothing", &tr("The board's tasks have none")));
        }
        let picker = if values.len() > 8 {
            Picker::new(&trf("Find: {0}", &[&facet.title()]))
        } else {
            Picker::menu()
        };
        let id = picker.items(items).under(WINDOW, facet.id()).open();
        self.facet_picker = Some((id, facet));
    }

    /// A value chosen in a facet's menu: it joins the filter or leaves it, and the menu opens again
    /// for the next one; Show All clears the facet.
    fn facet_chosen(&mut self, facet: Facet, choice: &str) {
        let mut filter = self.filter();
        if choice == "clear" {
            filter.clear(facet);
            self.set_filter(filter);
            return;
        }
        let Some(key) = choice.strip_prefix("value:") else {
            return;
        };
        let name = filter::options(
            &self.board_tasks(),
            facet,
            filter.chosen(facet),
            self.me.as_ref(),
        )
        .into_iter()
        .find(|value| value.key == key)
        .map_or_else(|| key.to_string(), |value| value.name);
        filter.toggle(facet, key, &name);
        self.set_filter(filter);
        self.open_facet_picker(facet);
    }

    /// The saved filters' menu: the board's saved filters (the one shown ✓), Save Current
    /// Filter…, Delete, Clear Filters.
    fn open_saved_picker(&mut self) {
        let filter = self.filter();
        let saved = self.saved_filters();
        let current = saved.iter().position(|saved| saved.filter.same_as(&filter));
        let group = tr("Saved Filters");
        let mut items: Vec<picker::Item> = saved
            .iter()
            .enumerate()
            .map(|(index, saved)| {
                let mut item = picker::item(&format!("saved:{index}"), &saved.name).group(&group);
                // What it filters, unless its name says just that.
                let words = saved.filter.describe();
                if words != saved.name {
                    item = item.detail(&words);
                }
                if current == Some(index) {
                    item.icon("check")
                } else {
                    item
                }
            })
            .collect();
        let actions = tr("Filters");
        if !filter.is_empty() && current.is_none() {
            items.push(picker::item("save", &tr("Save Current Filter…")).group(&actions));
        }
        if let Some(index) = current {
            items.push(
                picker::item(
                    &format!("delete:{index}"),
                    &trf("Delete «{0}»", &[&saved[index].name]),
                )
                .icon("trash")
                .group(&actions),
            );
        }
        if !filter.is_empty() {
            items.push(picker::item("clear", &tr("Clear Filters")).group(&actions));
        }
        if items.is_empty() {
            items.push(picker::item(
                "nothing",
                &tr("Choose filters — then save them here under a name"),
            ));
        }
        let id = Picker::menu()
            .items(items)
            .under(WINDOW, view::SAVED)
            .open();
        self.saved_picker = Some(id);
    }

    fn saved_chosen(&mut self, choice: &str) {
        let Some(board) = self.current.as_ref().map(BoardInfo::key) else {
            return;
        };
        if choice == "save" {
            let filter = self.filter();
            let id = flux_plugin_api::dialog::AskText::new(&tr("Save Filter"), &tr("Save"))
                .message(&tr(
                    "The filter is kept for this board, in the saved filters' menu.",
                ))
                .text(&filter::suggested_name(&filter))
                .placeholder(&tr("A name"))
                .send();
            self.naming = Some(id);
        } else if choice == "clear" {
            self.set_filter(BoardFilter::default());
        } else if let Some(index) = choice.strip_prefix("saved:").and_then(|i| i.parse().ok()) {
            if let Some(saved) = self.saved_filters().get::<usize>(index).cloned() {
                self.set_filter(saved.filter);
            }
        } else if let Some(index) = choice
            .strip_prefix("delete:")
            .and_then(|index| index.parse::<usize>().ok())
            && let Some(list) = self.saved.get_mut(&board)
            && index < list.len()
        {
            list.remove(index);
            if list.is_empty() {
                self.saved.remove(&board);
            }
            storage::set(SAVED_FILTERS, &self.saved);
            self.render();
        }
    }

    /// Saves the shown board's filter under `name` (one of the same name is replaced).
    fn save_filter(&mut self, name: &str) {
        let name = name.trim();
        let filter = self.filter();
        let Some(board) = self.current.as_ref().map(BoardInfo::key) else {
            return;
        };
        if name.is_empty() || filter.is_empty() {
            return;
        }
        let list = self.saved.entry(board).or_default();
        let saved = SavedFilter {
            name: name.to_string(),
            filter,
        };
        match list.iter_mut().find(|known| known.name == name) {
            Some(known) => *known = saved,
            None => list.push(saved),
        }
        storage::set(SAVED_FILTERS, &self.saved);
        self.render();
    }

    fn render(&mut self) {
        let lanes: Vec<Lane> = match self.phase {
            Phase::Ready => model::arrange(&self.columns, &self.visible(), &tr("Other statuses")),
            _ => Vec::new(),
        };
        // Each column's tasks before the filters and Only mine: "2 of 5".
        let totals: Vec<(String, usize)> = match self.phase {
            Phase::Ready => {
                model::arrange(&self.columns, &self.board_tasks(), &tr("Other statuses"))
                    .into_iter()
                    .map(|lane| (lane.key, lane.tasks.len()))
                    .collect()
            }
            _ => Vec::new(),
        };
        let filter = self.filter();
        let saved = self
            .saved_filters()
            .iter()
            .find(|saved| !filter.is_empty() && saved.filter.same_as(&filter))
            .map(|saved| saved.name.clone());
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
            filter: &filter,
            saved: saved.as_deref(),
            totals: &totals,
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
/// the sprint may leave the field out), the user's with Only Mine (when the user is known), those
/// the board's filter lets through.
fn visible_tasks(
    tasks: &[Issue],
    sprint: Option<&Sprint>,
    only_mine: bool,
    me: Option<&Me>,
    filter: &BoardFilter,
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
        .filter(|task| filter.matches(task))
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
        assert_eq!(
            keys(&visible_tasks(
                &tasks,
                None,
                false,
                Some(&me),
                &BoardFilter::default()
            )),
            ["A-1", "A-2", "A-3", "A-4"]
        );
        assert_eq!(
            keys(&visible_tasks(
                &tasks,
                Some(&sprint),
                false,
                Some(&me),
                &BoardFilter::default()
            )),
            ["A-1", "A-2"]
        );
        assert_eq!(
            keys(&visible_tasks(
                &tasks,
                Some(&sprint),
                true,
                Some(&me),
                &BoardFilter::default()
            )),
            ["A-1"]
        );
        // The user unknown: Only Mine can't filter.
        assert_eq!(
            visible_tasks(&tasks, None, true, None, &BoardFilter::default()).len(),
            4
        );
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
        assert_eq!(
            visible_tasks(&tasks, Some(&sprint), false, None, &BoardFilter::default()).len(),
            2
        );
    }
}
