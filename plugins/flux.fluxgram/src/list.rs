//! The FluxGram window on the left (part 3, mockup A): the chats of every account in one list
//! ([`merge`]: the pinned first, then the newest), FluxGram's folders over it as chips (All, the
//! user's folders with their unread counts, + and the shown folder's ⋯: Rename…, Delete Folder),
//! a search over the folder shown, the chats as rows of two lines — the picture, the title, the
//! time, the last message (or the draft), the unread count (dim when muted), pinned and muted
//! marks — and Add Chat: a popup that searches the chats (people, bots, groups, channels) of
//! every account ([`Request::SearchChats`] each → [`Note::Found`](crate::model::Note::Found)),
//! grouped by account when there are several; the same popup is Forward's picker (part 5): one
//! account's chats — Saved Messages, the list's, then what the search finds — and the choice opens
//! the chat's tab with the messages to forward ([`Go::Forward`]). A row opens its chat's tab; its actions put it in a folder
//! (or take it out of the one shown), hide it, remove it from the list; its ⋯ (part 5) opens a menu
//! under it — Mark as Read / Unread, Mute (a choice: an hour, 8 hours, 2 days, for good) or
//! Unmute, Add to Folder…, Available to Claude (part 6: checked while Claude may read the chat;
//! never a hidden chat's) — and a chat available to Claude has Claude's mark after its title
//! (the `claude` icon, in Claude's color). The unread messages count
//! on the window's launchpad icon and put a dot on the chats' tabs (`ui::set_badge`).
//!
//! The rows come from the service whole, per account ([`Note::Chats`](crate::model::Note::Chats):
//! tens of rows, sent only when they changed), and so do the folders
//! ([`Note::Folders`](crate::model::Note::Folders)); the folder shown is this window's own. A view
//! is set only when it differs from the one set before, and a mark only when it changed. The times
//! say "Yesterday" from midnight on: one timer to the next midnight, no ticking.
//!
//! The hidden chats: when the search's text, once typing stops, is the phrase set in Settings →
//! FluxGram → Hidden Chats ([`HIDDEN`], drawn here too), the service opens them for this window —
//! the list shows only them, marked, and their tabs don't come back after a restart; once the
//! phrase leaves the search they are gone again, their tabs closed. Nothing of them shows
//! otherwise: no rows, no counts, no marks. Hide Chat (a row's action, a tab's ⋯) needs the
//! phrase: the open list's, or one typed in the Hide Chat dialog ([`HIDE`]).

use std::collections::HashMap;
use std::time::Duration;

use flux_plugin_api::UiEvent;
use flux_plugin_api::conversation::Delivery;
use flux_plugin_api::dialog::{Ask, AskText};
use flux_plugin_api::host::ui::{self, ElementKind, RowAction, Span, TreeRow, TreeSpec, View};
use flux_plugin_api::system::{self, LocalTime};
use flux_plugin_api::view::*;
use flux_plugin_api::{notify, picker, service, timers, tr, trf};

use crate::chat::{CHAT, Forward, MORE};
use crate::chats::{Folder, FolderAsk, HiddenAsk, HiddenRow, HiddenTold, folder_name};
use crate::claude::ClaudeAsk;
use crate::hidden;
use crate::model::{AccountView, ChatKey, ChatRow, FoundChat, Request, Snapshot, account_labels};
use crate::window::MAIN;

/// The Add Chat buttons: under the list, and in the middle of an empty one.
const ADD_CHAT: &str = "main-add-chat";
const ADD_FIRST_CHAT: &str = "main-add-first-chat";
/// The search field over the list, and the list.
const SEARCH: &str = "main-search";
const CHATS: &str = "main-chats";
/// The folders' chips over the list: All, a folder (`main-folder/<id>`), New Folder (+) and the
/// shown folder's ⋯.
const FOLDER_ALL: &str = "main-folder/all";
const FOLDER: &str = "main-folder/";
const FOLDER_NEW: &str = "main-folder-new";
const FOLDER_MORE: &str = "main-folder-more";
/// Settings' hidden chats: the current phrase, the new one, Save, Forget Hidden Chats.
const SETTINGS_CURRENT: &str = "hidden-current";
const SETTINGS_PHRASE: &str = "hidden-phrase";
const SETTINGS_SAVE: &str = "hidden-save";
const SETTINGS_FORGET: &str = "hidden-forget";
/// The Hide Chat dialog's field and buttons.
const DIALOG_PHRASE: &str = "hide-phrase";
const DIALOG_HIDE: &str = "hide-confirm";
const DIALOG_CANCEL: &str = "hide-cancel";

/// A row's ⋯ item: Available to Claude (part 6).
const CLAUDE_ITEM: &str = "claude";

/// The search's text is tried as the phrase once typing stops this long.
const PHRASE_AFTER: Duration = Duration::from_millis(300);

/// What the list asks the window for.
pub enum Go {
    /// Open the chat's tab.
    Open(ChatKey),
    /// Open the chat's tab with messages to forward over its field (part 5).
    Forward(ChatKey, Forward),
    /// Open a hidden chat's tab: it doesn't come back after a restart.
    OpenHidden(ChatKey),
    /// Close the chat's tab, if it is open (hidden while the hidden chats aren't open here).
    Close(ChatKey),
    /// Close the hidden chats' tabs: the phrase left the search.
    CloseHidden,
    /// The chat was hidden (true) or shown again: its tab's restoring follows.
    MarkHidden(ChatKey, bool),
    /// Start adding an account (the empty window's button).
    AddAccount,
    /// Mute the chat for this many seconds; none — unmute it (a row's ⋯; part 5).
    Mute(ChatKey, Option<i64>),
    /// Stop the chat's bot, asked first (a row's ⋯; part 5).
    StopBot(ChatKey),
}

/// The Add Chat popup: a search per account; or the Forward picker (part 5).
struct Popup {
    /// The picker's id.
    id: u64,
    /// The accounts' searches, in the accounts' order.
    searches: Vec<Searched>,
    /// The Forward picker's messages: one search, of their account's chats; the choice gets
    /// them.
    forward: Option<Forward>,
    /// The query typed: the Forward picker shows the list's chats that have it.
    query: String,
}

/// An account's search of the Add Chat popup.
struct Searched {
    account: String,
    /// The heading of its chats: the account's first name, with two accounts or more.
    group: Option<String>,
    /// The latest query's request: older answers are stale.
    request: u64,
    /// The latest query's answer came.
    answered: bool,
    chats: Vec<FoundChat>,
}

/// What a folder's name is asked for.
enum Naming {
    /// A new folder (+), or one for the chat (Add to Folder… → New Folder…).
    New(Option<ChatKey>),
    Rename(u64),
}

/// A row's kind: its actions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// In the list: Add to Folder…, Hide, Remove from FluxGram.
    Listed,
    /// In the folder shown: Remove from Folder in Remove's place.
    InFolder,
    /// A hidden chat (the hidden chats are open here): marked; Unhide, Remove.
    Hidden,
}

/// The hidden chats' block of Settings → FluxGram: the phrase.
pub const HIDDEN: &str = "hidden-chats";
/// The Hide Chat dialog: the phrase, when the hidden chats aren't open in this window.
pub const HIDE: &str = "hide-chat";

/// The hidden chats in this window.
#[derive(Default)]
struct Hidden {
    /// A phrase is set (the service's word).
    set: bool,
    /// The search's text that opened them; none — they are hidden.
    unlocked: Option<String>,
    /// Their rows, while open.
    rows: Vec<HiddenRow>,
    /// The search's typing: the text is tried once it stops.
    timer: Option<u64>,
    /// Settings: the current phrase and the new one typed, what went wrong, saved, the service
    /// working.
    current: String,
    phrase: String,
    problem: Option<&'static str>,
    saved: bool,
    busy: bool,
    /// The phrase just saved shows as dots; typing shows it again (the author, 2026-11-18:
    /// visible while typed, hidden once saved).
    masked: bool,
    /// The questions on show: Forget Hidden Chats?, Open Settings?
    forget_asked: Option<u64>,
    settings_asked: Option<u64>,
    /// The Hide Chat dialog.
    dialog: Option<HideDialog>,
    /// The views set: an unchanged one isn't set again.
    settings_view: Option<View>,
    dialog_view: Option<View>,
}

/// The Hide Chat dialog: the chat, the phrase typed, a wrong one, the service working.
struct HideDialog {
    key: ChatKey,
    title: String,
    phrase: String,
    wrong: bool,
    busy: bool,
}

pub struct List {
    accounts: Vec<AccountView>,
    /// Each account's rows, as the service sent them; none yet — the list is loading.
    lists: HashMap<String, Vec<ChatRow>>,
    /// FluxGram's folders, as the service sent them.
    folders: Vec<Folder>,
    /// The folder shown in this window (not saved); none — All.
    folder: Option<u64>,
    /// A folder this window has just created (and shows), until the service's folders have it.
    created: Option<Folder>,
    /// A folder's name asked for: the question's id, and what for.
    naming: Option<(u64, Naming)>,
    /// Delete the folder? on show: the question's id, the folder.
    delete_asked: Option<(u64, u64)>,
    /// The folders' menu of a chat (its id, the chat); the shown folder's ⋯ (its id, the folder).
    folder_menu: Option<(u64, ChatKey)>,
    folder_actions: Option<(u64, u64)>,
    /// A row's ⋯ menu on show, and its Mute choice (part 5): the picker, the chat.
    row_menu: Option<(u64, ChatKey)>,
    mute_menu: Option<(u64, ChatKey)>,
    /// The search field's text.
    filter: String,
    /// The window is on screen: the popup opens under its Add Chat button.
    shown: bool,
    /// The Add Chat button of the view set, if any.
    anchor: Option<&'static str>,
    popup: Option<Popup>,
    /// The last request's id.
    requests: u64,
    /// The titles of chats the popup offered: a tab's title before the list has the chat.
    found_titles: HashMap<ChatKey, String>,
    /// The marks set (the launchpad icon's, the tabs'), by window.
    badges: HashMap<String, String>,
    /// The timer to the next midnight: "14:32" becomes "Yesterday".
    midnight: Option<u64>,
    /// The hidden chats here.
    hidden: Hidden,
    /// The view set, so an unchanged one isn't set again.
    view: Option<View>,
}

impl List {
    pub fn new() -> Self {
        List {
            accounts: Vec::new(),
            lists: HashMap::new(),
            folders: Vec::new(),
            folder: None,
            created: None,
            naming: None,
            delete_asked: None,
            folder_menu: None,
            folder_actions: None,
            row_menu: None,
            mute_menu: None,
            filter: String::new(),
            shown: false,
            anchor: None,
            popup: None,
            requests: 0,
            found_titles: HashMap::new(),
            badges: HashMap::new(),
            midnight: None,
            hidden: Hidden::default(),
            view: None,
        }
    }

    /// The accounts changed (the service's state); the window draws again after.
    pub fn state(&mut self, state: &Snapshot) {
        self.accounts = state.accounts.clone();
        let accounts = &self.accounts;
        self.lists
            .retain(|key, _| accounts.iter().any(|account| &account.key == key));
        self.hidden
            .rows
            .retain(|hidden| accounts.iter().any(|account| account.key == hidden.account));
        self.sync_badges();
    }

    /// An account's list, whole.
    pub fn chats(&mut self, account: &str, rows: Vec<ChatRow>) {
        if self.lists.get(account) == Some(&rows) {
            return;
        }
        self.lists.insert(account.to_string(), rows);
        self.render();
        self.sync_badges();
    }

    /// An account's answer to the Add Chat popup's `request`; a stale one is dropped.
    pub fn found(&mut self, request: u64, chats: Vec<FoundChat>) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let Some(search) = popup
            .searches
            .iter_mut()
            .find(|search| search.request == request)
        else {
            return;
        };
        for chat in &chats {
            self.found_titles
                .insert(ChatKey::new(&search.account, chat.chat), chat.title.clone());
        }
        search.chats = chats;
        search.answered = true;
        let loading = popup.searches.iter().any(|search| !search.answered);
        picker::update(popup.id, items_of(popup, &self.lists), loading);
    }

    /// FluxGram's folders, whole (the service's word).
    pub fn folders(&mut self, folders: Vec<Folder>) {
        if self
            .created
            .as_ref()
            .is_some_and(|created| folders.iter().any(|folder| folder.id == created.id))
        {
            self.created = None;
        }
        self.folders = folders;
        // The folder shown was deleted (here, or in another window): All.
        if self.folder.is_some() && self.current_folder().is_none() {
            self.folder = None;
        }
        self.render();
    }

    /// A folder's name was typed (New Folder, Rename…), or the question cancelled.
    pub fn text_answered(&mut self, id: u64, text: Option<String>) {
        let Some((_, naming)) = self.naming.take_if(|(asked, _)| *asked == id) else {
            return;
        };
        let Some(name) = text.as_deref().and_then(folder_name) else {
            return;
        };
        match naming {
            Naming::New(chat) => {
                let id = new_folder_id(|id| self.folder_by_id(id).is_some());
                if let Some(chat) = &chat {
                    self.list_first(chat);
                }
                let chats: Vec<ChatKey> = chat.into_iter().collect();
                let made_here = chats.is_empty();
                ask(Request::Folders(FolderAsk::Create {
                    id,
                    name: name.clone(),
                    chats: chats.clone(),
                }));
                self.created = Some(Folder { id, name, chats });
                // + shows the new folder; one made for a chat stays where the user is.
                if made_here {
                    self.folder = Some(id);
                }
                self.render();
            }
            Naming::Rename(id) => ask(Request::Folders(FolderAsk::Rename { id, name })),
        }
    }

    /// Add to Folder… (a row's action; a tab's ⋯ — `from_tab`, the menu under it): the folders,
    /// a ✓ on those that have the chat — a choice puts it in or takes it out — and New Folder….
    pub fn folder_menu(&mut self, key: &ChatKey, from_tab: bool) {
        let mut items: Vec<picker::Item> = self
            .all_folders()
            .map(|folder| {
                let icon = if folder.chats.contains(key) {
                    "check"
                } else {
                    "folder"
                };
                picker::item(&folder.id.to_string(), &folder.name).icon(icon)
            })
            .collect();
        items.push(picker::item("new", &tr("New Folder…")).icon("folder-plus"));
        let menu = picker::Picker::menu().items(items);
        let menu = match from_tab {
            true => menu.under(&tab_window(key), MORE),
            false => menu.under(MAIN, &row_anchor(key)),
        };
        self.folder_menu = Some((menu.open(), key.clone()));
    }

    /// A row's ⋯ (part 5): its menu under the row — Mark as Read or Unread, Mute… or Unmute, Add
    /// to Folder… (not a hidden chat's: the folders are plaintext), a bot's Stop Bot or Restart
    /// Bot.
    fn row_menu(&mut self, key: &ChatKey) {
        let hidden = self.is_hidden_row(key);
        let row = self
            .lists
            .get(&key.account)
            .and_then(|rows| rows.iter().find(|row| row.chat == key.chat))
            .or_else(|| {
                self.hidden
                    .rows
                    .iter()
                    .find(|found| found.account == key.account && found.row.chat == key.chat)
                    .map(|found| &found.row)
            });
        let Some(row) = row else {
            return;
        };
        let mut items = vec![
            match row.unread > 0 {
                true => picker::item("read", &tr("Mark as Read")).icon("check"),
                false => picker::item("unread", &tr("Mark as Unread")).icon("icons/unread.svg"),
            },
            match row.muted {
                true => picker::item("unmute", &tr("Unmute")).icon("bell"),
                false => picker::item("mute", &tr("Mute")).icon("icons/muted.svg"),
            },
        ];
        if !hidden {
            items.push(picker::item("folder", &tr("Add to Folder…")).icon("folder-plus"));
            // Claude (part 6): never a hidden chat; ✓ while it is on.
            let icon = if row.claude { "check" } else { "claude" };
            items.push(picker::item(CLAUDE_ITEM, &tr("Available to Claude")).icon(icon));
        }
        if row.bot {
            items.push(match row.blocked {
                true => picker::item("restart-bot", &tr("Restart Bot")).icon("update"),
                false => picker::item("stop-bot", &tr("Stop Bot")).icon("close"),
            });
        }
        let id = picker::Picker::menu()
            .items(items)
            .under(MAIN, &row_anchor(key))
            .open();
        self.row_menu = Some((id, key.clone()));
    }

    /// The chat of the list is available to Claude (part 6).
    fn claude_on(&self, key: &ChatKey) -> bool {
        self.lists
            .get(&key.account)
            .and_then(|rows| rows.iter().find(|row| row.chat == key.chat))
            .is_some_and(|row| row.claude)
    }

    /// Mute's choice under the row (part 5).
    fn mute_menu(&mut self, key: &ChatKey) {
        let items: Vec<_> = crate::chat::mute_choices()
            .into_iter()
            .map(|(seconds, label)| picker::item(&seconds.to_string(), &tr(label)))
            .collect();
        let id = picker::Picker::menu()
            .items(items)
            .under(MAIN, &row_anchor(key))
            .open();
        self.mute_menu = Some((id, key.clone()));
    }

    /// What the service tells of the hidden chats.
    pub fn hidden_told(&mut self, told: HiddenTold) -> Vec<Go> {
        let mut go = Vec::new();
        match told {
            HiddenTold::State { set } => {
                if self.hidden.set != set {
                    self.hidden.set = set;
                    self.hidden.current.clear();
                    self.hidden.problem = None;
                }
                if !set {
                    go = self.lock(false);
                }
                self.render();
                self.render_settings();
            }
            // Opened for a text the search still has; otherwise it is hidden again at once.
            HiddenTold::Unlocked { text } if text == self.filter => {
                self.hidden.unlocked = Some(text);
                self.render();
            }
            HiddenTold::Unlocked { .. } => ask(Request::Hidden(HiddenAsk::Lock)),
            HiddenTold::Rows { rows } => {
                if self.hidden.unlocked.is_some() {
                    self.hidden.rows = rows;
                    self.render();
                }
            }
            HiddenTold::Locked => go = self.lock(false),
            HiddenTold::Hid { key } => {
                if self
                    .hidden
                    .dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.key == key)
                {
                    self.hidden.dialog = None;
                    ui::hide(HIDE);
                }
                go.push(if self.hidden.unlocked.is_some() {
                    Go::MarkHidden(key, true)
                } else {
                    Go::Close(key)
                });
            }
            HiddenTold::Unhid { key } => go.push(Go::MarkHidden(key, false)),
            HiddenTold::Saved => {
                self.hidden.busy = false;
                self.hidden.saved = true;
                self.hidden.current.clear();
                // The phrase stays in its field as dots until typed over.
                self.hidden.masked = true;
                self.render_settings();
            }
            HiddenTold::WrongPhrase { hide: true } => {
                if let Some(dialog) = &mut self.hidden.dialog {
                    dialog.busy = false;
                    dialog.wrong = true;
                }
                self.render_dialog();
            }
            HiddenTold::WrongPhrase { hide: false } => {
                self.hidden.busy = false;
                self.hidden.problem = Some("The current phrase is wrong.");
                self.render_settings();
            }
        }
        go
    }

    /// The user did something in the hidden chats' block of Settings.
    pub fn hidden_ui(&mut self, element: &str, event: UiEvent) {
        match (element, event) {
            (SETTINGS_CURRENT, UiEvent::Changed(text)) => self.hidden.current = text,
            (SETTINGS_PHRASE, UiEvent::Changed(text)) => self.hidden.phrase = text,
            // Save says itself what went wrong (a short phrase): nothing below clears it.
            (SETTINGS_SAVE, UiEvent::Clicked) => return self.save_phrase(),
            (SETTINGS_CURRENT | SETTINGS_PHRASE, UiEvent::Submitted(text)) => {
                if element == SETTINGS_CURRENT {
                    self.hidden.current = text;
                } else {
                    self.hidden.phrase = text;
                }
                return self.save_phrase();
            }
            (SETTINGS_FORGET, UiEvent::Clicked) => {
                let id = Ask::warning(&tr("Forget hidden chats?"))
                    .message(&tr(
                        "The list of hidden chats goes; the chats stay in Telegram.",
                    ))
                    .danger(&tr("Forget"))
                    .cancel(&tr("Cancel"))
                    .send();
                self.hidden.forget_asked = Some(id);
                return;
            }
            _ => return,
        }
        // Typing: the old word goes, and the phrase shows as it is typed.
        if !self.hidden.busy {
            self.hidden.problem = None;
        }
        self.hidden.saved = false;
        self.hidden.masked = false;
        self.render_settings();
    }

    /// The user did something in the Hide Chat dialog.
    pub fn hide_dialog_ui(&mut self, element: &str, event: UiEvent) {
        let Some(dialog) = &mut self.hidden.dialog else {
            return;
        };
        if let (DIALOG_PHRASE, UiEvent::Submitted(text)) = (element, &event) {
            dialog.phrase = text.clone();
        }
        match (element, event) {
            (DIALOG_PHRASE, UiEvent::Changed(text)) => {
                dialog.phrase = text;
                dialog.wrong = false;
            }
            (DIALOG_HIDE, UiEvent::Clicked) | (DIALOG_PHRASE, UiEvent::Submitted(_))
                if !dialog.busy =>
            {
                // A phrase too short to be one is simply wrong: said at once, not silently
                // ignored.
                if hidden::can_be_phrase(&dialog.phrase) {
                    dialog.busy = true;
                    ask(Request::Hidden(HiddenAsk::Hide {
                        key: dialog.key.clone(),
                        phrase: Some(dialog.phrase.clone()),
                    }));
                } else {
                    dialog.wrong = true;
                }
            }
            (DIALOG_CANCEL, UiEvent::Clicked) => {
                self.hidden.dialog = None;
                ui::hide(HIDE);
                return;
            }
            _ => return,
        }
        self.render_dialog();
    }

    /// The Hide Chat dialog closed (Esc, ×): what was typed goes.
    pub fn hide_dialog_closed(&mut self) {
        self.hidden.dialog = None;
    }

    /// The answer to a question of the list (Delete the folder?, Forget Hidden Chats?, Open
    /// Settings?); false — not the list's.
    pub fn dialog_answered(&mut self, id: u64, answer: Option<u32>) -> bool {
        if let Some((_, folder)) = self.delete_asked.take_if(|(asked, _)| *asked == id) {
            if answer == Some(0) {
                ask(Request::Folders(FolderAsk::Delete { id: folder }));
                if self.folder == Some(folder) {
                    self.folder = None;
                    self.render();
                }
            }
            return true;
        }
        if self.hidden.forget_asked == Some(id) {
            self.hidden.forget_asked = None;
            if answer == Some(0) {
                ask(Request::Hidden(HiddenAsk::Forget));
            }
            return true;
        }
        if self.hidden.settings_asked == Some(id) {
            self.hidden.settings_asked = None;
            if answer == Some(0) {
                system::open_settings();
            }
            return true;
        }
        false
    }

    /// Hide Chat (a row's action, a tab's ⋯): with the open hidden list, else the phrase is asked
    /// for; without a phrase set — where to set it.
    pub fn hide(&mut self, key: &ChatKey) {
        if !self.hidden.set {
            let id = Ask::info(&tr("Set a phrase first"))
                .message(&tr(
                    "Hidden chats show only for a phrase typed in the chats' search. It is set in Settings → FluxGram.",
                ))
                .primary(&tr("Open Settings"))
                .cancel(&tr("Cancel"))
                .send();
            self.hidden.settings_asked = Some(id);
            return;
        }
        if self.hidden.unlocked.is_some() {
            ask(Request::Hidden(HiddenAsk::Hide {
                key: key.clone(),
                phrase: None,
            }));
            return;
        }
        self.hidden.dialog = Some(HideDialog {
            key: key.clone(),
            title: self.title(key).unwrap_or_else(|| tr("Chat")),
            phrase: String::new(),
            wrong: false,
            busy: false,
        });
        self.hidden.dialog_view = None;
        self.render_dialog();
        ui::show(HIDE);
    }

    /// Unhide (a hidden row's action, a tab's ⋯): back to the list.
    pub fn unhide(&mut self, key: &ChatKey) {
        if self.hidden.unlocked.is_some() {
            ask(Request::Hidden(HiddenAsk::Unhide { key: key.clone() }));
        }
    }

    /// The service started again: it knows nothing of the hidden chats open here — the search's
    /// phrase is tried again.
    pub fn service_started(&mut self) {
        if self.hidden.unlocked.is_some() {
            self.try_phrase();
        }
    }

    /// The window came into sight, or went out of it.
    pub fn shown(&mut self, shown: bool) {
        self.shown = shown;
    }

    /// A chat's title, as the list has it: the tab's title before the chat's header comes.
    pub fn title(&self, key: &ChatKey) -> Option<String> {
        self.lists
            .get(&key.account)
            .and_then(|rows| rows.iter().find(|row| row.chat == key.chat))
            .map(|row| row.title.clone())
            .or_else(|| {
                self.hidden
                    .rows
                    .iter()
                    .find(|hidden| hidden.account == key.account && hidden.row.chat == key.chat)
                    .map(|hidden| hidden.row.title.clone())
            })
            .or_else(|| self.found_titles.get(key).cloned())
    }

    /// The chat's tab has just opened: its mark is set again (one set before the tab was there
    /// went nowhere).
    pub fn tab_opened(&mut self, key: &ChatKey) {
        self.badges.remove(&tab_window(key));
        self.sync_badges();
    }

    /// Add Chat… (the command, the window's button): the popup of every account's people and
    /// groups.
    pub fn add_chat(&mut self) {
        if self.accounts.is_empty() {
            notify::info(&tr("Sign in to Telegram to add chats."));
            return;
        }
        let mut popup = picker::Picker::new(&tr("Search people and groups"))
            .live()
            .loading()
            .empty_text(&tr("No people or groups found"));
        if let Some(anchor) = self.anchor.filter(|_| self.shown) {
            popup = popup.under(MAIN, anchor);
        }
        let several = self.accounts.len() > 1;
        let labels = account_labels(&self.accounts);
        let searches = self
            .accounts
            .iter()
            .map(|account| Searched {
                account: account.key.clone(),
                group: several.then(|| labels[&account.key].clone()),
                request: 0,
                answered: false,
                chats: Vec::new(),
            })
            .collect();
        self.popup = Some(Popup {
            id: popup.open(),
            searches,
            forward: None,
            query: String::new(),
        });
        self.search_all("");
    }

    /// Forward (part 5): the picker of a chat of the messages' account (Telegram forwards only
    /// within one) — Saved Messages on top, then FluxGram's list, then the account's other chats
    /// the search finds.
    pub fn forward(&mut self, forward: Forward) {
        let popup = picker::Picker::new(&tr("Forward to…"))
            .live()
            .loading()
            .empty_text(&tr("No chats found"));
        let searches = vec![Searched {
            account: forward.from.account.clone(),
            group: None,
            request: 0,
            answered: false,
            chats: Vec::new(),
        }];
        let popup = Popup {
            id: popup.open(),
            searches,
            forward: Some(forward),
            query: String::new(),
        };
        // The list's chats at once; the search's join them.
        picker::update(popup.id, items_of(&popup, &self.lists), true);
        self.popup = Some(popup);
        self.search_all("");
    }

    /// The popup's query changed.
    pub fn picker_query(&mut self, picker: u64, query: &str) {
        let Some(popup) = self.popup.as_mut().filter(|popup| popup.id == picker) else {
            return;
        };
        popup.query = query.trim().to_string();
        // What it shows stays until the answers: the spinner says they're on their way.
        picker::update(popup.id, items_of(popup, &self.lists), true);
        self.search_all(query.trim());
    }

    /// The popup's query to every account: a request each.
    fn search_all(&mut self, query: &str) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        for search in &mut popup.searches {
            self.requests += 1;
            search.request = self.requests;
            search.answered = false;
            ask(Request::SearchChats {
                account: search.account.clone(),
                query: query.to_string(),
                request: search.request,
            });
        }
    }

    /// A popup closed: the Add Chat popup's chat, a folders' menu's choice, or none.
    pub fn picker_chosen(&mut self, picker: u64, key: Option<String>) -> Vec<Go> {
        if let Some((_, chat)) = self.row_menu.take_if(|(id, _)| *id == picker) {
            match key.as_deref() {
                Some("read") => ask(Request::MarkRead {
                    key: chat,
                    read: true,
                }),
                Some("unread") => ask(Request::MarkRead {
                    key: chat,
                    read: false,
                }),
                Some("mute") => self.mute_menu(&chat),
                Some("unmute") => return vec![Go::Mute(chat, None)],
                Some("folder") => self.folder_menu(&chat, false),
                Some(CLAUDE_ITEM) => {
                    let on = !self.claude_on(&chat);
                    ask(Request::Claude(ClaudeAsk::Set { key: chat, on }))
                }
                // A bot's (part 5): Stop asks first, as the tab's ⋯ does.
                Some("stop-bot") => return vec![Go::StopBot(chat)],
                Some("restart-bot") => ask(Request::BotStart {
                    key: chat,
                    restart: true,
                }),
                _ => {}
            }
            return Vec::new();
        }
        if let Some((_, chat)) = self.mute_menu.take_if(|(id, _)| *id == picker) {
            return match key.and_then(|seconds| seconds.parse::<i64>().ok()) {
                Some(seconds) => vec![Go::Mute(chat, Some(seconds))],
                None => Vec::new(),
            };
        }
        if let Some((_, chat)) = self.folder_menu.take_if(|(id, _)| *id == picker) {
            match key.as_deref() {
                Some("new") => self.ask_name(Naming::New(Some(chat))),
                Some(id) => {
                    let folder = id.parse().ok().and_then(|id| self.folder_by_id(id));
                    if let Some(folder) = folder {
                        let (id, key) = (folder.id, chat.clone());
                        if folder.chats.contains(&chat) {
                            ask(Request::Folders(FolderAsk::Remove { id, key }));
                        } else {
                            self.list_first(&chat);
                            ask(Request::Folders(FolderAsk::Add { id, key }));
                        }
                    }
                }
                None => {}
            }
            return Vec::new();
        }
        if let Some((_, folder)) = self.folder_actions.take_if(|(id, _)| *id == picker) {
            match key.as_deref() {
                Some("rename") => self.ask_name(Naming::Rename(folder)),
                Some("delete") => self.confirm_delete(folder),
                _ => {}
            }
            return Vec::new();
        }
        let Some(popup) = self.popup.take_if(|popup| popup.id == picker) else {
            return Vec::new();
        };
        let Some(key) = key.as_deref().and_then(ChatKey::parse) else {
            return Vec::new();
        };
        if let Some(forward) = popup.forward {
            return vec![Go::Forward(key, forward)];
        }
        let added = popup
            .searches
            .iter()
            .filter(|search| search.account == key.account)
            .flat_map(|search| &search.chats)
            .any(|found| found.chat == key.chat && found.added);
        if !added {
            ask(Request::AddChat { key: key.clone() });
        }
        // In a folder: the chat goes into it too (after the list has it: the requests keep their
        // order).
        if let Some(folder) = self
            .current_folder()
            .filter(|folder| !folder.chats.contains(&key))
        {
            ask(Request::Folders(FolderAsk::Add {
                id: folder.id,
                key: key.clone(),
            }));
        }
        // The chat is shown: the search that would hide it goes.
        let title = self.title(&key).unwrap_or_default();
        if !self.filter.is_empty() && !matches(&title, &self.filter) {
            self.filter.clear();
            self.render();
        }
        vec![Go::Open(key)]
    }

    /// A timer of the list fired (one it started); false — not the list's.
    pub fn timer(&mut self, timer: u64) -> bool {
        if self.hidden.timer == Some(timer) {
            self.hidden.timer = None;
            self.try_phrase();
            return true;
        }
        if self.midnight != Some(timer) {
            return false;
        }
        self.midnight = None;
        self.render();
        true
    }

    /// The user did something in the window.
    pub fn ui(&mut self, element: &str, event: UiEvent) -> Vec<Go> {
        match (element, event) {
            ("main-add", UiEvent::Clicked) => vec![Go::AddAccount],
            (ADD_CHAT | ADD_FIRST_CHAT, UiEvent::Clicked) => {
                self.add_chat();
                Vec::new()
            }
            (SEARCH, UiEvent::Changed(text)) => self.search_changed(text, false),
            // ↵ in the search: the text tried at once, and the first chat found.
            (SEARCH, UiEvent::Submitted(text)) => {
                let mut go = self.search_changed(text, true);
                let first = if self.hidden.unlocked.is_some() {
                    self.hidden_rows()
                        .first()
                        .map(|(account, row)| Go::OpenHidden(ChatKey::new(account, row.chat)))
                } else {
                    self.shown_rows()
                        .into_iter()
                        .find(|(_, row)| matches(&row.title, &self.filter))
                        .map(|(account, row)| Go::Open(ChatKey::new(account, row.chat)))
                };
                go.extend(first);
                go
            }
            (CHATS, UiEvent::Activated(key)) => match ChatKey::parse(&key) {
                Some(key) if self.is_hidden_row(&key) => vec![Go::OpenHidden(key)],
                Some(key) => vec![Go::Open(key)],
                None => Vec::new(),
            },
            (CHATS, UiEvent::RowAction((key, action))) => {
                match (ChatKey::parse(&key), action.as_str()) {
                    (Some(key), "remove") => ask(Request::RemoveChat { key }),
                    (Some(key), "hide") => self.hide(&key),
                    (Some(key), "unhide") => self.unhide(&key),
                    (Some(key), "folder") => self.folder_menu(&key, false),
                    (Some(key), "more") => self.row_menu(&key),
                    (Some(key), "unfolder") => {
                        if let Some(id) = self.current_folder().map(|folder| folder.id) {
                            ask(Request::Folders(FolderAsk::Remove { id, key }));
                        }
                    }
                    _ => {}
                }
                Vec::new()
            }
            (FOLDER_NEW, UiEvent::Clicked) => {
                self.ask_name(Naming::New(None));
                Vec::new()
            }
            (FOLDER_MORE, UiEvent::Clicked) => {
                self.folder_more();
                Vec::new()
            }
            (element, UiEvent::Clicked) => {
                // A folder's chip (All: none).
                if let Some(folder) = element.strip_prefix(FOLDER) {
                    let folder = folder.parse::<u64>().ok();
                    if self.folder != folder {
                        self.folder = folder;
                        self.render();
                    }
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Draws the window.
    pub fn render(&mut self) {
        let (root, anchor) = if self.accounts.is_empty() {
            let root =
                column(
                    "main-root",
                    [
                        text(
                            "main-empty",
                            [span(&tr("Sign in to Telegram to see your chats here."))
                                .tone(Tone::Dim)],
                        ),
                        row(
                            "main-buttons",
                            [button("main-add", &tr("Add Account"))
                                .icon("plus")
                                .primary()
                                .into()],
                        ),
                    ],
                );
            (root, None)
        } else {
            let (root, anchor) = self.list_view();
            (root, Some(anchor))
        };
        self.anchor = anchor;
        let view = root.into_view();
        self.sync_midnight();
        // Settings' block has its view before its page opens.
        if self.hidden.settings_view.is_none() {
            self.render_settings();
        }
        if self.view.as_ref() == Some(&view) {
            return;
        }
        ui::set_view(MAIN, &view);
        self.view = Some(view);
    }

    // --- Inside ---

    /// The list of every account's chats: the folders, the search, the rows of the folder shown
    /// (or the hidden chats), Add Chat; with the Add Chat button the popup opens under.
    fn list_view(&self) -> (Element, &'static str) {
        let open = self.hidden.unlocked.is_some();
        let loading = self
            .accounts
            .iter()
            .any(|account| !self.lists.contains_key(&account.key));
        let rows = self.rows();
        let no_chats = !loading && rows.is_empty();
        let mut children = Vec::new();
        // The folders, once there is something to put in them.
        if !no_chats || !self.folders.is_empty() || self.created.is_some() {
            children.push(self.chips(&rows));
        }
        let folder = self.current_folder();
        let shown: Vec<(&str, &ChatRow)> = match folder {
            None => rows,
            Some(folder) => rows
                .into_iter()
                .filter(|(account, row)| in_folder(folder, account, row.chat))
                .collect(),
        };
        // The search, where a phrase may be typed, when one is set.
        let search = || text_field(SEARCH, &self.filter, Some(&tr("Search chats")));
        let quiet = !open && self.filter.is_empty();
        let anchor = if no_chats && folder.is_none() && quiet {
            // No chats yet: what to do about it, in the middle.
            if self.hidden.set {
                children.push(search());
            }
            children.push(text(
                "main-no-chats",
                [span(&tr("Add the chats you want to see in Flux.")).tone(Tone::Dim)],
            ));
            children.push(add_first_chat());
            ADD_FIRST_CHAT
        } else if folder.is_some() && shown.is_empty() && !loading && quiet {
            // An empty folder: how to fill it.
            if self.hidden.set {
                children.push(search());
            }
            children.push(text(
                "main-folder-empty",
                [span(&tr("Add chats to this folder"))],
            ));
            children.push(text(
                "main-folder-how",
                [span(&tr("From a chat's row or its ⋯ menu: Add to Folder.")).tone(Tone::Dim)],
            ));
            children.push(add_first_chat());
            ADD_FIRST_CHAT
        } else {
            // The list, or its place while it loads.
            children.push(search());
            if open {
                children.push(self.hidden_tree());
            } else {
                let kind = if folder.is_some() {
                    RowKind::InFolder
                } else {
                    RowKind::Listed
                };
                children.push(self.tree(&shown, kind, loading));
            }
            children.push(row(
                "main-footer",
                [button(ADD_CHAT, &tr("Add Chat")).icon("plus").into()],
            ));
            ADD_CHAT
        };
        (column("main-root", children), anchor)
    }

    /// Every account's rows in one list ([`merge`]): the accounts in their order.
    fn rows(&self) -> Vec<(&str, &ChatRow)> {
        merge(self.accounts.iter().flat_map(|account| {
            let rows = self.lists.get(&account.key).map(Vec::as_slice);
            rows.unwrap_or_default()
                .iter()
                .map(|row| (account.key.as_str(), row))
        }))
    }

    /// The rows of the folder shown (every row in All).
    fn shown_rows(&self) -> Vec<(&str, &ChatRow)> {
        let rows = self.rows();
        match self.current_folder() {
            None => rows,
            Some(folder) => rows
                .into_iter()
                .filter(|(account, row)| in_folder(folder, account, row.chat))
                .collect(),
        }
    }

    /// The hidden chats open here (none while the phrase isn't in the search): the profile
    /// dialog offers Show in List for them (part 5).
    pub fn hidden_chats(&self) -> Vec<ChatKey> {
        if self.hidden.unlocked.is_none() {
            return Vec::new();
        }
        self.hidden
            .rows
            .iter()
            .map(|hidden| ChatKey::new(&hidden.account, hidden.row.chat))
            .collect()
    }

    /// The hidden chats' rows of every account in one list, while open here.
    fn hidden_rows(&self) -> Vec<(&str, &ChatRow)> {
        merge(
            self.hidden
                .rows
                .iter()
                .map(|hidden| (hidden.account.as_str(), &hidden.row)),
        )
    }

    /// The folders as shown: the service's, then one created here that it hasn't sent yet.
    fn all_folders(&self) -> impl Iterator<Item = &Folder> {
        self.folders.iter().chain(&self.created)
    }

    fn folder_by_id(&self, id: u64) -> Option<&Folder> {
        self.all_folders().find(|folder| folder.id == id)
    }

    /// The folder shown; none — All.
    fn current_folder(&self) -> Option<&Folder> {
        self.folder.and_then(|id| self.folder_by_id(id))
    }

    /// The folders' chips: All, the user's folders in their order — each with its unread count
    /// (All's is the launchpad icon's, beside it) — New Folder (+), and the shown folder's ⋯ after
    /// them (at the end of a line, when the chips wrap, it would stand alone on the next).
    fn chips(&self, rows: &[(&str, &ChatRow)]) -> Element {
        let current = self.current_folder().map(|folder| folder.id);
        let chip = |id: &str, name: &str, selected: bool, unread: u32| -> Element {
            let mut chip = button(id, name);
            if selected {
                chip = chip.primary();
            }
            if unread > 0 {
                chip = chip.badge(&count_text(unread));
            }
            chip.into()
        };
        let mut chips = vec![chip(FOLDER_ALL, &tr("All"), current.is_none(), 0)];
        for folder in self.all_folders() {
            let rows = rows
                .iter()
                .filter(|(account, row)| in_folder(folder, account, row.chat))
                .map(|(_, row)| *row);
            chips.push(chip(
                &format!("{FOLDER}{}", folder.id),
                &folder.name,
                current == Some(folder.id),
                unread(rows),
            ));
        }
        chips.push(icon_button(FOLDER_NEW, "plus", &tr("New Folder")));
        if current.is_some() {
            chips.push(icon_button(FOLDER_MORE, "more", &tr("More")));
        }
        row("main-folders", chips)
    }

    /// The name of a new folder, or of the folder renamed.
    fn ask_name(&mut self, naming: Naming) {
        let question = match &naming {
            Naming::New(_) => AskText::new(&tr("New folder"), &tr("Create")),
            Naming::Rename(id) => {
                let name = self
                    .folder_by_id(*id)
                    .map(|folder| folder.name.clone())
                    .unwrap_or_default();
                AskText::new(&tr("Rename folder"), &tr("Rename")).text(&name)
            }
        };
        let id = question.placeholder(&tr("Name")).send();
        self.naming = Some((id, naming));
    }

    /// A chat going into a folder is in the list first (a tab opened from a profile may show one
    /// that isn't): the service puts only chats of the list in folders, and takes the requests in
    /// order.
    fn list_first(&self, key: &ChatKey) {
        let listed = self
            .lists
            .get(&key.account)
            .is_some_and(|rows| rows.iter().any(|row| row.chat == key.chat));
        if !listed {
            ask(Request::AddChat { key: key.clone() });
        }
    }

    /// The shown folder's ⋯: Rename…, Delete Folder.
    fn folder_more(&mut self) {
        let Some(folder) = self.current_folder().map(|folder| folder.id) else {
            return;
        };
        let id = picker::Picker::menu()
            .items([
                picker::item("rename", &tr("Rename…")).icon("pencil"),
                picker::item("delete", &tr("Delete Folder")).icon("trash"),
            ])
            .under(MAIN, FOLDER_MORE)
            .open();
        self.folder_actions = Some((id, folder));
    }

    /// Delete Folder: asked first; the chats stay in the list.
    fn confirm_delete(&mut self, folder: u64) {
        let Some(name) = self.folder_by_id(folder).map(|folder| folder.name.clone()) else {
            return;
        };
        let id = Ask::warning(&trf("Delete the folder «{0}»?", &[&name]))
            .message(&tr("The chats stay in the list."))
            .danger(&tr("Delete"))
            .cancel(&tr("Cancel"))
            .send();
        self.delete_asked = Some((id, folder));
    }

    /// The rows that the search lets through.
    fn tree(&self, rows: &[(&str, &ChatRow)], kind: RowKind, loading: bool) -> Element {
        let now = system::now();
        let rows: Vec<TreeRow> = rows
            .iter()
            .filter(|(_, row)| self.filter.is_empty() || matches(&row.title, &self.filter))
            .map(|(account, row)| tree_row(account, row, now, kind))
            .collect();
        // While the list loads, an empty place: no "nothing found" before anything was looked at.
        let empty_text = (!loading).then(|| tr("No chats match the search."));
        Element::raw(
            CHATS,
            ElementKind::Tree(TreeSpec {
                rows,
                empty_text,
                cards: false,
                two_line: true,
            }),
        )
    }

    /// The hidden chats, while open here: only they, marked.
    fn hidden_tree(&self) -> Element {
        let now = system::now();
        let rows: Vec<TreeRow> = self
            .hidden_rows()
            .into_iter()
            .map(|(account, row)| tree_row(account, row, now, RowKind::Hidden))
            .collect();
        Element::raw(
            CHATS,
            ElementKind::Tree(TreeSpec {
                rows,
                empty_text: Some(tr("No hidden chats.")),
                cards: false,
                two_line: true,
            }),
        )
    }

    /// The row is a hidden chat's (the hidden chats are open here).
    fn is_hidden_row(&self, key: &ChatKey) -> bool {
        self.hidden.unlocked.is_some()
            && self
                .hidden
                .rows
                .iter()
                .any(|hidden| hidden.account == key.account && hidden.row.chat == key.chat)
    }

    /// The search's text changed: the hidden chats go when it isn't their phrase any more; a
    /// text long enough is tried once typing stops (`now` — at once: ↵).
    fn search_changed(&mut self, text: String, now: bool) -> Vec<Go> {
        self.filter = text;
        let mut go = Vec::new();
        if self
            .hidden
            .unlocked
            .as_ref()
            .is_some_and(|open| *open != self.filter)
        {
            go = self.lock(true);
        }
        if let Some(timer) = self.hidden.timer.take() {
            timers::cancel(timer);
        }
        if self.hidden.set && self.hidden.unlocked.is_none() && hidden::can_be_phrase(&self.filter)
        {
            if now {
                self.try_phrase();
            } else {
                self.hidden.timer = Some(timers::after(PHRASE_AFTER));
            }
        }
        self.render();
        go
    }

    /// The search's text to the service, to be tried as the phrase.
    fn try_phrase(&self) {
        ask(Request::Hidden(HiddenAsk::Try {
            text: self.filter.clone(),
        }));
    }

    /// The hidden chats are hidden again here: their rows go, their tabs close; `tell` — the
    /// service is told (it didn't decide it).
    fn lock(&mut self, tell: bool) -> Vec<Go> {
        if self.hidden.unlocked.take().is_none() {
            return Vec::new();
        }
        self.hidden.rows.clear();
        if tell {
            ask(Request::Hidden(HiddenAsk::Lock));
        }
        self.render();
        vec![Go::CloseHidden]
    }

    /// Settings' Save: the phrase (and the current one, to change it) to the service.
    fn save_phrase(&mut self) {
        let hidden = &mut self.hidden;
        // The phrase on show (dots) is the one just saved.
        if hidden.busy || hidden.masked {
            return;
        }
        hidden.problem = if !hidden::can_be_phrase(&hidden.phrase) {
            Some("The phrase needs at least 6 characters.")
        } else if hidden.set && hidden.current.trim().is_empty() {
            Some("Type the current phrase too.")
        } else {
            None
        };
        if hidden.problem.is_none() {
            hidden.busy = true;
            ask(Request::Hidden(HiddenAsk::SetPhrase {
                current: hidden.set.then(|| hidden.current.clone()),
                phrase: hidden.phrase.clone(),
            }));
        }
        self.render_settings();
    }

    /// Settings → FluxGram → Hidden Chats: the phrase (and the current one, when set), Save,
    /// Forget Hidden Chats.
    fn render_settings(&mut self) {
        let hidden = &self.hidden;
        let mut children = vec![text(
            "hidden-about",
            [span(&tr(
                "Chats you hide show only when you type this phrase in the chats' search.",
            ))
            .tone(Tone::Dim)],
        )];
        // The phrases are typed in the open, to see what is typed; the one saved shows as dots.
        if hidden.set && !hidden.masked {
            children.push(text_field(
                SETTINGS_CURRENT,
                &hidden.current,
                Some(&tr("Current phrase")),
            ));
        }
        let placeholder = if hidden.set {
            tr("New phrase")
        } else {
            tr("Phrase")
        };
        children.push(if hidden.masked {
            password_field(SETTINGS_PHRASE, &hidden.phrase, Some(&placeholder))
        } else {
            text_field(SETTINGS_PHRASE, &hidden.phrase, Some(&placeholder))
        });
        if let Some(problem) = hidden.problem {
            children.push(text(
                "hidden-problem",
                [span(&tr(problem)).tone(Tone::Error)],
            ));
        } else if hidden.saved {
            children.push(text("hidden-saved", [span(&tr("Saved.")).tone(Tone::Dim)]));
        }
        let mut buttons: Vec<Element> = vec![
            button(SETTINGS_SAVE, &tr("Save"))
                .primary()
                .busy(hidden.busy)
                .into(),
        ];
        if hidden.set {
            buttons.push(spacer("hidden-gap"));
            buttons.push(button(SETTINGS_FORGET, &tr("Forget Hidden Chats")).into());
        }
        children.push(row("hidden-buttons", buttons));
        children.push(text(
            "hidden-lost",
            [span(&tr(
                "A forgotten phrase can't be restored: the list goes, the chats stay in Telegram.",
            ))
            .tone(Tone::Dim)],
        ));
        let view = column("hidden-root", children).into_view();
        if self.hidden.settings_view.as_ref() == Some(&view) {
            return;
        }
        ui::set_view(HIDDEN, &view);
        self.hidden.settings_view = Some(view);
    }

    /// The Hide Chat dialog: the phrase for the chat.
    fn render_dialog(&mut self) {
        let Some(dialog) = &self.hidden.dialog else {
            return;
        };
        let mut children = vec![
            text(
                "hide-about",
                [span(&trf(
                    "Type the phrase of hidden chats to hide {0}.",
                    &[&dialog.title],
                ))],
            ),
            password_field(DIALOG_PHRASE, &dialog.phrase, Some(&tr("Phrase"))),
        ];
        if dialog.wrong {
            children.push(text(
                "hide-error",
                [span(&tr("Wrong phrase.")).tone(Tone::Error)],
            ));
        }
        children.push(row(
            "hide-buttons",
            [
                spacer("hide-gap"),
                button(DIALOG_CANCEL, &tr("Cancel")).into(),
                button(DIALOG_HIDE, &tr("Hide"))
                    .primary()
                    .busy(dialog.busy)
                    .enabled(hidden::can_be_phrase(&dialog.phrase))
                    .into(),
            ],
        ));
        let view = column("hide-root", children).into_view();
        if self.hidden.dialog_view.as_ref() == Some(&view) {
            return;
        }
        ui::set_view(HIDE, &view);
        self.hidden.dialog_view = Some(view);
    }

    /// The marks: the launchpad icon counts the unread messages of the chats that aren't muted;
    /// such a chat's tab has a dot. Only what changed is set.
    fn sync_badges(&mut self) {
        let mut wanted: HashMap<String, String> = HashMap::new();
        let total = unread(self.lists.values().flatten());
        if total > 0 {
            wanted.insert(MAIN.to_string(), count_text(total));
        }
        for (account, rows) in &self.lists {
            for row in rows.iter().filter(|row| row.unread > 0 && !row.muted) {
                wanted.insert(
                    tab_window(&ChatKey::new(account, row.chat)),
                    row.unread.to_string(),
                );
            }
        }
        for (window, badge) in &wanted {
            if self.badges.get(window) != Some(badge) {
                ui::set_badge(window, Some(badge));
            }
        }
        for window in self.badges.keys() {
            if !wanted.contains_key(window) {
                ui::set_badge(window, None);
            }
        }
        self.badges = wanted;
    }

    /// One timer, to the next local midnight, while there are rows with times.
    fn sync_midnight(&mut self) {
        if self.midnight.is_some() || self.lists.values().all(Vec::is_empty) {
            return;
        }
        let now = system::now();
        let local = LocalTime::at(now, system::utc_offset(now));
        let passed = u64::from(local.hour * 3600 + local.minute * 60 + local.second);
        let left = 86_400u64.saturating_sub(passed) + 1;
        self.midnight = Some(timers::after(Duration::from_secs(left)));
    }
}

fn ask(request: Request) {
    let _ = service::send_json(&request);
}

/// The Add Chat button in the middle of an empty list (or folder).
fn add_first_chat() -> Element {
    row(
        "main-no-chats-buttons",
        [button(ADD_FIRST_CHAT, &tr("Add Chat"))
            .icon("plus")
            .primary()
            .into()],
    )
}

/// The rows of several accounts in one list: the pinned ones first — each account's in
/// Telegram's order, the accounts in theirs — then the rest by their last message, newest first
/// (a chat without messages last).
pub fn merge<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a ChatRow)>,
) -> Vec<(&'a str, &'a ChatRow)> {
    let mut rows: Vec<(&str, &ChatRow)> = rows.into_iter().collect();
    // Stable: the pinned keep their order, and so do the chats of the same moment.
    rows.sort_by_key(|(_, row)| {
        let date = if row.pinned { 0 } else { row.date };
        (!row.pinned, std::cmp::Reverse(date))
    });
    rows
}

/// The folder has the account's chat.
fn in_folder(folder: &Folder, account: &str, chat: i64) -> bool {
    folder
        .chats
        .iter()
        .any(|key| key.chat == chat && key.account == account)
}

/// The unread messages of the chats that aren't muted.
fn unread<'a>(rows: impl IntoIterator<Item = &'a ChatRow>) -> u32 {
    rows.into_iter()
        .filter(|row| !row.muted)
        .map(|row| row.unread)
        .sum()
}

/// A new folder's id: random (two windows may make one at the same moment), not `taken`, and
/// exact as a JSON number anywhere.
fn new_folder_id(taken: impl Fn(u64) -> bool) -> u64 {
    const EXACT: u64 = (1 << 53) - 1;
    let mut fallback = system::now().unsigned_abs();
    loop {
        let id = getrandom::u64().unwrap_or_else(|_| {
            fallback += 1;
            fallback
        }) & EXACT;
        if id != 0 && !taken(id) {
            return id;
        }
    }
}

/// A popup's items: the Add Chat popup's, or the Forward picker's.
fn items_of(popup: &Popup, lists: &HashMap<String, Vec<ChatRow>>) -> Vec<picker::Item> {
    match (&popup.forward, popup.searches.first()) {
        (Some(_), Some(search)) => forward_items(
            &search.account,
            lists.get(&search.account).map_or(&[][..], Vec::as_slice),
            &search.chats,
            &popup.query,
        ),
        _ => popup_items(&popup.searches),
    }
}

/// The Forward picker's items (part 5): Saved Messages, the list's chats that have the query,
/// then the other chats found — each once.
fn forward_items(
    account: &str,
    rows: &[ChatRow],
    found: &[FoundChat],
    query: &str,
) -> Vec<picker::Item> {
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    let found_item = |chat: &FoundChat| {
        let item = picker::item(&ChatKey::new(account, chat.chat).tab(), &chat.title)
            .avatar(chat.avatar.clone());
        match &chat.detail {
            Some(detail) => item.detail(detail),
            None => item,
        }
    };
    for chat in found.iter().filter(|chat| chat.saved) {
        if seen.insert(chat.chat) {
            items.push(found_item(chat));
        }
    }
    for row in rows.iter().filter(|row| matches(&row.title, query)) {
        if seen.insert(row.chat) {
            let key = ChatKey::new(account, row.chat).tab();
            items.push(picker::item(&key, &row.title).avatar(row.avatar.clone()));
        }
    }
    for chat in found {
        if seen.insert(chat.chat) {
            items.push(found_item(chat));
        }
    }
    items
}

/// The Add Chat popup's items: each account's chats in turn, under its first name when there
/// are several.
fn popup_items(searches: &[Searched]) -> Vec<picker::Item> {
    searches
        .iter()
        .flat_map(|search| {
            search.chats.iter().map(|chat| {
                let item = item(&search.account, chat);
                match &search.group {
                    Some(group) => item.group(group),
                    None => item,
                }
            })
        })
        .collect()
}

/// The window of a chat's tab: `chat/<account>:<chat>`.
fn tab_window(key: &ChatKey) -> String {
    format!("{CHAT}/{}", key.tab())
}

/// A count in a pill: "99+" past 99.
fn count_text(count: u32) -> String {
    if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    }
}

/// The title has the search's text, in any case.
fn matches(title: &str, filter: &str) -> bool {
    title
        .to_lowercase()
        .contains(filter.trim().to_lowercase().as_str())
}

/// A chat's row: the picture, the title with its marks and the time; the last message (or the
/// draft) with the unread count; the actions of its `kind` — a hidden chat's is marked, and
/// offers Unhide.
fn tree_row(account: &str, chat: &ChatRow, now: i64, kind: RowKind) -> TreeRow {
    let hidden = kind == RowKind::Hidden;
    let mut subtitle: Vec<Span> = Vec::new();
    match &chat.draft {
        Some(draft) => {
            subtitle.push(span(&format!("{} ", tr("Draft:"))).tone(Tone::Error));
            subtitle.push(span(draft));
        }
        None => {
            match chat.delivery {
                Some(Delivery::Sent) => subtitle.push(span("✓ ").tone(Tone::Dim)),
                Some(Delivery::Read) => subtitle.push(span("✓✓ ").tone(Tone::Accent)),
                Some(Delivery::Failed) => subtitle.push(span("! ").tone(Tone::Error)),
                Some(Delivery::Sending) | None => {}
            }
            subtitle.push(span(chat.last.as_deref().unwrap_or_default()));
        }
    }
    // A mention is Flux's own "@" mark before the count.
    let badge = match (chat.unread, chat.mentions) {
        (0, 0) => None,
        (unread, _) => Some(count_text(unread.max(1))),
    };
    let mut label_icons = Vec::new();
    if hidden {
        label_icons.push("icons/hidden.svg".to_string());
    }
    if chat.pinned {
        label_icons.push("icons/pin.svg".to_string());
    }
    if chat.muted {
        label_icons.push("icons/muted.svg".to_string());
    }
    // Available to Claude (part 6): Flux draws `claude` in Claude's color.
    if chat.claude && !hidden {
        label_icons.push("claude".to_string());
    }
    TreeRow {
        key: ChatKey::new(account, chat.chat).tab(),
        parent: None,
        icon: None,
        label: vec![span(&chat.title)],
        detail: when(chat.date, now, system::utc_offset).map(|when| when.text()),
        badge,
        expanded: false,
        subtitle: Some(subtitle),
        tags: Vec::new(),
        actions: row_actions(kind),
        avatar: Some(chat.avatar.clone()),
        badge_tone: Some(if chat.muted {
            Tone::Muted
        } else {
            Tone::Accent
        }),
        label_icons,
        mention: chat.mentions > 0,
    }
}

/// A row's actions: ⋯ (its menu: read, mute, Add to Folder…), Hide, and Remove from FluxGram —
/// in a folder, Remove from Folder in its place (taking a chat out of the folder shown is what its
/// last button does there; and Flux keeps every action's place on a row, so a fourth would leave
/// the title no room); a hidden chat's — ⋯, Unhide, Remove (the folders are plaintext: a hidden
/// chat never goes into one).
fn row_actions(kind: RowKind) -> Vec<RowAction> {
    let action = |id: &str, icon: &str, tooltip: &str| RowAction {
        id: id.to_string(),
        label: None,
        icon: Some(icon.to_string()),
        tooltip: Some(tr(tooltip)),
        primary: false,
    };
    // ⋯ first (part 5): read, mute, the folders.
    let mut actions = vec![action("more", "more", "More")];
    match kind {
        RowKind::Hidden => actions.push(action("unhide", "icons/eye.svg", "Unhide Chat")),
        RowKind::Listed | RowKind::InFolder => {
            actions.push(action("hide", "icons/hidden.svg", "Hide Chat"));
        }
    }
    actions.push(if kind == RowKind::InFolder {
        action("unfolder", "icons/folder-minus.svg", "Remove from Folder")
    } else {
        action("remove", "close", "Remove from FluxGram")
    });
    actions
}

/// A picker under a chat's row: the list's row by its key (under the list while the row isn't
/// drawn).
fn row_anchor(key: &ChatKey) -> String {
    format!("{CHATS}/{}", key.tab())
}

/// A popup's item, named by the chat's tab key: the picture, the title, "@anna" or "5 members",
/// and "Added" for a chat in the list already.
fn item(account: &str, chat: &FoundChat) -> picker::Item {
    let key = ChatKey::new(account, chat.chat).tab();
    let mut item = picker::item(&key, &chat.title).avatar(chat.avatar.clone());
    if let Some(detail) = &chat.detail {
        item = item.detail(detail);
    }
    if chat.added {
        item = item.badge(&tr("Added"));
    }
    item
}

/// When a row's last message was, as the list says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum When {
    /// Today: "14:32".
    Time(String),
    Yesterday,
    /// Within the week: its day, Monday — 0.
    Weekday(u32),
    /// Earlier: "12.11.25".
    Date(String),
}

impl When {
    fn text(&self) -> String {
        match self {
            When::Time(time) | When::Date(time) => time.clone(),
            When::Yesterday => tr("Yesterday"),
            When::Weekday(day) => {
                tr(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][*day as usize % 7])
            }
        }
    }
}

/// `date` (seconds since 1970, UTC) seen `now`, in the local time `offset` gives; none for no
/// date.
pub fn when(date: i64, now: i64, offset: impl Fn(i64) -> i32) -> Option<When> {
    if date <= 0 {
        return None;
    }
    let then = LocalTime::at(date, offset(date));
    let today = LocalTime::at(now, offset(now));
    Some(match today.day_number() - then.day_number() {
        // Today (or a moment ahead of this clock).
        ..=0 => When::Time(then.hm()),
        1 => When::Yesterday,
        2..=6 => When::Weekday(then.weekday),
        _ => When::Date(format!(
            "{:02}.{:02}.{:02}",
            then.day,
            then.month,
            then.year.rem_euclid(100)
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_when() {
        let moscow = |_| 3 * 3600;
        // 2026-11-18 (Wednesday) 14:32 in Moscow.
        let now = 1_795_001_520;
        assert_eq!(LocalTime::at(now, 3 * 3600).hm(), "14:32");
        assert_eq!(
            when(now - 60, now, moscow),
            Some(When::Time("14:31".into()))
        );
        // Past midnight is yesterday, however close.
        let midnight = now - (14 * 3600 + 32 * 60);
        assert_eq!(
            when(midnight, now, moscow),
            Some(When::Time("00:00".into()))
        );
        assert_eq!(when(midnight - 1, now, moscow), Some(When::Yesterday));
        // Monday of the same week; the Wednesday before is a date.
        assert_eq!(when(now - 2 * 86_400, now, moscow), Some(When::Weekday(0)));
        assert_eq!(when(now - 6 * 86_400, now, moscow), Some(When::Weekday(3)));
        assert_eq!(
            when(now - 7 * 86_400, now, moscow),
            Some(When::Date("11.11.26".into()))
        );
        assert_eq!(when(0, now, moscow), None);
        // The same moment in another zone is another day.
        let tokyo = |_| 9 * 3600;
        assert_eq!(
            when(midnight - 1, now, tokyo),
            Some(When::Time("05:59".into()))
        );
    }

    #[test]
    fn the_forward_picker_has_saved_messages_then_the_list_first() {
        let mut rows = vec![chat_row(101, 1, false), chat_row(102, 2, false)];
        rows[0].title = "Anna".into();
        rows[1].title = "Boris".into();
        let found = |chat: i64, title: &str, saved: bool| FoundChat {
            chat,
            title: title.into(),
            avatar: flux_plugin_api::conversation::AvatarSpec::initials("A", 0),
            detail: None,
            added: false,
            saved,
        };
        let search = [
            found(103, "Clara", false),
            found(101, "Anna", false),
            found(7, "Saved Messages", true),
        ];
        // Each item's key, as the picker gives it back.
        let keys = |items: Vec<picker::Item>| -> Vec<String> {
            items
                .iter()
                .map(|item| {
                    let debug = format!("{item:?}");
                    let key = debug.split("key: \"").nth(1).unwrap_or_default();
                    key.split('"').next().unwrap_or_default().to_string()
                })
                .collect()
        };
        assert_eq!(
            keys(forward_items("a1", &rows, &search, "")),
            ["a1:7", "a1:101", "a1:102", "a1:103"]
        );
        // The list's chats that have the query; the search's are its own answer.
        assert_eq!(
            keys(forward_items("a1", &rows, &search, "an")),
            ["a1:7", "a1:101", "a1:103"]
        );
    }

    fn chat_row(chat: i64, date: i64, pinned: bool) -> ChatRow {
        ChatRow {
            chat,
            title: format!("chat {chat}"),
            avatar: flux_plugin_api::conversation::AvatarSpec::initials("C", 0),
            last: None,
            date,
            delivery: None,
            unread: 0,
            mentions: 0,
            muted: false,
            pinned,
            draft: None,
            bot: false,
            blocked: false,
            claude: false,
        }
    }

    #[test]
    fn merges_the_accounts_lists() {
        // Each account's rows as the service sends them: pinned first, in Telegram's order.
        let first = [
            chat_row(1, 100, true),
            chat_row(2, 300, true),
            chat_row(3, 500, false),
            chat_row(4, 0, false),
        ];
        let second = [
            chat_row(7, 50, true),
            chat_row(8, 900, false),
            chat_row(9, 500, false),
        ];
        let rows = merge(
            first
                .iter()
                .map(|row| ("a1", row))
                .chain(second.iter().map(|row| ("a2", row))),
        );
        let order: Vec<(&str, i64)> = rows
            .iter()
            .map(|(account, row)| (*account, row.chat))
            .collect();
        // The pinned in their order (the first account's, then the second's); then the newest
        // (the same moment: the accounts' order); a chat without messages last.
        assert_eq!(
            order,
            [
                ("a1", 1),
                ("a1", 2),
                ("a2", 7),
                ("a2", 8),
                ("a1", 3),
                ("a2", 9),
                ("a1", 4)
            ]
        );
    }

    #[test]
    fn folders_count_and_hold_chats_of_an_account() {
        let folder = Folder {
            id: 1,
            name: "Work".into(),
            chats: vec![ChatKey::new("a1", 3), ChatKey::new("a2", 3)],
        };
        assert!(in_folder(&folder, "a1", 3));
        assert!(!in_folder(&folder, "a1", 4));
        assert!(!in_folder(&folder, "a3", 3));
        let mut muted = chat_row(5, 0, false);
        muted.unread = 10;
        muted.muted = true;
        let mut loud = chat_row(6, 0, false);
        loud.unread = 3;
        assert_eq!(unread([&muted, &loud]), 3);
    }

    #[test]
    fn searches_titles_in_any_case() {
        assert!(matches("Backend · team", "TEAM"));
        assert!(matches("Анна Смирнова", "анна"));
        assert!(matches("Anna", " ann "));
        assert!(!matches("Anna", "dima"));
        assert_eq!(count_text(7), "7");
        assert_eq!(count_text(120), "99+");
    }
}
