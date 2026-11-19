//! The service's list (part 3): the chats the user added to FluxGram, per account, and what goes
//! with them — the rows of the window on the left ([`Note::Chats`]), the Add Chat popup's search
//! ([`Note::Found`]), the notifications of new messages.
//!
//! - The lists live in the plugin's storage (`chats/<account>`: chat ids, in the order added);
//!   [`Chats::listed`] gives the helper's filter its chats.
//! - An account's client is ready → the list's chats are asked for (`getChat`: the store takes the
//!   answers), their small photos downloaded; the rows follow the store's changes ([`Touched`]) and
//!   go to every window at the end of a batch, only when they changed.
//! - The Add Chat popup: the recent chats (`getChats` of the main list), then the query's
//!   (`searchChats`, `searchChatsOnServer`) — people, bots, groups and channels (part 5; no
//!   secret chats — [`Store::listed`]).
//! - A notification per chat with new messages (not muted, not in sight in any window): the card
//!   is updated while more come and removed once the chat is read; its Open runs the command
//!   `open-chat` with the chat's tab key.
//!
//! - FluxGram's own folders ([`Folder`], [`FolderAsk`]; not Telegram's), across the accounts: the
//!   user's data, kept here for every window (`folders` in the storage) and sent whole
//!   ([`Note::Folders`]) when they changed. Plaintext: a chat removed from the list, or hidden,
//!   leaves every folder — a hidden chat is never in one.
//! - The hidden chats ([`HiddenAsk`], [`crate::hidden`]): a list per account sealed with a key
//!   derived from the user's phrase (`hidden-chats` in the storage). While no window's search has
//!   the phrase, the service knows nothing of them: no rows, no notifications, no counts, and the
//!   helper's filter leaves them out. A window whose search has it gets their rows
//!   ([`HiddenTold::Unlocked`], [`HiddenTold::Rows`]); while some window has them, the service
//!   holds the key and the list, and lets their updates through — still without notifications.
//!
//! What a row or a found chat says is made by pure functions ([`row`], [`found`], [`sort`],
//! [`notice_line`]) over the [`Store`], tested natively.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use flux_plugin_api::conversation::{AvatarSpec, Delivery};
use flux_plugin_api::host::notifications;
use flux_plugin_api::{log, notify, service, storage};
use serde_json::{Value, json};

use crate::hidden::{self, HiddenList, Sealed};
use crate::model::{ChatKey, ChatRow, FoundChat, Note, Request, Words, initials};
use crate::service::{Asked, Cx, HostWords};
use crate::store::{ChatKind, Store, Touched, UserKind};
use crate::summary;

/// What a window asks of the hidden chats: `Request::Hidden`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "hidden", rename_all = "kebab-case")]
pub enum HiddenAsk {
    /// The list's search text, once typing stopped (long enough to be a phrase): the hidden
    /// chats for this window when it is the phrase ([`HiddenTold::Unlocked`]); otherwise they
    /// stay hidden here — nothing is told.
    Try { text: String },
    /// The phrase left this window's search: the chats are hidden here again.
    Lock,
    /// Hide a chat: with this window's open list (`phrase` none), or with the phrase a dialog
    /// asked for.
    Hide {
        key: ChatKey,
        phrase: Option<String>,
    },
    /// Show a hidden chat in the list again (this window has the hidden chats open).
    Unhide { key: ChatKey },
    /// Settings: the phrase; `current` — the one set before (none for the first).
    SetPhrase {
        current: Option<String>,
        phrase: String,
    },
    /// Settings: Forget Hidden Chats, confirmed — the list goes, the chats stay in Telegram.
    Forget,
}

/// What the service tells the windows of the hidden chats: `Note::Hidden`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "hidden", rename_all = "kebab-case")]
pub enum HiddenTold {
    /// Whether a phrase is set: to every window, at its start and when it changes.
    State { set: bool },
    /// `text` is the phrase: the hidden chats are open in this window (their rows follow).
    Unlocked { text: String },
    /// The hidden chats' rows of the windows that have them open, when they changed.
    Rows { rows: Vec<HiddenRow> },
    /// The chats are hidden again in this window (the phrase changed, the list was forgotten).
    Locked,
    /// A chat was hidden: a window without the hidden chats open closes its tab. To every
    /// window.
    Hid { key: ChatKey },
    /// A hidden chat is in the list again. To every window.
    Unhid { key: ChatKey },
    /// Settings: the phrase was saved.
    Saved,
    /// The phrase given isn't the one set: Settings' current one (`hide` false), or a Hide
    /// dialog's.
    WrongPhrase { hide: bool },
}

/// A hidden chat's row: its account and the row.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HiddenRow {
    pub account: String,
    pub row: ChatRow,
}

/// The storage's key of the sealed hidden list.
const HIDDEN: &str = "hidden-chats";
/// The storage's key of the folders.
const FOLDERS: &str = "folders";

/// A folder of FluxGram's own (not Telegram's): chats of any account, by the user's choice.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Folder {
    /// Made by the window that created it (it selects the folder at once).
    pub id: u64,
    pub name: String,
    /// In the order added.
    pub chats: Vec<ChatKey>,
}

/// What a window asks of the folders: `Request::Folders`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "folders", rename_all = "kebab-case")]
pub enum FolderAsk {
    /// A new folder (+, or New Folder… for a chat: `chats` has it).
    Create {
        id: u64,
        name: String,
        chats: Vec<ChatKey>,
    },
    Rename {
        id: u64,
        name: String,
    },
    /// Delete Folder, confirmed: the chats stay in the list.
    Delete {
        id: u64,
    },
    /// Put a chat of the list in the folder.
    Add {
        id: u64,
        key: ChatKey,
    },
    /// Take a chat out of the folder.
    Remove {
        id: u64,
        key: ChatKey,
    },
}

/// A folder's name as kept: one line, trimmed, at most this many characters.
const FOLDER_NAME_MAX: usize = 40;

/// TDLib's updates the list needs beyond the store's (`model::KEPT`): new messages notify, and a
/// chat marked as unread shows a count.
pub const KEPT: &[&str] = &["updateNewMessage", "updateChatIsMarkedAsUnread"];

/// How many chats the Add Chat popup asks TDLib for: the recent ones, each search.
const FOUND_LIMIT: u32 = 50;
/// A card shown longer ago than this has left the corner (Flux keeps one with actions 14 s): the
/// next message replaces it, so it pops up again, rather than changing it out of sight.
const CARD_FRESH: Duration = Duration::from_secs(10);

/// What a request of the list asked: its answer comes to [`Chats::answer`].
#[derive(Debug)]
pub enum Ask {
    /// `getChat` of a listed chat: the store takes it, the rows follow.
    Chat(i64),
    /// `getChat` of a hidden chat, while the hidden chats are open: their rows follow.
    Hidden(i64),
    /// A popup's search: `getChats`, `searchChats` or `searchChatsOnServer`.
    Search {
        window: u64,
        request: u64,
        step: Step,
    },
    /// `getChat` of a chat a popup found that the store lacked.
    Found { window: u64, request: u64 },
    /// What only the store needs (a group, a photo's download): the rows and the popups look
    /// again.
    Store,
}

/// Which of a popup's searches answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The main list's first chats: the popup with an empty query.
    Recent,
    /// TDLib's search among the chats it knows.
    Local,
    /// Telegram's search among the user's chats and contacts.
    Server,
}

/// An account's list.
#[derive(Debug, Default)]
struct List {
    /// The chats added, in the order added.
    chats: Vec<i64>,
    /// The account's client is ready: the chats are asked for, and new messages notify.
    ready: bool,
    /// Listed chats TDLib doesn't have (`getChat` failed): left out of the rows.
    missing: HashSet<i64>,
    /// The rows last sent to the windows; none — not sent yet.
    sent: Option<Vec<ChatRow>>,
    /// Something the rows show may have changed.
    dirty: bool,
}

/// A window's Add Chat popup: its latest request and what it found.
#[derive(Debug)]
struct Search {
    account: String,
    request: u64,
    /// The chats the store had at the request (the listed ones for an empty query): the first
    /// answer, before TDLib's.
    known: Vec<i64>,
    /// TDLib's answers: the recent chats or its local search, then Telegram's.
    first: Vec<i64>,
    server: Vec<i64>,
    /// TDLib's answers still to come: an empty popup isn't sent while it looks.
    pending: u8,
    sent: Option<Vec<FoundChat>>,
    dirty: bool,
}

/// A notification of a chat's new messages.
#[derive(Debug, Default)]
struct Card {
    /// The notification's id once sent, and when it was sent.
    shown: Option<(u64, Instant)>,
    /// The messages it tells of, and the last one in a line.
    count: u32,
    last: String,
    dirty: bool,
}

/// The hidden chats while some window has them open: the key (to seal the list again), the list,
/// the windows, the rows last sent.
struct Unlocked {
    key: hidden::Key,
    list: HiddenList,
    windows: HashSet<u64>,
    sent: Option<Vec<HiddenRow>>,
    dirty: bool,
}

pub struct Chats {
    lists: HashMap<String, List>,
    /// A phrase is set: the sealed list is in the storage.
    phrase_set: bool,
    /// The hidden chats, while some window has them open; none — the service knows nothing of
    /// them.
    unlocked: Option<Unlocked>,
    /// Each window's popup: a search per account.
    searches: HashMap<(u64, String), Search>,
    /// FluxGram's folders; changed since they were last sent.
    folders: Vec<Folder>,
    folders_changed: bool,
    cards: HashMap<ChatKey, Card>,
    /// The chats in sight, by window: they don't notify.
    in_sight: HashMap<u64, HashSet<ChatKey>>,
    /// Photos asked for (account, file): not asked twice while the helper runs.
    downloading: HashSet<(String, i64)>,
}

impl Chats {
    pub fn new() -> Self {
        Chats {
            lists: HashMap::new(),
            phrase_set: false,
            unlocked: None,
            searches: HashMap::new(),
            folders: Vec::new(),
            folders_changed: false,
            cards: HashMap::new(),
            in_sight: HashMap::new(),
            downloading: HashSet::new(),
        }
    }

    /// The service starts: the accounts' lists and the folders from the storage.
    pub fn activate(&mut self, accounts: &[String]) {
        self.phrase_set = storage::get::<Sealed>(HIDDEN).is_some();
        for account in accounts {
            let chats: Vec<i64> = storage::get(&storage_key(account)).unwrap_or_default();
            self.lists.insert(
                account.clone(),
                List {
                    chats,
                    dirty: true,
                    ..List::default()
                },
            );
        }
        self.folders = storage::get(FOLDERS).unwrap_or_default();
        // Only chats of the lists: whatever left them while the folders weren't looked at goes.
        let lists = &self.lists;
        let listed = |key: &ChatKey| {
            lists
                .get(&key.account)
                .is_some_and(|list| list.chats.contains(&key.chat))
        };
        if keep_listed(&mut self.folders, listed) {
            storage::set(FOLDERS, &self.folders);
        }
    }

    /// The chats in the account's list, and its hidden ones while some window has them open:
    /// the helper passes their updates.
    ///
    /// Claude (part 6) must never see the hidden chats: what it is given of the chats comes from
    /// the account's list (`List::chats`), never from this function or [`Unlocked`].
    pub fn listed(&self, account: &str) -> Vec<i64> {
        let mut chats = self
            .lists
            .get(account)
            .map(|list| list.chats.clone())
            .unwrap_or_default();
        if let Some(unlocked) = &self.unlocked {
            chats.extend(unlocked.list.chats(account));
        }
        chats
    }

    /// The account's client is ready (signed in now, or the helper started again): load the list.
    pub fn account_ready(&mut self, cx: &mut Cx, account: &str) {
        self.downloading.retain(|(key, _)| key != account);
        let list = self.lists.entry(account.to_string()).or_default();
        list.ready = true;
        list.missing.clear();
        list.dirty = true;
        for &chat in &list.chats {
            cx.td.send(
                account,
                json!({"@type": "getChat", "chat_id": chat}),
                Asked::Chats(Ask::Chat(chat)),
            );
        }
        if let Some(unlocked) = &mut self.unlocked {
            unlocked.dirty = true;
            for &chat in unlocked.list.chats(account) {
                cx.td.send(
                    account,
                    json!({"@type": "getChat", "chat_id": chat}),
                    Asked::Chats(Ask::Hidden(chat)),
                );
            }
        }
    }

    /// The account is gone (signed out): its list goes, from the storage too, and its chats leave
    /// the folders.
    pub fn account_gone(&mut self, account: &str) {
        self.lists.remove(account);
        storage::remove(&storage_key(account));
        if keep_listed(&mut self.folders, |key| key.account != account) {
            self.save_folders();
        }
        // Its hidden chats go with it when the list is open (otherwise at its next opening).
        if let Some(unlocked) = &mut self.unlocked
            && unlocked.list.forget_account(account)
        {
            unlocked.dirty = true;
            storage::set(HIDDEN, &unlocked.key.seal(&unlocked.list, hidden::random()));
        }
        self.searches.retain(|_, search| search.account != account);
        self.downloading.retain(|(key, _)| key != account);
        let gone: Vec<ChatKey> = self
            .cards
            .keys()
            .filter(|key| key.account == account)
            .cloned()
            .collect();
        for key in gone {
            self.drop_card(&key);
        }
    }

    /// An update of the account's client, after the store took it (`touched` — what it changed).
    pub fn update(&mut self, cx: &mut Cx, account: &str, update: &Value, touched: Touched) {
        match update["@type"].as_str().unwrap_or_default() {
            "updateNewMessage" => self.new_message(cx, account, &update["message"]),
            // Read here or on another device: the chat's card goes.
            "updateChatReadInbox" if update["unread_count"].as_u64() == Some(0) => {
                let key = ChatKey::new(account, update["chat_id"].as_i64().unwrap_or_default());
                self.drop_card(&key);
            }
            _ => {}
        }
        self.touched(account, touched);
    }

    /// The answer to a request of the list.
    pub fn answer(&mut self, cx: &mut Cx, account: &str, ask: Ask, answer: &Value) {
        let failed = answer["@type"] == "error";
        match ask {
            Ask::Chat(chat) => {
                let Some(list) = self.lists.get_mut(account) else {
                    return;
                };
                list.dirty = true;
                if failed {
                    log::warn(&format!(
                        "getChat {chat} of {account}: {}",
                        answer["message"].as_str().unwrap_or_default()
                    ));
                    list.missing.insert(chat);
                } else {
                    list.missing.remove(&chat);
                    self.download_photo(cx, account, chat);
                }
            }
            Ask::Hidden(chat) => {
                if let Some(unlocked) = &mut self.unlocked {
                    unlocked.dirty = true;
                }
                if !failed {
                    self.download_photo(cx, account, chat);
                }
            }
            Ask::Search {
                window,
                request,
                step,
            } => {
                let Some(search) = self
                    .searches
                    .get_mut(&(window, account.to_string()))
                    .filter(|search| search.request == request)
                else {
                    return;
                };
                if failed {
                    log::warn(&format!(
                        "A search of {account}: {}",
                        answer["message"].as_str().unwrap_or_default()
                    ));
                }
                let ids: Vec<i64> = answer["chat_ids"]
                    .as_array()
                    .map(|ids| ids.iter().filter_map(Value::as_i64).collect())
                    .unwrap_or_default();
                match step {
                    Step::Recent | Step::Local => search.first = ids.clone(),
                    Step::Server => search.server = ids.clone(),
                }
                search.pending = search.pending.saturating_sub(1);
                search.dirty = true;
                for chat in ids {
                    let known = cx
                        .stores
                        .get(account)
                        .is_some_and(|store| store.chats.contains_key(&chat));
                    if known {
                        self.complete(cx, account, chat);
                    } else {
                        cx.td.send(
                            account,
                            json!({"@type": "getChat", "chat_id": chat}),
                            Asked::Chats(Ask::Found { window, request }),
                        );
                    }
                }
            }
            Ask::Found { window, request } => {
                if let Some(search) = self
                    .searches
                    .get_mut(&(window, account.to_string()))
                    .filter(|search| search.request == request)
                {
                    search.dirty = true;
                }
                if let Some(chat) = answer["id"].as_i64().filter(|_| !failed) {
                    self.complete(cx, account, chat);
                }
            }
            Ask::Store => self.touched(account, Touched::Scope),
        }
    }

    /// A window's request: `AddChat`, `RemoveChat`, `SearchChats`, `InSight`.
    pub fn request(&mut self, cx: &mut Cx, window: u64, request: Request) {
        match request {
            Request::AddChat { key } => {
                let list = self.lists.entry(key.account.clone()).or_default();
                if !list.chats.contains(&key.chat) {
                    list.chats.push(key.chat);
                    storage::set(&storage_key(&key.account), &list.chats);
                }
                list.dirty = true;
                // A hidden chat added where its window has the hidden chats closed (part 5): it
                // leaves the open hidden list (a closed one drops it when next opened).
                if let Some(unlocked) = &mut self.unlocked
                    && unlocked.list.unhide(&key.account, key.chat)
                {
                    unlocked.dirty = true;
                    storage::set(HIDDEN, &unlocked.key.seal(&unlocked.list, hidden::random()));
                }
                // The store's copy may be old (the popup's): TDLib's current one, and its updates
                // from now on (the filter follows at the end of the batch).
                cx.td.send(
                    &key.account,
                    json!({"@type": "getChat", "chat_id": key.chat}),
                    Asked::Chats(Ask::Chat(key.chat)),
                );
                self.dirty_searches(&key.account);
            }
            Request::RemoveChat { key } => {
                if let Some(list) = self.lists.get_mut(&key.account) {
                    list.chats.retain(|chat| *chat != key.chat);
                    list.missing.remove(&key.chat);
                    storage::set(&storage_key(&key.account), &list.chats);
                    list.dirty = true;
                }
                // A hidden chat removed (its tab's ⋯): it leaves the hidden list too.
                if let Some(unlocked) = &mut self.unlocked
                    && unlocked.list.unhide(&key.account, key.chat)
                {
                    unlocked.dirty = true;
                    storage::set(HIDDEN, &unlocked.key.seal(&unlocked.list, hidden::random()));
                }
                self.leave_folders(&key);
                self.drop_card(&key);
                self.dirty_searches(&key.account);
            }
            Request::SearchChats {
                account,
                query,
                request,
            } => self.search(cx, window, account, query.trim(), request),
            Request::InSight { key, in_sight } => {
                let keys = self.in_sight.entry(window).or_default();
                if in_sight {
                    keys.insert(key.clone());
                    self.drop_card(&key);
                } else {
                    keys.remove(&key);
                }
            }
            _ => {}
        }
    }

    /// A timer of the list fired (one it started); false — not the list's. The list has none.
    pub fn timer(&mut self, _cx: &mut Cx, _timer: u64) -> bool {
        false
    }

    /// A window's instance started: it gets every account's list ([`Note::Chats`]), the folders
    /// and whether a phrase is set.
    pub fn window_opened(&mut self, _cx: &mut Cx, window: u64) {
        let _ = service::send_json_to(
            window,
            &Note::Hidden(HiddenTold::State {
                set: self.phrase_set,
            }),
        );
        let _ = service::send_json_to(
            window,
            &Note::Folders {
                folders: self.folders.clone(),
            },
        );
        for (account, list) in &self.lists {
            if let Some(rows) = &list.sent {
                let _ = service::send_json_to(
                    window,
                    &Note::Chats {
                        account: account.clone(),
                        rows: rows.clone(),
                    },
                );
            }
        }
    }

    /// A window's instance stopped: its chats in sight aren't any more, its popup is gone.
    pub fn window_closed(&mut self, _cx: &mut Cx, window: u64) {
        self.in_sight.remove(&window);
        self.searches
            .retain(|(searching, _), _| *searching != window);
        self.lock_window(window);
    }

    /// The end of a batch: the lists that changed go to the windows, the notifications follow.
    pub fn flush(&mut self, cx: &mut Cx) {
        let words = HostWords;
        for (account, list) in &mut self.lists {
            if !list.dirty {
                continue;
            }
            list.dirty = false;
            let Some(store) = cx.stores.get(account.as_str()) else {
                continue;
            };
            let Some(rows) = rows(store, list, &words) else {
                continue;
            };
            if list.sent.as_ref() == Some(&rows) {
                continue;
            }
            let _ = service::broadcast_json(&Note::Chats {
                account: account.clone(),
                rows: rows.clone(),
            });
            list.sent = Some(rows);
        }

        if std::mem::take(&mut self.folders_changed) {
            let _ = service::broadcast_json(&Note::Folders {
                folders: self.folders.clone(),
            });
        }

        for ((window, _), search) in &mut self.searches {
            if !search.dirty {
                continue;
            }
            search.dirty = false;
            let Some(store) = cx.stores.get(&search.account) else {
                continue;
            };
            let added = self
                .lists
                .get(&search.account)
                .map(|list| list.chats.as_slice())
                .unwrap_or_default();
            let chats = found_chats(store, search, added, &words);
            if search.sent.as_ref() == Some(&chats) || (chats.is_empty() && search.pending > 0) {
                continue;
            }
            let _ = service::send_json_to(
                *window,
                &Note::Found {
                    request: search.request,
                    chats: chats.clone(),
                },
            );
            search.sent = Some(chats);
        }

        if let Some(unlocked) = self.unlocked.as_mut().filter(|unlocked| unlocked.dirty) {
            unlocked.dirty = false;
            let rows = hidden_rows(cx.stores, &unlocked.list, &words);
            if unlocked.sent.as_ref() != Some(&rows) {
                for window in &unlocked.windows {
                    let _ = service::send_json_to(
                        *window,
                        &Note::Hidden(HiddenTold::Rows { rows: rows.clone() }),
                    );
                }
                unlocked.sent = Some(rows);
            }
        }

        for (key, card) in &mut self.cards {
            if !card.dirty {
                continue;
            }
            card.dirty = false;
            let Some(store) = cx.stores.get(&key.account) else {
                continue;
            };
            let body = if card.count > 1 {
                format!(
                    "{}\n{}",
                    words.count(card.count.into(), "{0} new message", "{0} new messages"),
                    card.last
                )
            } else {
                card.last.clone()
            };
            let notice = notify::Notice::info(&title(store, key.chat, &words))
                .body(&body)
                .action_with(&words.tr("Open"), "open-chat", &key.tab());
            card.shown = Some(match card.shown {
                Some((id, at)) if at.elapsed() < CARD_FRESH => {
                    notice.update(id);
                    (id, at)
                }
                shown => {
                    if let Some((id, _)) = shown {
                        notifications::remove(id);
                    }
                    (notice.send(), Instant::now())
                }
            });
        }
    }

    // --- Inside ---

    /// What the store changed: the account's rows and popups look again.
    fn touched(&mut self, account: &str, touched: Touched) {
        if let Some(unlocked) = &mut self.unlocked {
            match touched {
                Touched::Nothing => {}
                Touched::Chat(chat) if !unlocked.list.contains(account, chat) => {}
                _ => unlocked.dirty = true,
            }
        }
        let Some(list) = self.lists.get_mut(account) else {
            return;
        };
        match touched {
            Touched::Nothing => {}
            Touched::Chat(chat) => {
                if list.chats.contains(&chat) {
                    list.dirty = true;
                }
                for search in self.searches.values_mut() {
                    if search.account == account
                        && (search.first.contains(&chat)
                            || search.server.contains(&chat)
                            || search.known.contains(&chat))
                    {
                        search.dirty = true;
                    }
                }
            }
            // A name (of a sender too), a photo, the default muting: anything may show it.
            Touched::User(_) | Touched::File(_) | Touched::Scope => {
                list.dirty = true;
                self.dirty_searches(account);
            }
        }
    }

    fn dirty_searches(&mut self, account: &str) {
        for search in self.searches.values_mut() {
            if search.account == account {
                search.dirty = true;
            }
        }
    }

    /// A popup's query of one account (the popup asks each): what the store has at once, then
    /// TDLib's answers.
    fn search(&mut self, cx: &mut Cx, window: u64, account: String, query: &str, request: u64) {
        let Some(store) = cx.stores.get(&account) else {
            // The account's client isn't ready: nothing found there, said at once — the popup
            // waits for every account's answer.
            self.searches.remove(&(window, account));
            let _ = service::send_json_to(
                window,
                &Note::Found {
                    request,
                    chats: Vec::new(),
                },
            );
            return;
        };
        let known = if query.is_empty() {
            let mut listed: Vec<i64> = self
                .lists
                .get(&account)
                .map(|list| list.chats.clone())
                .unwrap_or_default();
            sort(store, &mut listed);
            listed
        } else {
            let mut matching: Vec<i64> = store
                .chats
                .keys()
                .copied()
                .filter(|&chat| matches(store, chat, query, &HostWords))
                .collect();
            sort(store, &mut matching);
            matching
        };
        self.searches.insert(
            (window, account.clone()),
            Search {
                account: account.clone(),
                request,
                known,
                first: Vec::new(),
                server: Vec::new(),
                pending: if query.is_empty() { 1 } else { 2 },
                sent: None,
                dirty: true,
            },
        );
        let ask = |step| {
            Asked::Chats(Ask::Search {
                window,
                request,
                step,
            })
        };
        if query.is_empty() {
            cx.td.send(
                &account,
                json!({"@type": "getChats", "chat_list": {"@type": "chatListMain"},
                    "limit": FOUND_LIMIT}),
                ask(Step::Recent),
            );
        } else {
            cx.td.send(
                &account,
                json!({"@type": "searchChats", "query": query, "limit": FOUND_LIMIT}),
                ask(Step::Local),
            );
            cx.td.send(
                &account,
                json!({"@type": "searchChatsOnServer", "query": query, "limit": FOUND_LIMIT}),
                ask(Step::Server),
            );
        }
    }

    /// A found chat the store has: its group's members (for "5 members") and its photo.
    fn complete(&mut self, cx: &mut Cx, account: &str, chat: i64) {
        let Some(store) = cx.stores.get(account) else {
            return;
        };
        let group = match store.chats.get(&chat).map(|found| found.kind) {
            Some(ChatKind::BasicGroup { group }) if store.group(chat).is_none() => {
                Some(json!({"@type": "getBasicGroup", "basic_group_id": group}))
            }
            Some(ChatKind::Supergroup { group, .. }) if store.group(chat).is_none() => {
                Some(json!({"@type": "getSupergroup", "supergroup_id": group}))
            }
            _ => None,
        };
        if let Some(request) = group {
            cx.td.send(account, request, Asked::Chats(Ask::Store));
        }
        self.download_photo(cx, account, chat);
    }

    /// The chat's small photo, when it isn't downloaded yet.
    fn download_photo(&mut self, cx: &mut Cx, account: &str, chat: i64) {
        let Some(file) = cx
            .stores
            .get(account)
            .and_then(|store| store.chat_photo_missing(chat))
        else {
            return;
        };
        if !self.downloading.insert((account.to_string(), file)) {
            return;
        }
        cx.td.send(
            account,
            json!({"@type": "downloadFile", "file_id": file, "priority": 1, "offset": 0,
                "limit": 0, "synchronous": false}),
            Asked::Chats(Ask::Store),
        );
    }

    /// A new message: a card for the chat, unless it is the user's own, muted, in sight or read.
    fn new_message(&mut self, cx: &mut Cx, account: &str, message: &Value) {
        let chat = message["chat_id"].as_i64().unwrap_or_default();
        let listed = self
            .lists
            .get(account)
            .is_some_and(|list| list.ready && list.chats.contains(&chat));
        if !listed || message["is_outgoing"] == true {
            return;
        }
        let Some(store) = cx.stores.get(account) else {
            return;
        };
        let Some(found) = store.chats.get(&chat) else {
            return;
        };
        let read = message["id"].as_i64().unwrap_or_default() <= found.last_read_inbox;
        let key = ChatKey::new(account, chat);
        let in_sight = self.in_sight.values().any(|keys| keys.contains(&key));
        if read || in_sight || store.muted(chat) {
            return;
        }
        let card = self.cards.entry(key).or_default();
        card.count += 1;
        card.last = notice_line(store, chat, message, &HostWords);
        card.dirty = true;
    }

    /// The chat's card goes: read, in sight, removed from the list.
    fn drop_card(&mut self, key: &ChatKey) {
        if let Some(Card {
            shown: Some((id, _)),
            ..
        }) = self.cards.remove(key)
        {
            notifications::remove(id);
        }
    }
}

fn storage_key(account: &str) -> String {
    format!("chats/{account}")
}

// --- What the rows and the popup say (no host: tested natively) ---

/// The account's rows in order; none while a listed chat is neither in the store nor known to be
/// missing and nothing was sent yet (the window shows no list rather than a part of it). A chat
/// the store lacks for now (the helper started again) keeps its row as last sent.
fn rows(store: &Store, list: &List, words: &dyn Words) -> Option<Vec<ChatRow>> {
    let mut known: Vec<i64> = Vec::new();
    let mut kept: Vec<ChatRow> = Vec::new();
    for &chat in &list.chats {
        if store.chats.contains_key(&chat) {
            if store.listed(chat) {
                known.push(chat);
            }
        } else if let Some(row) = list
            .sent
            .as_ref()
            .and_then(|sent| sent.iter().find(|row| row.chat == chat))
        {
            kept.push(row.clone());
        } else if !list.missing.contains(&chat) && list.sent.is_none() {
            return None;
        }
    }
    let mut rows: Vec<(bool, i64, ChatRow)> = known
        .iter()
        .filter_map(|&chat| {
            let (pinned, order) = order_of(store, chat);
            Some((pinned, order, row(store, chat, words)?))
        })
        .collect();
    rows.extend(
        kept.into_iter()
            .map(|row| (row.pinned, row.date << 32, row)),
    );
    rows.sort_by_key(|(pinned, order, row)| {
        (
            std::cmp::Reverse(*pinned),
            std::cmp::Reverse(*order),
            row.chat,
        )
    });
    Some(rows.into_iter().map(|(_, _, row)| row).collect())
}

/// Telegram's order of a chat: pinned first, then its place in the main list (the archive's, or
/// the last message's date, for a chat in neither).
fn order_of(store: &Store, chat: i64) -> (bool, i64) {
    let Some(found) = store.chats.get(&chat) else {
        return (false, 0);
    };
    let order = if found.order != 0 {
        found.order
    } else if found.archive_order != 0 {
        found.archive_order
    } else {
        found
            .last_message
            .as_ref()
            .and_then(|message| message["date"].as_i64())
            .unwrap_or_default()
            << 32
    };
    (found.pinned && found.order != 0, order)
}

/// Chats in Telegram's order.
pub fn sort(store: &Store, chats: &mut [i64]) {
    chats.sort_by_key(|&chat| {
        let (pinned, order) = order_of(store, chat);
        (std::cmp::Reverse(pinned), std::cmp::Reverse(order), chat)
    });
}

/// The chat's name as FluxGram shows it: "Saved Messages" for the user's own chat, "Deleted
/// Account" for one without a name.
pub fn title(store: &Store, chat: i64, words: &dyn Words) -> String {
    if store.is_saved_messages(chat) {
        return words.tr("Saved Messages");
    }
    let found = store.chats.get(&chat);
    match found.map(|found| found.title.trim()) {
        Some(title) if !title.is_empty() => title.to_string(),
        _ => match found.map(|found| found.kind) {
            Some(ChatKind::Private { user }) => {
                let name = store.user_name(user);
                if name.is_empty() {
                    words.tr("Deleted Account")
                } else {
                    name
                }
            }
            _ => String::new(),
        },
    }
}

/// The chat's picture; Saved Messages by its own initial.
fn avatar(store: &Store, chat: i64, title: &str) -> AvatarSpec {
    let mut avatar = store.chat_avatar(chat);
    if store.is_saved_messages(chat) || avatar.initials.is_empty() {
        avatar.initials = initials(title);
    }
    avatar
}

/// A listed chat's row.
pub fn row(store: &Store, chat: i64, words: &dyn Words) -> Option<ChatRow> {
    let found = store.chats.get(&chat)?;
    let title = title(store, chat, words);
    let message = found.last_message.as_ref();
    let delivery = message
        .filter(|message| message["is_outgoing"] == true && !store.is_saved_messages(chat))
        .map(|message| match message["sending_state"]["@type"].as_str() {
            Some("messageSendingStatePending") => Delivery::Sending,
            Some("messageSendingStateFailed") => Delivery::Failed,
            _ if message["id"].as_i64().unwrap_or_default() <= found.last_read_outbox => {
                Delivery::Read
            }
            _ => Delivery::Sent,
        });
    Some(ChatRow {
        chat,
        avatar: avatar(store, chat, &title),
        title,
        last: message.map(|message| last_line(store, chat, message, words)),
        date: message
            .and_then(|message| message["date"].as_i64())
            .unwrap_or_default(),
        delivery,
        unread: if found.unread_count == 0 && found.marked_unread {
            1
        } else {
            found.unread_count
        },
        mentions: found.unread_mentions,
        muted: store.muted(chat),
        pinned: found.pinned && found.order != 0,
        draft: found
            .draft
            .as_deref()
            .map(summary::first_line)
            .filter(|draft| !draft.is_empty()),
        bot: store.is_bot(chat),
        blocked: found.blocked,
    })
}

/// The list's last line: "Petya: Release on Friday?" in a group, "You: See you at 11?" for the
/// user's own, the text alone in a person's chat; a line of the chat itself ("Anna joined the
/// group") as it is.
pub fn last_line(store: &Store, chat: i64, message: &Value, words: &dyn Words) -> String {
    let line = summary::line(store, message, words);
    if !said(message) || store.is_saved_messages(chat) {
        return line;
    }
    if message["is_outgoing"] == true {
        return words.trf("You: {0}", &[&line]);
    }
    with_sender(store, chat, message, line, words)
}

/// A notification's line of a new message: "Petya: Release on Friday?" in a group, the text in a
/// person's chat.
pub fn notice_line(store: &Store, chat: i64, message: &Value, words: &dyn Words) -> String {
    let line = summary::line(store, message, words);
    if !said(message) {
        return line;
    }
    with_sender(store, chat, message, line, words)
}

/// In a group, the sender's first name before the line (not for the group's own posts: an
/// anonymous admin).
fn with_sender(
    store: &Store,
    chat: i64,
    message: &Value,
    line: String,
    words: &dyn Words,
) -> String {
    let group = matches!(
        store.chats.get(&chat).map(|found| found.kind),
        Some(ChatKind::BasicGroup { .. } | ChatKind::Supergroup { .. })
    );
    if !group {
        return line;
    }
    let sender = summary::sender(store, message, words);
    if !sender.user && sender.id == chat {
        return line;
    }
    format!("{}: {line}", sender.first_name())
}

/// The message is something someone said (a text, a photo…), not a line of the chat itself.
fn said(message: &Value) -> bool {
    matches!(
        message["content"]["@type"].as_str().unwrap_or_default(),
        "messageText"
            | "messagePhoto"
            | "messageVideo"
            | "messageAnimation"
            | "messageVoiceNote"
            | "messageVideoNote"
            | "messageAudio"
            | "messageDocument"
            | "messageSticker"
            | "messageAnimatedEmoji"
            | "messageDice"
            | "messageLocation"
            | "messageVenue"
            | "messageContact"
            | "messagePoll"
            | "messageGame"
            | "messageInvoice"
            | "messageStory"
            | "messagePaidMedia"
    )
}

/// The chat's title or a person's @username has the query (any case).
fn matches(store: &Store, chat: i64, query: &str, words: &dyn Words) -> bool {
    if !store.listed(chat) {
        return false;
    }
    let query = query.trim_start_matches('@').to_lowercase();
    if title(store, chat, words).to_lowercase().contains(&query) {
        return true;
    }
    let username = match store.chats.get(&chat).map(|found| found.kind) {
        Some(ChatKind::Private { user }) => store.users.get(&user).and_then(|u| u.username.clone()),
        _ => store.group(chat).and_then(|group| group.username.clone()),
    };
    username.is_some_and(|name| name.to_lowercase().contains(&query))
}

/// The popup's chats: TDLib's answers in their order, then what the store had (no secret
/// chats) — each once.
fn found_chats(store: &Store, search: &Search, added: &[i64], words: &dyn Words) -> Vec<FoundChat> {
    let mut seen = HashSet::new();
    search
        .first
        .iter()
        .chain(&search.server)
        .chain(&search.known)
        .filter(|chat| seen.insert(**chat))
        .filter_map(|&chat| found(store, chat, added.contains(&chat), words))
        .collect()
}

/// A chat as the popup offers it: people, bots, groups and channels (part 5), with "@anna",
/// "bot", "5 members" or "12 345 subscribers".
pub fn found(store: &Store, chat: i64, added: bool, words: &dyn Words) -> Option<FoundChat> {
    if !store.listed(chat) {
        return None;
    }
    let kind = store.chats.get(&chat)?.kind;
    let title = title(store, chat, words);
    let detail = match kind {
        _ if store.is_saved_messages(chat) => None,
        _ if store.is_bot(chat) => Some(words.tr("bot")),
        _ if store.is_channel(chat) => store
            .group(chat)
            .map(|group| crate::feed::subscribers_line(group.member_count, words)),
        ChatKind::Private { user } => store
            .users
            .get(&user)
            .filter(|user| user.kind != UserKind::Deleted)
            .and_then(|user| user.username.as_ref())
            .map(|name| format!("@{name}")),
        _ => store
            .group(chat)
            .map(|group| words.count(group.member_count.into(), "{0} member", "{0} members")),
    };
    Some(FoundChat {
        chat,
        avatar: avatar(store, chat, &title),
        title,
        detail,
        added,
        saved: store.is_saved_messages(chat),
    })
}

// --- The hidden chats ---

impl Chats {
    /// A window's request of the hidden chats: `Request::Hidden`.
    pub fn hidden(&mut self, cx: &mut Cx, window: u64, ask: HiddenAsk) {
        match ask {
            HiddenAsk::Try { text } => self.try_phrase(cx, window, &text),
            HiddenAsk::Lock => self.lock_window(window),
            HiddenAsk::Hide { key, phrase } => self.hide(cx, window, key, phrase),
            HiddenAsk::Unhide { key } => self.unhide(cx, window, key),
            HiddenAsk::SetPhrase { current, phrase } => self.set_phrase(window, current, &phrase),
            HiddenAsk::Forget => {
                storage::remove(HIDDEN);
                self.lock_all();
                self.set_phrase_state(false);
            }
        }
    }

    /// The window's search text, tried as the phrase: derived and tried on the sealed list — no
    /// other check, so a wrong text tells nothing faster.
    fn try_phrase(&mut self, cx: &mut Cx, window: u64, text: &str) {
        let sealed = storage::get::<Sealed>(HIDDEN).filter(|_| hidden::can_be_phrase(text));
        let Some(sealed) = sealed else {
            self.lock_window(window);
            return;
        };
        let started = Instant::now();
        let opened = hidden::open(&sealed, text);
        log::info(&format!(
            "A phrase tried in {} ms",
            started.elapsed().as_millis()
        ));
        let Some((key, mut list)) = opened else {
            self.lock_window(window);
            return;
        };
        let _ = service::send_json_to(
            window,
            &Note::Hidden(HiddenTold::Unlocked {
                text: text.to_string(),
            }),
        );
        if let Some(unlocked) = &mut self.unlocked {
            unlocked.windows.insert(window);
            // The rows go to every window that has them at the end of the batch.
            unlocked.sent = None;
            unlocked.dirty = true;
            return;
        }
        if self.drop_visible(&mut list) {
            storage::set(HIDDEN, &key.seal(&list, hidden::random()));
        }
        for (account, chats) in &list.accounts {
            if !self.lists.get(account).is_some_and(|list| list.ready) {
                continue;
            }
            for &chat in chats {
                cx.td.send(
                    account,
                    json!({"@type": "getChat", "chat_id": chat}),
                    Asked::Chats(Ask::Hidden(chat)),
                );
            }
        }
        self.unlocked = Some(Unlocked {
            key,
            list,
            windows: HashSet::from([window]),
            sent: None,
            dirty: true,
        });
    }

    /// Accounts signed out while the list was closed go from it, and so do chats added to the
    /// visible list meanwhile (the Add Chat popup, a link's tab, a profile can't tell they are
    /// hidden — part 5): the list opened by a phrase, before it is sealed again. True — it changed.
    fn drop_visible(&self, list: &mut HiddenList) -> bool {
        let before = list.clone();
        list.accounts
            .retain(|account, _| self.lists.contains_key(account));
        for (account, chats) in list.accounts.iter_mut() {
            if let Some(visible) = self.lists.get(account) {
                chats.retain(|chat| !visible.chats.contains(chat));
            }
        }
        list.accounts.retain(|_, chats| !chats.is_empty());
        *list != before
    }

    /// The hidden chats are hidden again in the window; once no window has them, the service
    /// forgets the key and the list.
    fn lock_window(&mut self, window: u64) {
        let Some(unlocked) = &mut self.unlocked else {
            return;
        };
        unlocked.windows.remove(&window);
        if unlocked.windows.is_empty() {
            self.unlocked = None;
        }
    }

    /// Every window hides them again: the phrase changed, the list was forgotten.
    fn lock_all(&mut self) {
        let Some(unlocked) = self.unlocked.take() else {
            return;
        };
        for window in unlocked.windows {
            let _ = service::send_json_to(window, &Note::Hidden(HiddenTold::Locked));
        }
    }

    fn set_phrase_state(&mut self, set: bool) {
        self.phrase_set = set;
        let _ = service::broadcast_json(&Note::Hidden(HiddenTold::State { set }));
    }

    /// Hides a chat: it leaves the account's list and joins the sealed one. The key is the open
    /// list's when the window has it open, else the phrase's.
    fn hide(&mut self, cx: &mut Cx, window: u64, key: ChatKey, phrase: Option<String>) {
        let Some(sealed) = storage::get::<Sealed>(HIDDEN) else {
            let _ = service::send_json_to(window, &Note::Hidden(HiddenTold::State { set: false }));
            return;
        };
        let open_here = self
            .unlocked
            .as_ref()
            .filter(|unlocked| phrase.is_none() && unlocked.windows.contains(&window));
        let (seal_key, mut list) = match (open_here, phrase) {
            (Some(unlocked), _) => (unlocked.key.clone(), unlocked.list.clone()),
            (None, Some(phrase)) => match hidden::open(&sealed, &phrase) {
                Some(opened) => opened,
                None => {
                    let _ = service::send_json_to(
                        window,
                        &Note::Hidden(HiddenTold::WrongPhrase { hide: true }),
                    );
                    return;
                }
            },
            (None, None) => return,
        };
        self.drop_visible(&mut list);
        list.hide(&key.account, key.chat);
        storage::set(HIDDEN, &seal_key.seal(&list, hidden::random()));
        if let Some(unlocked) = &mut self.unlocked {
            unlocked.list = list;
            unlocked.dirty = true;
            cx.td.send(
                &key.account,
                json!({"@type": "getChat", "chat_id": key.chat}),
                Asked::Chats(Ask::Hidden(key.chat)),
            );
        }
        if let Some(visible) = self.lists.get_mut(&key.account) {
            visible.chats.retain(|chat| *chat != key.chat);
            visible.missing.remove(&key.chat);
            storage::set(&storage_key(&key.account), &visible.chats);
            visible.dirty = true;
        }
        // The folders are plaintext: a hidden chat is never in one.
        self.leave_folders(&key);
        self.drop_card(&key);
        self.dirty_searches(&key.account);
        let _ = service::broadcast_json(&Note::Hidden(HiddenTold::Hid { key }));
    }

    /// A hidden chat back in the account's list (the window has the hidden chats open).
    fn unhide(&mut self, cx: &mut Cx, window: u64, key: ChatKey) {
        let Some(unlocked) = self
            .unlocked
            .as_mut()
            .filter(|unlocked| unlocked.windows.contains(&window))
        else {
            return;
        };
        if !unlocked.list.unhide(&key.account, key.chat) {
            return;
        }
        unlocked.dirty = true;
        storage::set(HIDDEN, &unlocked.key.seal(&unlocked.list, hidden::random()));
        self.request(cx, window, Request::AddChat { key: key.clone() });
        let _ = service::broadcast_json(&Note::Hidden(HiddenTold::Unhid { key }));
    }

    /// Settings: a new phrase seals the list (a new one, or the one the current phrase opens).
    fn set_phrase(&mut self, window: u64, current: Option<String>, phrase: &str) {
        if !hidden::can_be_phrase(phrase) {
            return;
        }
        let list = match storage::get::<Sealed>(HIDDEN) {
            None => HiddenList::default(),
            Some(sealed) => match current.and_then(|current| hidden::open(&sealed, &current)) {
                Some((_, list)) => list,
                None => {
                    let _ = service::send_json_to(
                        window,
                        &Note::Hidden(HiddenTold::WrongPhrase { hide: false }),
                    );
                    return;
                }
            },
        };
        let mut list = list;
        self.drop_visible(&mut list);
        let started = Instant::now();
        let key = hidden::new_key(phrase);
        log::info(&format!(
            "The hidden chats' key derived in {} ms",
            started.elapsed().as_millis()
        ));
        storage::set(HIDDEN, &key.seal(&list, hidden::random()));
        // The searches that had the old phrase don't have this one.
        self.lock_all();
        self.set_phrase_state(true);
        let _ = service::send_json_to(window, &Note::Hidden(HiddenTold::Saved));
    }
}

// --- The folders ---

impl Chats {
    /// A window's request of the folders: `Request::Folders`. A chat goes into a folder only from
    /// the list: never a hidden one.
    pub fn folders(&mut self, ask: FolderAsk) {
        let lists = &self.lists;
        let listed = |key: &ChatKey| {
            lists
                .get(&key.account)
                .is_some_and(|list| list.chats.contains(&key.chat))
        };
        if apply_folder(&mut self.folders, ask, listed) {
            self.save_folders();
        }
    }

    /// The chat left the list (removed, hidden): it leaves every folder.
    fn leave_folders(&mut self, key: &ChatKey) {
        if keep_listed(&mut self.folders, |kept| kept != key) {
            self.save_folders();
        }
    }

    /// The folders changed: kept, and sent to every window at the end of the batch.
    fn save_folders(&mut self) {
        storage::set(FOLDERS, &self.folders);
        self.folders_changed = true;
    }
}

/// A folder's name as typed, as kept (the window shows it so before the service does): one
/// line, trimmed, not too long; none — nothing to keep.
pub fn folder_name(name: &str) -> Option<String> {
    let line = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = line.chars().take(FOLDER_NAME_MAX).collect();
    let name = name.trim_end().to_string();
    (!name.is_empty()).then_some(name)
}

/// A window's ask applied to the folders; `listed` — the chat is in the list (only such a chat
/// goes into a folder). True — they changed.
fn apply_folder(
    folders: &mut Vec<Folder>,
    ask: FolderAsk,
    listed: impl Fn(&ChatKey) -> bool,
) -> bool {
    match ask {
        FolderAsk::Create { id, name, chats } => {
            let Some(name) = folder_name(&name) else {
                return false;
            };
            if folders.iter().any(|folder| folder.id == id) {
                return false;
            }
            let mut kept: Vec<ChatKey> = Vec::new();
            for key in chats {
                if listed(&key) && !kept.contains(&key) {
                    kept.push(key);
                }
            }
            folders.push(Folder {
                id,
                name,
                chats: kept,
            });
            true
        }
        FolderAsk::Rename { id, name } => {
            let Some(name) = folder_name(&name) else {
                return false;
            };
            match folders.iter_mut().find(|folder| folder.id == id) {
                Some(folder) if folder.name != name => {
                    folder.name = name;
                    true
                }
                _ => false,
            }
        }
        FolderAsk::Delete { id } => {
            let before = folders.len();
            folders.retain(|folder| folder.id != id);
            folders.len() != before
        }
        FolderAsk::Add { id, key } => match folders.iter_mut().find(|folder| folder.id == id) {
            Some(folder) if listed(&key) && !folder.chats.contains(&key) => {
                folder.chats.push(key);
                true
            }
            _ => false,
        },
        FolderAsk::Remove { id, key } => match folders.iter_mut().find(|folder| folder.id == id) {
            Some(folder) if folder.chats.contains(&key) => {
                folder.chats.retain(|kept| *kept != key);
                true
            }
            _ => false,
        },
    }
}

/// Only the chats `keep` lets stay in the folders; true — some left.
fn keep_listed(folders: &mut [Folder], keep: impl Fn(&ChatKey) -> bool) -> bool {
    let mut changed = false;
    for folder in folders {
        let before = folder.chats.len();
        folder.chats.retain(&keep);
        changed |= folder.chats.len() != before;
    }
    changed
}

/// The hidden chats' rows, each account's in Telegram's order: those the store has.
fn hidden_rows(
    stores: &HashMap<String, Store>,
    list: &HiddenList,
    words: &dyn Words,
) -> Vec<HiddenRow> {
    let mut rows = Vec::new();
    for (account, chats) in &list.accounts {
        let Some(store) = stores.get(account) else {
            continue;
        };
        let mut chats: Vec<i64> = chats
            .iter()
            .copied()
            .filter(|chat| store.chats.contains_key(chat) && store.listed(*chat))
            .collect();
        sort(store, &mut chats);
        rows.extend(chats.into_iter().filter_map(|chat| {
            Some(HiddenRow {
                account: account.clone(),
                row: row(store, chat, words)?,
            })
        }));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::English;

    const ME: i64 = 42;

    fn chat(id: i64, kind: Value, title: &str, order: i64, pinned: bool) -> Value {
        json!({"@type": "chat", "id": id, "type": kind, "title": title,
            "positions": [{"@type": "chatPosition", "list": {"@type": "chatListMain"},
                "order": order.to_string(), "is_pinned": pinned}],
            "unread_count": 0, "notification_settings": {"use_default_mute_for": true}})
    }

    fn message(id: i64, chat: i64, sender: i64, text: &str, date: i64, outgoing: bool) -> Value {
        json!({"@type": "message", "id": id, "chat_id": chat, "is_outgoing": outgoing,
            "date": date, "sender_id": {"@type": "messageSenderUser", "user_id": sender},
            "content": {"@type": "messageText", "text": {"text": text}}})
    }

    fn store() -> Store {
        let mut store = Store {
            me: Some(ME),
            ..Store::default()
        };
        for user in [
            json!({"@type": "user", "id": 101, "first_name": "Anna", "last_name": "Smirnova",
                "usernames": {"active_usernames": ["anna"]}}),
            json!({"@type": "user", "id": 103, "first_name": "Petya", "last_name": "Smirnov"}),
            json!({"@type": "user", "id": 105, "first_name": "Weather", "type": {"@type": "userTypeBot"}}),
            json!({"@type": "user", "id": ME, "first_name": "Egor"}),
        ] {
            store.apply(&user);
        }
        store.apply(
            &json!({"@type": "supergroup", "id": 1001, "member_count": 5,
            "status": {"@type": "chatMemberStatusMember"}}),
        );
        store.apply(
            &json!({"@type": "supergroup", "id": 1002, "member_count": 12345, "is_channel": true,
            "status": {"@type": "chatMemberStatusMember"}}),
        );
        let group =
            json!({"@type": "chatTypeSupergroup", "supergroup_id": 1001, "is_channel": false});
        let channel =
            json!({"@type": "chatTypeSupergroup", "supergroup_id": 1002, "is_channel": true});
        let private = |user: i64| json!({"@type": "chatTypePrivate", "user_id": user});
        for chat in [
            chat(-1000000001001, group, "Backend · team", 900, true),
            chat(101, private(101), "Anna Smirnova", 890, false),
            chat(103, private(103), "Petya Smirnov", 0, false),
            chat(105, private(105), "Weather Bot", 880, false),
            chat(-1000000001002, channel, "Flux News", 870, false),
            chat(ME, private(ME), "Egor", 860, false),
        ] {
            store.apply(&chat);
        }
        store
    }

    fn last(store: &mut Store, message: Value) {
        let chat = message["chat_id"].clone();
        store.apply(&json!({"@type": "updateChatLastMessage", "chat_id": chat,
            "last_message": message}));
    }

    #[test]
    fn rows_say_who_wrote_last() {
        let mut store = store();
        let team = -1000000001001;
        last(
            &mut store,
            message(5, team, 103, "Release on Friday?\nOr Thursday?", 100, false),
        );
        last(
            &mut store,
            message(7, 101, 101, "Sent you the mockups", 90, false),
        );
        last(&mut store, message(9, ME, ME, "a note", 80, true));
        assert_eq!(
            row(&store, team, &English).unwrap().last.as_deref(),
            Some("Petya: Release on Friday?")
        );
        assert_eq!(row(&store, team, &English).unwrap().title, "Backend · team");
        assert!(row(&store, team, &English).unwrap().pinned);
        assert_eq!(
            row(&store, 101, &English).unwrap().last.as_deref(),
            Some("Sent you the mockups")
        );
        assert_eq!(row(&store, 101, &English).unwrap().date, 90);
        assert_eq!(row(&store, 101, &English).unwrap().delivery, None);
        // Saved Messages: its own name, no "You:", no marks.
        assert_eq!(row(&store, ME, &English).unwrap().title, "Saved Messages");
        assert_eq!(row(&store, ME, &English).unwrap().avatar.initials, "S");
        assert_eq!(
            row(&store, ME, &English).unwrap().last.as_deref(),
            Some("a note")
        );
        assert_eq!(row(&store, ME, &English).unwrap().delivery, None);

        // The user's own: "You:", ✓ until read, ✓✓ after.
        last(&mut store, message(11, 101, ME, "See you at 11?", 95, true));
        assert_eq!(
            row(&store, 101, &English).unwrap().last.as_deref(),
            Some("You: See you at 11?")
        );
        assert_eq!(
            row(&store, 101, &English).unwrap().delivery,
            Some(Delivery::Sent)
        );
        store.apply(&json!({"@type": "updateChatReadOutbox", "chat_id": 101,
            "last_read_outbox_message_id": 11}));
        assert_eq!(
            row(&store, 101, &English).unwrap().delivery,
            Some(Delivery::Read)
        );
        let mut sending = message(12, 101, ME, "…", 96, true);
        sending["sending_state"] = json!({"@type": "messageSendingStatePending"});
        last(&mut store, sending);
        assert_eq!(
            row(&store, 101, &English).unwrap().delivery,
            Some(Delivery::Sending)
        );

        // A line of the chat itself has no sender before it.
        let mut joined = message(13, team, 103, "", 101, false);
        joined["content"] = json!({"@type": "messageChatAddMembers", "member_user_ids": [101]});
        last(&mut store, joined);
        assert_eq!(
            row(&store, team, &English).unwrap().last.as_deref(),
            Some("Anna Smirnova joined the group")
        );
    }

    #[test]
    fn rows_count_unread_and_drafts() {
        let mut store = store();
        store.apply(&json!({"@type": "updateChatReadInbox", "chat_id": 101,
            "last_read_inbox_message_id": 3, "unread_count": 4}));
        store.apply(
            &json!({"@type": "updateChatIsMarkedAsUnread", "chat_id": 103,
            "is_marked_as_unread": true}),
        );
        store.apply(&json!({"@type": "updateChatDraftMessage", "chat_id": 101,
            "draft_message": {"input_message_text": {"@type": "inputMessageText",
                "text": {"text": "  half a\nthought"}}}}));
        store.apply(
            &json!({"@type": "updateChatNotificationSettings", "chat_id": 103,
            "notification_settings": {"use_default_mute_for": false, "mute_for": 1000}}),
        );
        let anna = row(&store, 101, &English).unwrap();
        assert_eq!((anna.unread, anna.muted), (4, false));
        let petya = row(&store, 103, &English).unwrap();
        // Marked as unread: at least one.
        assert_eq!((petya.unread, petya.muted), (1, true));
        assert_eq!(anna.draft.as_deref(), Some("half a"));
    }

    #[test]
    fn hidden_rows_are_the_hidden_chats_the_store_has() {
        let mut stores = HashMap::new();
        stores.insert("a1".to_string(), store());
        let mut list = HiddenList::default();
        // Anna, a chat the store lacks (yet), the channel (part 5: shown), Backend · team.
        for chat in [101, 999, -1000000001002, -1000000001001] {
            list.hide("a1", chat);
        }
        list.hide("gone", 101);
        let rows = hidden_rows(&stores, &list, &English);
        let shown: Vec<(&str, i64)> = rows
            .iter()
            .map(|hidden| (hidden.account.as_str(), hidden.row.chat))
            .collect();
        assert_eq!(
            shown,
            [("a1", -1000000001001), ("a1", 101), ("a1", -1000000001002)]
        );
        assert!(hidden_rows(&stores, &HiddenList::default(), &English).is_empty());
    }

    #[test]
    fn rows_go_in_telegrams_order() {
        let mut store = store();
        // Petya isn't in the main list: his last message's date places him.
        last(&mut store, message(5, 103, 103, "hey", 1000, false));
        let list = List {
            chats: vec![103, 101, -1000000001001, 105, -1000000001002, ME],
            ..List::default()
        };
        let rows = rows(&store, &list, &English).unwrap();
        let order: Vec<i64> = rows.iter().map(|row| row.chat).collect();
        // Pinned first; the bot and the channel too (part 5).
        assert_eq!(
            order,
            vec![-1000000001001, 103, 101, 105, -1000000001002, ME]
        );
    }

    #[test]
    fn rows_wait_for_the_store_then_keep_what_was_sent() {
        let store = store();
        let mut list = List {
            chats: vec![101, 555],
            ..List::default()
        };
        // 555 isn't known yet: no rows rather than a part of them.
        assert_eq!(rows(&store, &list, &English), None);
        // TDLib doesn't have it: the rest goes.
        list.missing.insert(555);
        let sent = rows(&store, &list, &English).unwrap();
        assert_eq!(sent.len(), 1);
        // A fresh store (the helper started again) keeps the rows sent.
        list.sent = Some(sent.clone());
        list.missing.clear();
        assert_eq!(rows(&Store::default(), &list, &English), Some(sent));
        // An empty list is a list.
        assert_eq!(rows(&store, &List::default(), &English), Some(Vec::new()));
    }

    #[test]
    fn the_popup_offers_people_groups_bots_and_channels() {
        let store = store();
        let anna = found(&store, 101, true, &English).unwrap();
        assert_eq!(anna.detail.as_deref(), Some("@anna"));
        assert!(anna.added);
        let team = found(&store, -1000000001001, false, &English).unwrap();
        assert_eq!(team.detail.as_deref(), Some("5 members"));
        let bot = found(&store, 105, false, &English).unwrap();
        assert_eq!(bot.detail.as_deref(), Some("bot"));
        let channel = found(&store, -1000000001002, false, &English).unwrap();
        assert_eq!(channel.title, "Flux News");
        assert_eq!(channel.detail.as_deref(), Some("12\u{a0}345 subscribers"));
        assert_eq!(found(&store, 999, false, &English), None);

        let search = Search {
            account: "a1".into(),
            request: 1,
            known: vec![101, 103],
            first: vec![105, 101],
            server: vec![-1000000001002, -1000000001001],
            pending: 0,
            sent: None,
            dirty: false,
        };
        let chats: Vec<i64> = found_chats(&store, &search, &[], &English)
            .iter()
            .map(|chat| chat.chat)
            .collect();
        // TDLib's order, each once; the bot and the channel among them (part 5).
        assert_eq!(chats, vec![105, 101, -1000000001002, -1000000001001, 103]);

        assert!(matches(&store, 101, "ann", &English));
        assert!(matches(&store, 101, "@ANNA", &English));
        assert!(matches(&store, -1000000001001, "TEAM", &English));
        assert!(matches(&store, 105, "weather", &English));
        assert!(matches(&store, ME, "saved", &English));
    }

    #[test]
    fn folders_keep_chats_of_the_list_only() {
        let a = ChatKey::new("a1", 101);
        let b = ChatKey::new("a2", 7);
        let hidden = ChatKey::new("a1", 555);
        let listed = |key: &ChatKey| *key != hidden;
        let mut folders = Vec::new();
        let create = FolderAsk::Create {
            id: 1,
            name: "  Colleagues \n team ".into(),
            chats: vec![a.clone(), hidden.clone(), a.clone()],
        };
        assert!(apply_folder(&mut folders, create.clone(), listed));
        assert_eq!(folders[0].name, "Colleagues team");
        // The hidden chat never gets in; the chat once.
        assert_eq!(folders[0].chats, vec![a.clone()]);
        // The same id again, an empty name: nothing.
        assert!(!apply_folder(&mut folders, create, listed));
        let empty = FolderAsk::Create {
            id: 2,
            name: "   ".into(),
            chats: vec![],
        };
        assert!(!apply_folder(&mut folders, empty, listed));

        let add = |id, key: &ChatKey| FolderAsk::Add {
            id,
            key: key.clone(),
        };
        assert!(apply_folder(&mut folders, add(1, &b), listed));
        assert!(!apply_folder(&mut folders, add(1, &b), listed));
        assert!(!apply_folder(&mut folders, add(1, &hidden), listed));
        assert!(!apply_folder(&mut folders, add(9, &b), listed));
        assert_eq!(folders[0].chats, vec![a.clone(), b.clone()]);

        let rename = |name: &str| FolderAsk::Rename {
            id: 1,
            name: name.into(),
        };
        assert!(apply_folder(&mut folders, rename("Work"), listed));
        assert!(!apply_folder(&mut folders, rename("Work "), listed));
        assert!(!apply_folder(&mut folders, rename(""), listed));
        assert_eq!(
            folder_name(&"x".repeat(100)).unwrap().len(),
            FOLDER_NAME_MAX
        );

        let remove = FolderAsk::Remove {
            id: 1,
            key: a.clone(),
        };
        assert!(apply_folder(&mut folders, remove.clone(), listed));
        assert!(!apply_folder(&mut folders, remove, listed));
        assert_eq!(folders[0].chats, vec![b.clone()]);

        // A chat that leaves the list (removed, hidden, its account signed out) leaves them all.
        let second = FolderAsk::Create {
            id: 2,
            name: "Personal".into(),
            chats: vec![b.clone(), a.clone()],
        };
        assert!(apply_folder(&mut folders, second, listed));
        assert!(keep_listed(&mut folders, |key| key.account != "a2"));
        assert!(!keep_listed(&mut folders, |key| key.account != "a2"));
        assert_eq!(
            folders.iter().map(|f| f.chats.len()).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(apply_folder(
            &mut folders,
            FolderAsk::Delete { id: 1 },
            listed
        ));
        assert!(!apply_folder(
            &mut folders,
            FolderAsk::Delete { id: 1 },
            listed
        ));
        assert_eq!(folders.len(), 1);
    }

    #[test]
    fn folder_messages_round_trip() {
        let ask = Request::Folders(FolderAsk::Add {
            id: 3,
            key: ChatKey::new("a1", -100),
        });
        let text = serde_json::to_string(&ask).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), ask);
        let note = Note::Folders {
            folders: vec![Folder {
                id: 3,
                name: "Work".into(),
                chats: vec![ChatKey::new("a1", -100)],
            }],
        };
        let text = serde_json::to_string(&note).unwrap();
        assert_eq!(serde_json::from_str::<Note>(&text).unwrap(), note);
    }

    #[test]
    fn notifications_name_the_sender_in_groups() {
        let store = store();
        let team = -1000000001001;
        assert_eq!(
            notice_line(
                &store,
                team,
                &message(5, team, 103, "Release?", 1, false),
                &English
            ),
            "Petya: Release?"
        );
        assert_eq!(
            notice_line(&store, 101, &message(5, 101, 101, "hi", 1, false), &English),
            "hi"
        );
        let mut anonymous = message(6, team, 0, "From the admins", 1, false);
        anonymous["sender_id"] = json!({"@type": "messageSenderChat", "chat_id": team});
        assert_eq!(
            notice_line(&store, team, &anonymous, &English),
            "From the admins"
        );
    }
}
