//! The service's open chats (part 3): a chat a window opened ([`Request::OpenChat`]) gets its
//! header ([`Note::Header`]) and its feed ([`Note::Feed`]: changes of the conversation, never the
//! whole feed), then every change of them, until the last window closes it.
//!
//! - Opening: TDLib's `openChat`, the history around the first unread message (`getChatHistory`;
//!   TDLib may answer with fewer messages than asked — ask again from the oldest one), the "New
//!   messages" line (`unread-from`), the draft. `load-older` / `load-newer` page on.
//! - The messages go through [`crate::convert`] (TDLib's message → the conversation's message)
//!   once, in the service, and to every window that has the chat open; the feed keeps the loaded
//!   messages of each open chat for quotes and changes.
//! - Updates of an open chat: new, edited, deleted messages, sent ones (the temporary id goes, the
//!   server's comes), reactions, read marks (✓✓), who is typing, files downloading (the progress
//!   on the picture or the file, the path once it is there).
//! - What the user does: `Visible` marks the messages read (`viewMessages`) and downloads their
//!   pictures first; `Send` (text, files — pictures as photos, several as an album — the reply),
//!   `Edit`, `Delete`, `Download`, `JumpTo` (the history around a quote's message, then
//!   `reveal`), `Draft` ("typing…" to the chat, the draft kept in Telegram).
//! - A window's search in a chat ([`SearchAsk`]): `searchChatMessages`, a page of the matches'
//!   ids (newest first, with Telegram's count) to that window only ([`SearchTold`]); the window
//!   jumps to them (`JumpTo`) and marks the words itself — the messages here stay as they are.
//!
//! The changes of a batch (the helper's lines, a request, a timer) are coalesced: one
//! [`Note::Feed`] per chat at [`Feed::flush`], a message converted again only when something it
//! shows changed, and put only when it came out different; the header only when it changed. The
//! one timer that may run is the typing's expiry, and only while someone types.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use flux_plugin_api::conversation::{self as conv, Change, Composed, History};
use flux_plugin_api::system::LocalTime;
use flux_plugin_api::{log, notify, service, system, timers};
use serde_json::{Value, json};

use crate::convert::{self, Context, Converted};
use crate::model::{ChatHeader, ChatKey, Note, Request, Words, unix_now};
use crate::service::{Asked, Cx, HostWords};
use crate::store::{ChatKind, Status, Store, Touched, UserKind};
use crate::summary;

/// What a chat's search asks the service: `Request::Search`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "search", rename_all = "kebab-case")]
pub enum SearchAsk {
    /// The messages of the chat that have `query` (Telegram's search: words by their beginnings),
    /// newest first: the first page (`from` 0), or the next one (`from` — the last answer's
    /// `next`). The answer is [`SearchTold::Found`] with the same `request`, to this window only.
    Find {
        key: ChatKey,
        query: String,
        request: u64,
        from: i64,
    },
}

/// What the service tells a chat's search: `Note::Search`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "search", rename_all = "kebab-case")]
pub enum SearchTold {
    /// A page of the matches of `request` (asked `from`): the messages' ids, newest first;
    /// `total` — how many Telegram counts in all (-1: it doesn't know); `next` — where the next
    /// page starts (0: no more); `failed` — Telegram couldn't search.
    Found {
        key: ChatKey,
        request: u64,
        from: i64,
        ids: Vec<String>,
        total: i64,
        next: i64,
        failed: bool,
    },
}

/// TDLib's updates the feed needs beyond the store's (`model::KEPT`).
pub const KEPT: &[&str] = &[
    "updateNewMessage",
    "updateMessageSendSucceeded",
    "updateMessageSendFailed",
    "updateMessageContent",
    "updateMessageEdited",
    "updateMessageInteractionInfo",
    "updateDeleteMessages",
    "updateChatAction",
];

/// A page of history.
const PAGE: usize = 50;
/// TDLib answers the first page in pieces: this many asks at most.
const FIRST_TRIES: u8 = 6;
/// Someone's "typing…" goes after this long without a repeat (Telegram's clients repeat it every
/// 5 s).
const TYPING_FOR: Duration = Duration::from_secs(6);
/// The user's "typing…" goes to the chat at most this often.
const TYPING_EVERY: Duration = Duration::from_secs(5);
/// The draft goes to Telegram this long after the field stops changing.
const DRAFT_AFTER: Duration = Duration::from_secs(2);
/// Download priorities (TDLib's 1–32): a click, then what is on screen.
const CLICKED: i64 = 32;
const IN_SIGHT: i64 = 24;
/// The photos' full sizes on screen: after the rest, so that a click opens the viewer on them.
const FULL: i64 = 8;
/// The pictures sent as photos; other files go as documents.
const PICTURES: &[&str] = &["jpg", "jpeg", "png", "webp", "heic"];
/// An album holds this many photos or files at most.
const ALBUM: usize = 10;
/// A page of a chat's search's matches (TDLib's most).
const SEARCH_PAGE: u32 = 100;

/// What a request of the feed asked: its answer comes to [`Feed::answer`].
#[derive(Debug)]
pub enum Ask {
    /// A page of the chat's history, for its load `epoch` (a jump starts a new one).
    History { chat: i64, epoch: u64, page: Page },
    /// The message a reply quotes, outside the loaded feed.
    Replied { chat: i64 },
    /// A user or a chat a message names: the store took it.
    Named,
    /// The chat of a chat being opened, which the store lacked: the history follows.
    Opened { chat: i64, epoch: u64 },
    /// A message, or an album, sent: the pending messages come back.
    Sent { chat: i64 },
    /// The field's text (`text`) through TDLib's Markdown (`parseMarkdown`: `**bold**`,
    /// `__italic__`, `~~strike~~`, `||spoiler||`, `` `code` ``, ```` ```pre``` ````,
    /// `[text](url)`), as Telegram's apps send it: then sent, or saved as an edit.
    Markdown { chat: i64, text: String, then: Then },
    /// A page of a window's search in the chat (`searchChatMessages`).
    Search {
        window: u64,
        chat: i64,
        request: u64,
        from: i64,
    },
    /// Anything else: an error is logged, and `told` to the user when it is theirs to know.
    Done {
        what: &'static str,
        told: Option<&'static str>,
    },
}

/// What the field's text is parsed for ([`Ask::Markdown`]).
#[derive(Debug)]
pub enum Then {
    /// Sent with the field's files and reply.
    Send(Box<Composed>),
    /// A message's new text, or its caption (an album's: its captioned item).
    Edit { message: i64 },
}

/// Which page a history request is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    /// The first fill: around the last read message (`around`), else the newest; asked again
    /// from the oldest received while TDLib gives fewer than a page.
    First {
        around: Option<i64>,
        tries: u8,
    },
    Older,
    Newer,
    /// The history around a quote's message, which is revealed then.
    Jump {
        target: i64,
    },
}

/// How far an open chat's first fill is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Load {
    /// The account's client isn't ready: the chat loads once it is.
    Waiting,
    /// The first page is on its way.
    First,
    Ready,
}

/// A chat open in some windows.
struct Open {
    windows: BTreeSet<u64>,
    load: Load,
    /// A jump or a reload starts a new load: pages asked before it are dropped.
    epoch: u64,
    /// When the window asked for it: the first fill's time goes to the log.
    asked_at: Instant,
    /// The loaded messages as TDLib gave them, by id.
    raw: BTreeMap<i64, Value>,
    /// The same as the windows have them: an album is one message, under its first item's id.
    shown: HashMap<i64, Converted>,
    /// The loaded albums (TDLib's `media_album_id`: the photos and videos sent together, whose
    /// items come as messages of their own): the items of each, the album of each item.
    albums: HashMap<i64, BTreeSet<i64>>,
    album_of: HashMap<i64, i64>,
    /// The id each album is shown under, as the windows have it (its first item's then).
    album_shown: HashMap<i64, i64>,
    /// The server's album ids of the user's albums sent now: the pending album's, which their
    /// items keep here.
    album_aliases: HashMap<i64, i64>,
    /// Albums that lost items (deleted, sent): put again at the end of the batch, or removed with
    /// the last one.
    dirty_albums: BTreeSet<i64>,
    /// Messages outside the feed that loaded ones answer, fetched for their quotes.
    replied: HashMap<i64, Value>,
    /// The loaded messages answering each message.
    replies: HashMap<i64, BTreeSet<i64>>,
    /// The loaded messages showing each file.
    files: HashMap<i64, BTreeSet<i64>>,
    /// The loaded messages naming a user or a chat the store didn't know.
    named: HashMap<i64, BTreeSet<i64>>,
    /// The messages on screen, the first and the last (`visible`).
    visible: Option<(i64, i64)>,
    /// Pictures of messages on screen that came to be known only now (an author's photo): they
    /// are downloaded at the end of the batch — the file, its priority.
    wanted_files: Vec<(i64, i64)>,
    /// What the messages converted in this batch lack: users, chats, replied messages — asked
    /// for at its end.
    wanted_users: Vec<i64>,
    wanted_chats: Vec<i64>,
    wanted_replies: Vec<i64>,
    older: History,
    newer: History,
    paging_older: bool,
    paging_newer: bool,
    /// The first unread message of the fill: the "New messages" line.
    unread_from: Option<i64>,
    /// Changes for the windows at the end of the batch, in order; `dirty` messages are put after.
    queue: Vec<Change>,
    dirty: BTreeSet<i64>,
    /// The header the windows have.
    header: Option<ChatHeader>,
    header_dirty: bool,
    /// Who is typing (a user's or a chat's id): what, since when.
    typing: BTreeMap<i64, (String, Instant)>,
    /// The chat's `last_read_outbox` and the user's right to write, as last shown.
    read_outbox: i64,
    can_send: bool,
    /// The user's "typing…" last sent, and the draft waiting for its timer.
    typed_at: Option<Instant>,
    draft: Option<(u64, String)>,
}

impl Open {
    fn new(window: u64) -> Self {
        Open {
            windows: BTreeSet::from([window]),
            load: Load::Waiting,
            epoch: 0,
            asked_at: Instant::now(),
            raw: BTreeMap::new(),
            shown: HashMap::new(),
            albums: HashMap::new(),
            album_of: HashMap::new(),
            album_shown: HashMap::new(),
            album_aliases: HashMap::new(),
            dirty_albums: BTreeSet::new(),
            replied: HashMap::new(),
            replies: HashMap::new(),
            files: HashMap::new(),
            named: HashMap::new(),
            visible: None,
            wanted_files: Vec::new(),
            wanted_users: Vec::new(),
            wanted_chats: Vec::new(),
            wanted_replies: Vec::new(),
            older: History::More,
            newer: History::Complete,
            paging_older: false,
            paging_newer: false,
            unread_from: None,
            queue: Vec::new(),
            dirty: BTreeSet::new(),
            header: None,
            header_dirty: true,
            typing: BTreeMap::new(),
            read_outbox: 0,
            can_send: false,
            typed_at: None,
            draft: None,
        }
    }

    /// Forgets the loaded messages: a jump, a reload (the caller starts a new epoch).
    fn reset(&mut self) {
        self.raw.clear();
        self.shown.clear();
        self.albums.clear();
        self.album_of.clear();
        self.album_shown.clear();
        self.album_aliases.clear();
        self.dirty_albums.clear();
        self.replies.clear();
        self.files.clear();
        self.named.clear();
        self.queue.clear();
        self.dirty.clear();
        self.paging_older = false;
        self.paging_newer = false;
    }

    /// Keeps a message of the chat, what it answers, the album it is in.
    fn insert(&mut self, message: Value) -> Option<i64> {
        let id = message["id"].as_i64()?;
        if let Some(target) = message["reply_to"]["message_id"].as_i64() {
            self.replies.entry(target).or_default().insert(id);
        }
        let album = convert::album_id(&message);
        let album = (album != 0).then(|| *self.album_aliases.get(&album).unwrap_or(&album));
        if self.album_of.get(&id).copied() != album {
            self.leave_album(id);
            if let Some(album) = album {
                self.album_of.insert(id, album);
                self.albums.entry(album).or_default().insert(id);
            }
        }
        self.raw.insert(id, message);
        Some(id)
    }

    /// The message leaves its album (gone, or in another one): the album is put again.
    fn leave_album(&mut self, id: i64) {
        let Some(album) = self.album_of.remove(&id) else {
            return;
        };
        if let Some(items) = self.albums.get_mut(&album) {
            items.remove(&id);
            if items.is_empty() {
                self.albums.remove(&album);
            }
        }
        self.dirty_albums.insert(album);
    }

    /// The items of the album a message is in (by any item's id), in order; a message not in an
    /// album — itself.
    fn items_of(&self, id: i64) -> Vec<i64> {
        self.album_of
            .get(&id)
            .and_then(|album| self.albums.get(album))
            .map_or_else(|| vec![id], |items| items.iter().copied().collect())
    }

    /// The id a loaded message is shown under: its album's (the first item's), or its own.
    fn shown_id(&self, id: i64) -> i64 {
        self.items_of(id).first().copied().unwrap_or(id)
    }

    /// The item an album's caption is edited in: the one that has it, else the first.
    fn captioned(&self, id: i64) -> i64 {
        let items = self.items_of(id);
        items
            .iter()
            .copied()
            .find(|item| self.raw.get(item).is_some_and(convert::has_caption))
            .or(items.first().copied())
            .unwrap_or(id)
    }

    /// An album not shown yet at an end of the loaded history where more of it may come (a
    /// page's edge may split it): it waits for the rest rather than show as a half.
    fn held(&self, album: i64, items: &[i64]) -> bool {
        if items.len() >= ALBUM || self.album_shown.contains_key(&album) {
            return false;
        }
        let older = self.older != History::Complete && items.first().copied() == self.oldest();
        let newer = self.newer != History::Complete && items.last().copied() == self.newest();
        older || newer
    }

    /// Takes a message of the chat: new, or changed — it, and the messages quoting it, are put
    /// again.
    fn take(&mut self, message: Value) {
        let Some(id) = self.insert(message) else {
            return;
        };
        if let Some(replying) = self.replies.get(&id) {
            self.dirty.extend(replying.iter().copied());
        }
        self.dirty.insert(id);
    }

    /// Drops messages: deleted, or replaced by the server's (a sent one). An album's item leaves
    /// the album, which is put again at the end of the batch (removed with its last item).
    fn drop_messages(&mut self, ids: &[i64]) {
        let mut gone = Vec::new();
        for id in ids {
            if self.raw.remove(id).is_some() {
                self.dirty.remove(id);
                if self.album_of.contains_key(id) {
                    self.leave_album(*id);
                } else if self.shown.remove(id).is_some() {
                    gone.push(id.to_string());
                }
            }
            if let Some(replying) = self.replies.get(id) {
                self.dirty.extend(replying.iter().copied());
            }
        }
        if !gone.is_empty() {
            self.queue.push(conv::remove(gone));
        }
    }

    /// A message sent, or failed: the server's (or the failed one) replaces the pending one —
    /// in its album, when it was in one: the server's album id stands for the pending album's,
    /// so the album stays one message while its items are sent one by one. `keep` — taken even
    /// when the pending one isn't loaded.
    fn replace(&mut self, old: i64, message: Value, keep: bool) {
        if let Some(album) = self.album_of.get(&old).copied() {
            let sent = convert::album_id(&message);
            if sent != 0 && sent != album {
                self.album_aliases.insert(sent, album);
            }
        }
        let had = self.raw.contains_key(&old);
        self.drop_messages(&[old]);
        if had || keep {
            self.take(message);
        }
    }

    fn oldest(&self) -> Option<i64> {
        self.raw.keys().next().copied()
    }

    fn newest(&self) -> Option<i64> {
        self.raw.keys().next_back().copied()
    }

    /// The whole feed for a window that opens the chat now: what the others have.
    fn snapshot(&self, store: Option<&Store>, chat: i64) -> Vec<Change> {
        let mut messages: Vec<_> = self
            .shown
            .values()
            .map(|converted| converted.message.clone())
            .collect();
        messages.sort_by_key(|message| message.order);
        let mut changes = vec![conv::clear(), conv::put(messages)];
        changes.extend(self.state(store, chat));
        changes
    }

    /// The feed's state besides its messages: the history's ends, the "New messages" line, the
    /// draft.
    fn state(&self, store: Option<&Store>, chat: i64) -> Vec<Change> {
        let mut changes = vec![
            conv::older(self.older),
            conv::newer(self.newer),
            conv::unread_from(self.unread_from.map(|id| id.to_string()).as_deref()),
        ];
        if let Some(draft) = store
            .and_then(|store| store.chats.get(&chat))
            .and_then(|chat| chat.draft.as_deref())
        {
            changes.push(conv::draft(draft));
        }
        changes
    }
}

pub struct Feed {
    chats: HashMap<ChatKey, Open>,
    /// The accounts whose client is ready.
    ready: HashSet<String>,
    /// Users, chats and replied messages asked for once, by account.
    asked: HashSet<(String, i64, i64)>,
    /// Files asked to download, by account.
    downloading: HashSet<(String, i64)>,
    /// The downloads the feed started by itself (the pictures on screen, the full sizes), by
    /// account and file: their chat. Those of messages that left the screen are cancelled; a
    /// click's aren't here.
    auto: HashMap<(String, i64), i64>,
    /// The typing's expiry timer, while someone types.
    typing_timer: Option<u64>,
    /// The drafts' timers: their chat.
    draft_timers: HashMap<u64, ChatKey>,
    /// The part of a message the reply over a chat's field quotes ([`Feed::quote`]): the
    /// message, the text, its offset in the message's text (UTF-16 units, as TDLib counts).
    quotes: HashMap<ChatKey, (String, String, u32)>,
    /// The last load's epoch: each load and jump takes a new one, so a page asked for a chat
    /// closed and opened again, or before a jump, is dropped.
    epochs: u64,
}

impl Feed {
    pub fn new() -> Self {
        Feed {
            chats: HashMap::new(),
            ready: HashSet::new(),
            asked: HashSet::new(),
            downloading: HashSet::new(),
            auto: HashMap::new(),
            typing_timer: None,
            draft_timers: HashMap::new(),
            quotes: HashMap::new(),
            epochs: 0,
        }
    }

    /// The account's chats open in some window: the helper passes their updates.
    pub fn open(&self, account: &str) -> Vec<i64> {
        self.chats
            .keys()
            .filter(|key| key.account == account)
            .map(|key| key.chat)
            .collect()
    }

    /// The account's client is ready (signed in now, or the helper started again): the chats
    /// open in windows are loaded again.
    pub fn account_ready(&mut self, cx: &mut Cx, account: &str) {
        self.ready.insert(account.to_string());
        self.asked.retain(|(asked, _, _)| asked != account);
        self.downloading.retain(|(asked, _)| asked != account);
        self.auto.retain(|(asked, _), _| asked != account);
        let keys: Vec<ChatKey> = self
            .chats
            .keys()
            .filter(|key| key.account == account)
            .cloned()
            .collect();
        for key in keys {
            self.load(cx, &key);
        }
    }

    /// The account is gone (signed out): its open chats are forgotten.
    pub fn account_gone(&mut self, account: &str) {
        self.ready.remove(account);
        self.chats.retain(|key, _| key.account != account);
        self.asked.retain(|(asked, _, _)| asked != account);
        self.downloading.retain(|(asked, _)| asked != account);
        self.auto.retain(|(asked, _), _| asked != account);
        self.draft_timers.retain(|timer, key| {
            let gone = key.account == account;
            if gone {
                timers::cancel(*timer);
            }
            !gone
        });
    }

    /// The chat's first fill: `openChat`, the history around the first unread message.
    fn load(&mut self, cx: &mut Cx, key: &ChatKey) {
        let Some(open) = self.chats.get_mut(key) else {
            return;
        };
        open.reset();
        self.epochs += 1;
        open.epoch = self.epochs;
        open.asked_at = Instant::now();
        open.load = Load::First;
        open.header_dirty = true;
        open.typing.clear();
        let chat = key.chat;
        let account = &key.account;
        cx.td.send(
            account,
            json!({"@type": "openChat", "chat_id": chat}),
            Asked::Feed(Ask::Done {
                what: "openChat",
                told: None,
            }),
        );
        // The history starts from the chat's read marks: the chat first, when the store lacks it.
        if cx
            .stores
            .get(account)
            .is_none_or(|store| !store.chats.contains_key(&chat))
        {
            let epoch = open.epoch;
            cx.td.send(
                account,
                json!({"@type": "getChat", "chat_id": chat}),
                Asked::Feed(Ask::Opened { chat, epoch }),
            );
            return;
        }
        first_page(cx, key, open);
    }

    /// An update of the account's client, after the store took it (`touched` — what it changed).
    pub fn update(&mut self, cx: &mut Cx, account: &str, update: &Value, touched: Touched) {
        match touched {
            Touched::Nothing => {}
            Touched::Chat(chat) => self.chat_touched(cx, account, chat),
            Touched::User(user) => {
                for (key, open) in self.chats.iter_mut() {
                    if key.account != account {
                        continue;
                    }
                    open.header_dirty = true;
                    if let Some(waiting) = open.named.remove(&user) {
                        open.dirty.extend(waiting);
                    }
                }
            }
            Touched::File(file) => {
                for (key, open) in self.chats.iter_mut() {
                    if key.account != account {
                        continue;
                    }
                    open.header_dirty = true;
                    if let Some(messages) = open.files.get(&file) {
                        open.dirty.extend(messages.iter().copied());
                    }
                }
            }
            Touched::Scope => {
                for (key, open) in self.chats.iter_mut() {
                    if key.account == account {
                        open.header_dirty = true;
                    }
                }
            }
        }
        let kind = update["@type"].as_str().unwrap_or_default();
        let chat = update["chat_id"]
            .as_i64()
            .or_else(|| update["message"]["chat_id"].as_i64())
            .unwrap_or_default();
        let key = ChatKey::new(account, chat);
        let Some(open) = self.chats.get_mut(&key) else {
            return;
        };
        if open.load != Load::Ready {
            // The first fill is on its way: it brings what changed meanwhile.
            return;
        }
        let message_id = update["message_id"].as_i64().unwrap_or_default();
        match kind {
            "updateNewMessage" => {
                // After a jump the newest messages aren't loaded: the new one waits for them.
                if open.newer == History::Complete {
                    open.take(update["message"].clone());
                }
            }
            "updateMessageSendSucceeded" | "updateMessageSendFailed" => {
                let old = update["old_message_id"].as_i64().unwrap_or_default();
                let keep = open.newer == History::Complete;
                open.replace(old, update["message"].clone(), keep);
                if kind == "updateMessageSendFailed" {
                    let error = update["error"]["message"].as_str().unwrap_or_default();
                    log::warn(&format!("{}: a message wasn't sent: {error}", key.account));
                }
            }
            "updateMessageContent" => {
                if let Some(message) = open.raw.get_mut(&message_id) {
                    message["content"] = update["new_content"].clone();
                    let message = message.clone();
                    open.take(message);
                }
            }
            "updateMessageEdited" => {
                if let Some(message) = open.raw.get_mut(&message_id) {
                    message["edit_date"] = update["edit_date"].clone();
                    open.dirty.insert(message_id);
                }
            }
            "updateMessageInteractionInfo" => {
                if let Some(message) = open.raw.get_mut(&message_id) {
                    message["interaction_info"] = update["interaction_info"].clone();
                    open.dirty.insert(message_id);
                }
            }
            "updateDeleteMessages" => {
                // Gone from TDLib's cache only: they are still in Telegram.
                if update["from_cache"] == true {
                    return;
                }
                let ids: Vec<i64> = update["message_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                    .collect();
                open.drop_messages(&ids);
            }
            "updateChatAction" => {
                let sender = &update["sender_id"];
                let who = sender["user_id"]
                    .as_i64()
                    .or_else(|| sender["chat_id"].as_i64())
                    .unwrap_or_default();
                let action = update["action"]["@type"].as_str().unwrap_or_default();
                // Not the user's own, and not in a forum's topic.
                if update["message_thread_id"].as_i64().unwrap_or_default() != 0
                    || Some(who) == cx.stores.get(account).and_then(|store| store.me)
                {
                    return;
                }
                let changed = if action == "chatActionCancel" || action.is_empty() {
                    open.typing.remove(&who).is_some()
                } else {
                    let before = open
                        .typing
                        .insert(who, (action.to_string(), Instant::now()));
                    before.is_none_or(|(was, _)| was != action)
                };
                if changed {
                    open.header_dirty = true;
                }
                self.schedule_typing();
            }
            _ => {}
        }
    }

    /// The store changed a chat: its header, its read marks (✓✓), the right to write (Reply).
    fn chat_touched(&mut self, cx: &mut Cx, account: &str, chat: i64) {
        let Some(store) = cx.stores.get(account) else {
            return;
        };
        for (key, open) in self.chats.iter_mut() {
            if key.account != account {
                continue;
            }
            if let Some(waiting) = open.named.remove(&chat) {
                open.dirty.extend(waiting);
            }
            if key.chat != chat {
                continue;
            }
            open.header_dirty = true;
            let Some(found) = store.chats.get(&chat) else {
                continue;
            };
            if found.last_read_outbox > open.read_outbox {
                let read = open.read_outbox + 1..=found.last_read_outbox;
                let mine: Vec<i64> = open
                    .raw
                    .range(read)
                    .filter(|(_, message)| message["is_outgoing"] == true)
                    .map(|(id, _)| *id)
                    .collect();
                open.dirty.extend(mine);
                open.read_outbox = found.last_read_outbox;
            }
            let can_send = store.can_send(chat);
            if can_send != open.can_send {
                open.can_send = can_send;
                open.dirty.extend(open.raw.keys().copied());
            }
        }
    }

    /// The answer to a request of the feed.
    pub fn answer(&mut self, cx: &mut Cx, account: &str, ask: Ask, answer: &Value) {
        let error = (answer["@type"] == "error")
            .then(|| answer["message"].as_str().unwrap_or_default().to_string());
        match ask {
            Ask::History { chat, epoch, page } => {
                let key = ChatKey::new(account, chat);
                self.history_answer(cx, &key, epoch, page, answer, error);
            }
            Ask::Replied { chat } => {
                let key = ChatKey::new(account, chat);
                let Some(open) = self.chats.get_mut(&key) else {
                    return;
                };
                let Some(id) = answer["id"].as_i64().filter(|_| error.is_none()) else {
                    return;
                };
                if let Some(replying) = open.replies.get(&id) {
                    open.dirty.extend(replying.iter().copied());
                }
                open.replied.insert(id, answer.clone());
            }
            Ask::Opened { chat, epoch } => {
                let key = ChatKey::new(account, chat);
                self.update(cx, account, &Value::Null, Touched::Chat(chat));
                let Some(open) = self
                    .chats
                    .get(&key)
                    .filter(|open| open.epoch == epoch && open.load == Load::First)
                else {
                    return;
                };
                if let Some(error) = error {
                    log::warn(&format!("{}: getChat failed: {error}", key.account));
                }
                first_page(cx, &key, open);
            }
            // The store took the user or the chat; `update`'s touches don't come for answers.
            Ask::Named => {
                let touched = match answer["@type"].as_str().unwrap_or_default() {
                    "user" => answer["id"].as_i64().map(Touched::User),
                    "chat" => answer["id"].as_i64().map(Touched::Chat),
                    "basicGroup" => answer["id"]
                        .as_i64()
                        .map(|id| Touched::Chat(crate::store::group_chat_id(id, false))),
                    "supergroup" => answer["id"]
                        .as_i64()
                        .map(|id| Touched::Chat(crate::store::group_chat_id(id, true))),
                    _ => None,
                };
                if let Some(touched) = touched {
                    self.update(cx, account, &Value::Null, touched);
                }
            }
            Ask::Sent { chat } => {
                if let Some(error) = error {
                    log::warn(&format!("{account}:{chat}: sendMessage failed: {error}"));
                    notify::error(&HostWords.tr("Couldn't send the message"), Some(&error));
                    return;
                }
                // The pending message (or an album's): shown at once, as `updateNewMessage` would.
                let key = ChatKey::new(account, chat);
                let Some(open) = self.chats.get_mut(&key) else {
                    return;
                };
                if open.newer != History::Complete {
                    return;
                }
                let messages = match answer["@type"].as_str().unwrap_or_default() {
                    "messages" => answer["messages"].as_array().cloned().unwrap_or_default(),
                    "message" => vec![answer.clone()],
                    _ => Vec::new(),
                };
                for message in messages {
                    let id = message["id"].as_i64().unwrap_or_default();
                    // Already succeeded meanwhile: the server's message replaced it.
                    if !open.raw.contains_key(&id) && message["sending_state"].is_object() {
                        open.take(message);
                    }
                }
            }
            Ask::Markdown { chat, text, then } => {
                let key = ChatKey::new(account, chat);
                // TDLib's text with its entities; the plain text when it couldn't parse it.
                let formatted = match error {
                    None if answer["@type"] == "formattedText" => answer.clone(),
                    _ => formatted_text(&text),
                };
                match then {
                    Then::Send(composed) => self.send_formatted(cx, &key, &composed, formatted),
                    Then::Edit { message } => self.edit_formatted(cx, &key, message, formatted),
                }
            }
            Ask::Search {
                window,
                chat,
                request,
                from,
            } => {
                if let Some(error) = &error {
                    log::warn(&format!(
                        "{account}:{chat}: searchChatMessages failed: {error}"
                    ));
                }
                // An album's item loaded here: the album (once, however many items match).
                let open = self.chats.get(&ChatKey::new(account, chat));
                let mut ids: Vec<String> = Vec::new();
                for id in answer["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|message| message["id"].as_i64())
                {
                    let id = open.map_or(id, |open| open.shown_id(id)).to_string();
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                let told = SearchTold::Found {
                    key: ChatKey::new(account, chat),
                    request,
                    from,
                    ids,
                    total: answer["total_count"].as_i64().unwrap_or(-1),
                    next: answer["next_from_message_id"].as_i64().unwrap_or_default(),
                    failed: error.is_some(),
                };
                let _ = service::send_json_to(window, &Note::Search(told));
            }
            Ask::Done { what, told } => {
                if let Some(error) = error {
                    log::warn(&format!("{account}: {what} failed: {error}"));
                    if let Some(told) = told {
                        notify::error(&HostWords.tr(told), Some(&error));
                    }
                }
            }
        }
    }

    fn history_answer(
        &mut self,
        cx: &mut Cx,
        key: &ChatKey,
        epoch: u64,
        page: Page,
        answer: &Value,
        error: Option<String>,
    ) {
        let Some(open) = self.chats.get_mut(key) else {
            return;
        };
        if open.epoch != epoch {
            return;
        }
        let messages: Vec<Value> = answer["messages"]
            .as_array()
            .map(|list| list.iter().filter(|m| m.is_object()).cloned().collect())
            .unwrap_or_default();
        if let Some(error) = &error {
            log::warn(&format!("{}: getChatHistory failed: {error}", key.account));
        }
        let last = cx
            .stores
            .get(&key.account)
            .and_then(|store| store.chats.get(&key.chat))
            .and_then(|chat| chat.last_message.as_ref())
            .and_then(|message| message["id"].as_i64())
            .unwrap_or_default();
        match page {
            Page::First { around, tries } => {
                let oldest_before = open.oldest();
                let received = messages.len();
                for message in messages {
                    open.insert(message);
                }
                // TDLib gives what it has at hand first: ask again from the oldest received
                // until the page is full or the history ends.
                let started = open.oldest() != oldest_before || oldest_before.is_none();
                // The unread messages below the last read one didn't come: asked again.
                let unread_missing = around
                    .is_some_and(|read| last > read && open.raw.range(read + 1..).next().is_none());
                if error.is_none()
                    && received > 0
                    && tries < FIRST_TRIES
                    && let Some(oldest) = open.oldest()
                {
                    let next = Page::First {
                        around,
                        tries: tries + 1,
                    };
                    let epoch = open.epoch;
                    if unread_missing {
                        let read = around.unwrap_or_default();
                        history(cx, key, epoch, next, read, -(PAGE as i64 / 2));
                        return;
                    }
                    if open.raw.len() < PAGE && started {
                        history(cx, key, epoch, next, oldest, 0);
                        return;
                    }
                }
                let complete = error.is_none() && (received == 0 || open.raw.len() < PAGE);
                open.older = if complete && open.raw.len() < PAGE {
                    History::Complete
                } else {
                    History::More
                };
                open.newer = match around {
                    Some(_) if open.newest().is_some_and(|newest| newest < last) => History::More,
                    _ => History::Complete,
                };
                open.unread_from = around.and_then(|read| {
                    open.raw
                        .range(read + 1..)
                        .find(|(_, message)| message["is_outgoing"] != true)
                        .map(|(id, _)| open.shown_id(*id))
                });
                open.load = Load::Ready;
                open.dirty.extend(open.raw.keys().copied());
                if let Some(store) = cx.stores.get(&key.account) {
                    open.read_outbox = store
                        .chats
                        .get(&key.chat)
                        .map_or(0, |chat| chat.last_read_outbox);
                    open.can_send = store.can_send(key.chat);
                }
                // The account only: the log never names a chat (a hidden one included).
                log::info(&format!(
                    "{}: a chat's first page ({} messages) in {} ms",
                    key.account,
                    open.raw.len(),
                    open.asked_at.elapsed().as_millis()
                ));
                // The whole fill at once: Flux opens it at the "New messages" line.
                let windows: Vec<u64> = open.windows.iter().copied().collect();
                let ids: Vec<i64> = open.raw.keys().copied().collect();
                let store = cx.stores.get(&key.account);
                let (_, converted) = convert_all(cx, key, open, &ids);
                open.dirty.clear();
                let mut changes = vec![conv::clear(), conv::put(converted)];
                changes.extend(open.state(store, key.chat));
                for window in windows {
                    let _ = service::send_json_to(
                        window,
                        &Note::Feed {
                            key: key.clone(),
                            changes: changes.clone(),
                        },
                    );
                }
            }
            Page::Older => {
                open.paging_older = false;
                let before = open.oldest().unwrap_or(i64::MAX);
                let older: Vec<Value> = messages
                    .into_iter()
                    .filter(|m| m["id"].as_i64().is_some_and(|id| id < before))
                    .collect();
                open.older = if error.is_none() && older.is_empty() {
                    History::Complete
                } else {
                    History::More
                };
                // An album held at the old edge: shown with the rest, or as it is at the start.
                if let Some(edge) = open.oldest() {
                    open.dirty.insert(edge);
                }
                page_in(cx, key, open, older);
                let older = open.older;
                open.queue.push(conv::older(older));
            }
            Page::Newer => {
                open.paging_newer = false;
                let after = open.newest().unwrap_or(i64::MIN);
                let newer: Vec<Value> = messages
                    .into_iter()
                    .filter(|m| m["id"].as_i64().is_some_and(|id| id > after))
                    .collect();
                let reached = newer.is_empty()
                    || newer
                        .iter()
                        .any(|m| m["id"].as_i64().is_some_and(|id| id >= last));
                open.newer = if error.is_none() && reached {
                    History::Complete
                } else {
                    History::More
                };
                // An album held at the new edge: shown with the rest, or as it is at the end.
                if let Some(edge) = open.newest() {
                    open.dirty.insert(edge);
                }
                page_in(cx, key, open, newer);
                let newer = open.newer;
                open.queue.push(conv::newer(newer));
            }
            Page::Jump { target } => {
                if messages.is_empty() {
                    return;
                }
                open.reset();
                let ids: Vec<i64> = messages.iter().filter_map(|m| m["id"].as_i64()).collect();
                let reached = ids.iter().any(|id| *id >= last);
                for message in messages {
                    open.insert(message);
                }
                open.older = History::More;
                open.newer = if reached {
                    History::Complete
                } else {
                    History::More
                };
                let ids: Vec<i64> = open.raw.keys().copied().collect();
                let (_, converted) = convert_all(cx, key, open, &ids);
                // The message itself, or the one before it (deleted); an album's item — the album.
                let target = open
                    .raw
                    .range(..=target)
                    .next_back()
                    .map_or(target, |(id, _)| open.shown_id(*id));
                let (older, newer) = (open.older, open.newer);
                open.queue.extend([
                    conv::clear(),
                    conv::put(converted),
                    conv::older(older),
                    conv::newer(newer),
                    conv::reveal(&target.to_string()),
                ]);
            }
        }
    }

    /// A window's request: `OpenChat`, `CloseChat`, `LoadOlder`, `LoadNewer`, `Visible`, `Send`,
    /// `Edit`, `Delete`, `Download`, `JumpTo`, `Draft`.
    pub fn request(&mut self, cx: &mut Cx, window: u64, request: Request) {
        match request {
            Request::OpenChat { key } => self.open_chat(cx, window, key),
            Request::CloseChat { key } => self.close_chat(cx, window, &key),
            Request::LoadOlder { key } => {
                let Some(open) = self.chats.get_mut(&key) else {
                    return;
                };
                let Some(oldest) = open.oldest().filter(|_| {
                    open.load == Load::Ready && !open.paging_older && open.older == History::More
                }) else {
                    return;
                };
                open.paging_older = true;
                open.queue.push(conv::older(History::Loading));
                let epoch = open.epoch;
                history(cx, &key, epoch, Page::Older, oldest, 0);
            }
            Request::LoadNewer { key } => {
                let Some(open) = self.chats.get_mut(&key) else {
                    return;
                };
                let Some(newest) = open.newest().filter(|_| {
                    open.load == Load::Ready && !open.paging_newer && open.newer == History::More
                }) else {
                    return;
                };
                open.paging_newer = true;
                open.queue.push(conv::newer(History::Loading));
                let epoch = open.epoch;
                history(cx, &key, epoch, Page::Newer, newest, -(PAGE as i64));
            }
            Request::Visible { key, first, last } => self.visible(cx, &key, &first, &last),
            Request::Send { key, composed } => self.send(cx, &key, composed),
            Request::Edit { key, message, text } => self.edit(cx, &key, &message, &text),
            Request::Delete { key, message } => {
                let Some(open) = self.chats.get(&key) else {
                    return;
                };
                let Some(id) = message.parse::<i64>().ok() else {
                    return;
                };
                let revoke = open.shown.get(&id).is_some_and(|converted| {
                    converted
                        .message
                        .actions
                        .as_ref()
                        .is_some_and(|actions| actions.iter().any(|a| a == convert::DELETE))
                });
                // An album goes whole.
                let ids = open.items_of(id);
                cx.td.send(
                    &key.account,
                    json!({"@type": "deleteMessages", "chat_id": key.chat, "message_ids": ids,
                        "revoke": revoke}),
                    Asked::Feed(Ask::Done {
                        what: "deleteMessages",
                        told: Some("Couldn't delete the message"),
                    }),
                );
            }
            Request::Download { key, message, part } => {
                let file = self.chats.get(&key).and_then(|open| {
                    let id = message.parse::<i64>().ok()?;
                    open.shown
                        .get(&id)?
                        .part_files
                        .get(part as usize)
                        .copied()?
                });
                if let Some(file) = file {
                    // A click's download goes on wherever the message goes.
                    self.auto.remove(&(key.account.clone(), file));
                    download(cx, &mut self.downloading, &key.account, file, CLICKED, true);
                }
            }
            Request::JumpTo { key, message } => {
                let Some(open) = self.chats.get_mut(&key) else {
                    return;
                };
                let Some(id) = message.parse::<i64>().ok() else {
                    return;
                };
                // An album's item: the album.
                let shown = open.shown_id(id);
                if open.shown.contains_key(&shown) {
                    open.queue.push(conv::reveal(&shown.to_string()));
                    return;
                }
                self.epochs += 1;
                open.epoch = self.epochs;
                open.paging_older = false;
                open.paging_newer = false;
                let epoch = open.epoch;
                history(
                    cx,
                    &key,
                    epoch,
                    Page::Jump { target: id },
                    id,
                    -(PAGE as i64 / 2),
                );
            }
            Request::Draft { key, text } => self.draft(cx, &key, text),
            Request::Quote {
                key,
                message,
                quote,
            } => self.quote(&key, message, quote),
            _ => {}
        }
    }

    fn open_chat(&mut self, cx: &mut Cx, window: u64, key: ChatKey) {
        if let Some(open) = self.chats.get_mut(&key) {
            open.windows.insert(window);
            if open.load == Load::Ready {
                // Loaded for another window (or a tab reopened): what the others have, at once.
                let store = cx.stores.get(&key.account);
                let changes = open.snapshot(store, key.chat);
                let _ = service::send_json_to(
                    window,
                    &Note::Feed {
                        key: key.clone(),
                        changes,
                    },
                );
                if let Some(header) = &open.header {
                    let _ = service::send_json_to(
                        window,
                        &Note::Header {
                            key,
                            header: header.clone(),
                        },
                    );
                }
            }
            return;
        }
        self.chats.insert(key.clone(), Open::new(window));
        if self.ready.contains(&key.account) {
            self.load(cx, &key);
        }
    }

    fn close_chat(&mut self, cx: &mut Cx, window: u64, key: &ChatKey) {
        let Some(open) = self.chats.get_mut(key) else {
            return;
        };
        open.windows.remove(&window);
        if !open.windows.is_empty() {
            return;
        }
        // The draft typed last goes to Telegram before the chat is let go.
        if let Some((timer, text)) = open.draft.take() {
            timers::cancel(timer);
            self.draft_timers.remove(&timer);
            save_draft(cx, key, &text);
        }
        if self.ready.contains(&key.account) {
            // Its pictures aren't on any screen now.
            self.stop_downloads(cx, key, &HashSet::new());
            cx.td.send(
                &key.account,
                json!({"@type": "closeChat", "chat_id": key.chat}),
                Asked::Feed(Ask::Done {
                    what: "closeChat",
                    told: None,
                }),
            );
        }
        self.chats.remove(key);
    }

    /// The messages on screen: the unread ones are read (as Telegram's clients do — only what
    /// the user sees, in the active window), their pictures downloaded first.
    fn visible(&mut self, cx: &mut Cx, key: &ChatKey, first: &str, last: &str) {
        let (Ok(first), Ok(last)) = (first.parse::<i64>(), last.parse::<i64>()) else {
            return;
        };
        let (first, last) = (first.min(last), first.max(last));
        let Some(open) = self.chats.get_mut(key) else {
            return;
        };
        open.visible = Some((first, last));
        let open = &*open;
        let Some(store) = cx.stores.get(&key.account) else {
            return;
        };
        let read = store
            .chats
            .get(&key.chat)
            .map_or(i64::MAX, |chat| chat.last_read_inbox);
        // An album on screen: every item of it (the later ones are past `last`).
        let seen: BTreeSet<i64> = open
            .raw
            .range(first..=last)
            .flat_map(|(id, _)| open.items_of(*id))
            .collect();
        let unread: Vec<i64> = seen
            .into_iter()
            .filter(|id| {
                *id > read
                    && open
                        .raw
                        .get(id)
                        .is_some_and(|message| message["is_outgoing"] != true)
            })
            .collect();
        // The pictures on screen (the newest first), then the photos' full sizes.
        let shown: Vec<&Converted> = open
            .raw
            .range(first..=last)
            .rev()
            .filter_map(|(id, _)| open.shown.get(id))
            .collect();
        let wanted: Vec<(i64, i64)> = shown
            .iter()
            .flat_map(|converted| converted.auto.iter().map(|file| (*file, IN_SIGHT)))
            .chain(
                shown
                    .iter()
                    .flat_map(|converted| converted.full.iter().map(|file| (*file, FULL))),
            )
            .filter(|(file, _)| store.file_path(*file).is_none())
            .collect();
        if !unread.is_empty() {
            cx.td.send(
                &key.account,
                json!({"@type": "viewMessages", "chat_id": key.chat, "message_ids": unread,
                    "source": null, "force_read": true}),
                Asked::Feed(Ask::Done {
                    what: "viewMessages",
                    told: None,
                }),
            );
        }
        // What left the screen: the downloads the feed started for it stop.
        let on_screen: HashSet<i64> = wanted.iter().map(|(file, _)| *file).collect();
        self.stop_downloads(cx, key, &on_screen);
        for (file, priority) in wanted {
            self.auto.insert((key.account.clone(), file), key.chat);
            download(
                cx,
                &mut self.downloading,
                &key.account,
                file,
                priority,
                false,
            );
        }
    }

    /// Cancels the downloads the feed started for the chat's messages (not a click's), but
    /// those of `keep`; the finished ones are forgotten.
    fn stop_downloads(&mut self, cx: &mut Cx, key: &ChatKey, keep: &HashSet<i64>) {
        let store = cx.stores.get(&key.account);
        let done = |file: i64| store.is_some_and(|store| store.file_path(file).is_some());
        let mut stopped = Vec::new();
        self.auto.retain(|(account, file), chat| {
            if *account != key.account || *chat != key.chat || keep.contains(file) {
                return true;
            }
            if !done(*file) {
                stopped.push(*file);
            }
            false
        });
        for file in stopped {
            self.downloading.remove(&(key.account.clone(), file));
            cx.td.send(
                &key.account,
                json!({"@type": "cancelDownloadFile", "file_id": file, "only_if_pending": false}),
                Asked::Feed(Ask::Done {
                    what: "cancelDownloadFile",
                    told: None,
                }),
            );
        }
    }

    /// The field sent: the text, the files (pictures as photos — several as an album — the rest
    /// as files, the text as the first one's caption), the reply.
    fn send(&mut self, cx: &mut Cx, key: &ChatKey, composed: Composed) {
        let files = !composed.files.is_empty();
        let text = composed.text.trim().to_string();
        if text.is_empty() {
            self.send_formatted(cx, key, &composed, formatted_text(""));
        } else {
            // The markup first, as Telegram's apps send it: the rest when TDLib parsed it.
            cx.td.send(
                &key.account,
                json!({"@type": "parseMarkdown", "text": formatted_text(&text)}),
                Asked::Feed(Ask::Markdown {
                    chat: key.chat,
                    text,
                    then: Then::Send(Box::new(composed)),
                }),
            );
        }
        // The draft is spent: its timer goes, and Telegram's draft with it.
        if let Some(open) = self.chats.get_mut(key) {
            open.typed_at = None;
            if let Some((timer, _)) = open.draft.take() {
                timers::cancel(timer);
                self.draft_timers.remove(&timer);
            }
        }
        let has_draft = cx
            .stores
            .get(&key.account)
            .and_then(|store| store.chats.get(&key.chat))
            .is_some_and(|chat| chat.draft.is_some());
        if has_draft && files {
            save_draft(cx, key, "");
        }
    }

    /// The reply over the chat's field quotes a part of its message (`quote`: the text, its
    /// offset in the message's text in UTF-16 units), or the whole message again (none): the
    /// next message sent in reply to `message` carries it.
    pub fn quote(&mut self, key: &ChatKey, message: String, quote: Option<(String, u32)>) {
        match quote {
            Some((text, offset)) => {
                self.quotes.insert(key.clone(), (message, text, offset));
            }
            None => {
                self.quotes.remove(key);
            }
        }
    }

    /// The field's text parsed (`text`: TDLib's `formattedText`, empty when there is none) goes:
    /// as a message, or as the caption of the first file.
    fn send_formatted(&mut self, cx: &mut Cx, key: &ChatKey, composed: &Composed, text: Value) {
        let quote = self.quotes.remove(key);
        let reply_to = composed
            .reply_to
            .as_deref()
            .and_then(|id| Some((id, id.parse::<i64>().ok()?)))
            .map(|(id, message)| {
                // The part the reply quotes, when it quotes the message it answers.
                let quote = quote
                    .filter(|(quoted, _, _)| quoted == id)
                    .map(|(_, text, offset)| {
                        json!({"@type": "inputTextQuote", "text": formatted_text(&text),
                            "position": offset})
                    });
                json!({"@type": "inputMessageReplyToMessage", "message_id": message,
                    "quote": quote})
            });
        let mut contents: Vec<(bool, Value)> = Vec::new();
        let has_text = text["text"].as_str().is_some_and(|text| !text.is_empty());
        let mut caption = has_text.then_some(text);
        for path in &composed.files {
            let extension = path
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let picture = PICTURES.contains(&extension.as_str());
            let file = json!({"@type": "inputFileLocal", "path": path});
            let content = if picture {
                json!({"@type": "inputMessagePhoto", "photo": file, "thumbnail": null,
                    "added_sticker_file_ids": [], "width": 0, "height": 0,
                    "caption": caption.take(), "show_caption_above_media": false,
                    "self_destruct_type": null, "has_spoiler": false})
            } else {
                json!({"@type": "inputMessageDocument", "document": file, "thumbnail": null,
                    "disable_content_type_detection": false, "caption": caption.take()})
            };
            contents.push((picture, content));
        }
        if contents.is_empty() {
            let Some(text) = caption.take() else {
                return;
            };
            contents.push((
                false,
                json!({"@type": "inputMessageText", "text": text, "link_preview_options": null,
                    "clear_draft": true}),
            ));
        }
        // Photos and files can't share an album: each kind in albums of ten.
        let mut batches: Vec<Vec<Value>> = Vec::new();
        let mut last_kind = None;
        for (picture, content) in contents {
            match batches.last_mut() {
                Some(batch) if last_kind == Some(picture) && batch.len() < ALBUM => {
                    batch.push(content)
                }
                _ => batches.push(vec![content]),
            }
            last_kind = Some(picture);
        }
        let mut reply_to = reply_to;
        for batch in batches {
            let reply = reply_to.take();
            let request = if batch.len() == 1 {
                json!({"@type": "sendMessage", "chat_id": key.chat, "message_thread_id": 0,
                    "reply_to": reply, "options": null, "reply_markup": null,
                    "input_message_content": batch[0]})
            } else {
                json!({"@type": "sendMessageAlbum", "chat_id": key.chat, "message_thread_id": 0,
                    "reply_to": reply, "options": null, "input_message_contents": batch})
            };
            cx.td.send(
                &key.account,
                request,
                Asked::Feed(Ask::Sent { chat: key.chat }),
            );
        }
    }

    /// A message's new text: `editMessageText`, or the caption of a photo or a file.
    fn edit(&mut self, cx: &mut Cx, key: &ChatKey, message: &str, text: &str) {
        let Some(open) = self.chats.get(key) else {
            return;
        };
        let Some(id) = message.parse::<i64>().ok() else {
            return;
        };
        // An album's caption: in the item that has it (else the first).
        let id = open.captioned(id);
        if !open.raw.contains_key(&id) {
            return;
        }
        // The field has the text as Markdown: parsed back, its bold and links stay.
        let text = text.trim().to_string();
        cx.td.send(
            &key.account,
            json!({"@type": "parseMarkdown", "text": formatted_text(&text)}),
            Asked::Feed(Ask::Markdown {
                chat: key.chat,
                text,
                then: Then::Edit { message: id },
            }),
        );
    }

    /// The edit parsed (`text`: TDLib's `formattedText`): `editMessageText`, or the caption of a
    /// photo or a file.
    fn edit_formatted(&mut self, cx: &mut Cx, key: &ChatKey, id: i64, text: Value) {
        let Some(raw) = self.chats.get(key).and_then(|open| open.raw.get(&id)) else {
            return;
        };
        let request = if raw["content"]["@type"] == "messageText" {
            json!({"@type": "editMessageText", "chat_id": key.chat, "message_id": id,
                "reply_markup": null, "input_message_content": {"@type": "inputMessageText",
                "text": text, "link_preview_options": null, "clear_draft": false}})
        } else {
            json!({"@type": "editMessageCaption", "chat_id": key.chat, "message_id": id,
                "reply_markup": null, "caption": text, "show_caption_above_media":
                raw["content"]["show_caption_above_media"] == true})
        };
        cx.td.send(
            &key.account,
            request,
            Asked::Feed(Ask::Done {
                what: "editMessage",
                told: Some("Couldn't edit the message"),
            }),
        );
    }

    /// The field's text changed: "typing…" to the chat (at most every 5 s), and the draft to
    /// Telegram once the field rests.
    fn draft(&mut self, cx: &mut Cx, key: &ChatKey, text: String) {
        let Some(open) = self.chats.get_mut(key) else {
            return;
        };
        let saved = cx
            .stores
            .get(&key.account)
            .and_then(|store| store.chats.get(&key.chat))
            .and_then(|chat| chat.draft.clone())
            .unwrap_or_default();
        if let Some((timer, _)) = open.draft.take() {
            timers::cancel(timer);
            self.draft_timers.remove(&timer);
        }
        // The field took the saved draft (a tab opened): nothing was typed.
        if text == saved {
            return;
        }
        let typing = !text.trim().is_empty();
        let due = open.typed_at.is_none_or(|at| at.elapsed() >= TYPING_EVERY);
        if typing && due {
            open.typed_at = Some(Instant::now());
            send_action(cx, key, "chatActionTyping");
        } else if !typing && open.typed_at.take().is_some() {
            send_action(cx, key, "chatActionCancel");
        }
        let timer = timers::after(DRAFT_AFTER);
        open.draft = Some((timer, text));
        self.draft_timers.insert(timer, key.clone());
    }

    /// A timer of the feed fired (one it started).
    pub fn timer(&mut self, cx: &mut Cx, timer: u64) {
        if self.typing_timer == Some(timer) {
            self.typing_timer = None;
            for open in self.chats.values_mut() {
                let before = open.typing.len();
                open.typing.retain(|_, (_, at)| at.elapsed() < TYPING_FOR);
                if open.typing.len() != before {
                    open.header_dirty = true;
                }
            }
            self.schedule_typing();
            return;
        }
        if let Some(key) = self.draft_timers.remove(&timer) {
            let Some(open) = self.chats.get_mut(&key) else {
                return;
            };
            if let Some((_, text)) = open.draft.take() {
                save_draft(cx, &key, &text);
            }
        }
    }

    /// The typing's expiry: one timer, at the earliest "typing…" to go — none while nobody types.
    fn schedule_typing(&mut self) {
        let earliest = self
            .chats
            .values()
            .flat_map(|open| open.typing.values())
            .map(|(_, at)| *at)
            .min();
        if let Some(timer) = self.typing_timer.take() {
            timers::cancel(timer);
        }
        if let Some(at) = earliest {
            let left = TYPING_FOR.saturating_sub(at.elapsed()) + Duration::from_millis(50);
            self.typing_timer = Some(timers::after(left));
        }
    }

    /// A window's instance stopped: its open chats close there.
    pub fn window_closed(&mut self, cx: &mut Cx, window: u64) {
        let keys: Vec<ChatKey> = self
            .chats
            .iter()
            .filter(|(_, open)| open.windows.contains(&window))
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            self.close_chat(cx, window, &key);
        }
    }

    /// The end of a batch: the feeds' and headers' changes go to the windows that show them.
    pub fn flush(&mut self, cx: &mut Cx) {
        let now = unix_now() as i64;
        let offset = system::utc_offset(now);
        for (key, open) in self.chats.iter_mut() {
            if open.load != Load::Ready {
                continue;
            }
            let mut changes = std::mem::take(&mut open.queue);
            if !open.dirty.is_empty() || !open.dirty_albums.is_empty() {
                let ids: Vec<i64> = std::mem::take(&mut open.dirty).into_iter().collect();
                let (removed, changed) = convert_changed(cx, key, open, &ids);
                if !removed.is_empty() {
                    changes.push(conv::remove(removed));
                }
                if !changed.is_empty() {
                    changes.push(conv::put(changed));
                }
            }
            if !changes.is_empty() {
                for window in &open.windows {
                    let _ = service::send_json_to(
                        *window,
                        &Note::Feed {
                            key: key.clone(),
                            changes: changes.clone(),
                        },
                    );
                }
            }
            if !open.header_dirty {
                continue;
            }
            open.header_dirty = false;
            let Some(store) = cx.stores.get(&key.account) else {
                continue;
            };
            let typing: Vec<(i64, &str)> = open
                .typing
                .iter()
                .map(|(who, (action, _))| (*who, action.as_str()))
                .collect();
            let header = header(store, key.chat, &typing, now, offset, &HostWords);
            if open.header.as_ref() == Some(&header) {
                continue;
            }
            open.header = Some(header.clone());
            for window in &open.windows {
                let _ = service::send_json_to(
                    *window,
                    &Note::Header {
                        key: key.clone(),
                        header: header.clone(),
                    },
                );
            }
        }
        // What the new messages name and quote: asked once, put again when it comes.
        self.ask_missing(cx);
    }

    /// The users, chats and replied messages the converted messages lack.
    fn ask_missing(&mut self, cx: &mut Cx) {
        for (key, open) in self.chats.iter_mut() {
            for (file, priority) in std::mem::take(&mut open.wanted_files) {
                self.auto.insert((key.account.clone(), file), key.chat);
                download(
                    cx,
                    &mut self.downloading,
                    &key.account,
                    file,
                    priority,
                    false,
                );
            }
            let users = std::mem::take(&mut open.wanted_users);
            let chats = std::mem::take(&mut open.wanted_chats);
            let replies = std::mem::take(&mut open.wanted_replies);
            for user in users {
                if self.asked.insert((key.account.clone(), 1, user)) {
                    cx.td.send(
                        &key.account,
                        json!({"@type": "getUser", "user_id": user}),
                        Asked::Feed(Ask::Named),
                    );
                }
            }
            for chat in chats {
                if self.asked.insert((key.account.clone(), 2, chat)) {
                    cx.td.send(
                        &key.account,
                        json!({"@type": "getChat", "chat_id": chat}),
                        Asked::Feed(Ask::Named),
                    );
                }
            }
            for target in replies {
                if open.replied.contains_key(&target) || open.raw.contains_key(&target) {
                    continue;
                }
                if self.asked.insert((key.account.clone(), key.chat, target)) {
                    cx.td.send(
                        &key.account,
                        json!({"@type": "getMessage", "chat_id": key.chat, "message_id": target}),
                        Asked::Feed(Ask::Replied { chat: key.chat }),
                    );
                }
            }
        }
    }
}

/// The first page of an open chat whose chat the store knows: around the last read message when
/// some are unread (they are below it, the line above them), else the newest; and what the header
/// needs and the store may lack — the person, the group, the picture.
fn first_page(cx: &mut Cx, key: &ChatKey, open: &Open) {
    let account = &key.account;
    let chat = key.chat;
    let Some(store) = cx.stores.get(account) else {
        return;
    };
    let known = store.chats.get(&chat);
    let mut asks = Vec::new();
    match known.map(|chat| chat.kind) {
        Some(ChatKind::Private { user }) if !store.users.contains_key(&user) => {
            asks.push(json!({"@type": "getUser", "user_id": user}));
        }
        Some(ChatKind::BasicGroup { group }) if !store.basic_groups.contains_key(&group) => {
            asks.push(json!({"@type": "getBasicGroup", "basic_group_id": group}));
        }
        Some(ChatKind::Supergroup { group, .. }) if !store.supergroups.contains_key(&group) => {
            asks.push(json!({"@type": "getSupergroup", "supergroup_id": group}));
        }
        _ => {}
    }
    let photo = store.chat_photo_missing(chat);
    let around = known
        .filter(|chat| chat.unread_count > 0 && chat.last_read_inbox > 0)
        .map(|chat| chat.last_read_inbox);
    for ask in asks {
        cx.td.send(account, ask, Asked::Feed(Ask::Named));
    }
    if let Some(photo) = photo {
        cx.td.send(
            account,
            json!({"@type": "downloadFile", "file_id": photo, "priority": IN_SIGHT, "offset": 0,
                "limit": 0, "synchronous": false}),
            Asked::Feed(Ask::Done {
                what: "downloadFile",
                told: None,
            }),
        );
    }
    history(
        cx,
        key,
        open.epoch,
        Page::First { around, tries: 1 },
        around.unwrap_or(0),
        around.map_or(0, |_| -(PAGE as i64 / 2)),
    );
}

/// `getChatHistory` of an open chat.
fn history(cx: &mut Cx, key: &ChatKey, epoch: u64, page: Page, from: i64, offset: i64) {
    cx.td.send(
        &key.account,
        json!({"@type": "getChatHistory", "chat_id": key.chat, "from_message_id": from,
            "offset": offset, "limit": PAGE, "only_local": false}),
        Asked::Feed(Ask::History {
            chat: key.chat,
            epoch,
            page,
        }),
    );
}

/// A page more: taken, converted and queued for the windows.
fn page_in(cx: &mut Cx, key: &ChatKey, open: &mut Open, messages: Vec<Value>) {
    let ids: Vec<i64> = messages.iter().filter_map(|m| m["id"].as_i64()).collect();
    for message in messages {
        open.take(message);
    }
    for id in &ids {
        open.dirty.remove(id);
    }
    let (removed, converted) = convert_all(cx, key, open, &ids);
    if !removed.is_empty() {
        open.queue.push(conv::remove(removed));
    }
    if !converted.is_empty() {
        open.queue.push(conv::put(converted));
    }
}

/// What a conversion changes for the windows: the ids that went (an album shown under another
/// id now, or held back), the messages to put.
type Converts = (Vec<String>, Vec<flux_plugin_api::conversation::Message>);

/// Converts the messages for the windows, and remembers what each shows.
fn convert_all(cx: &Cx, key: &ChatKey, open: &mut Open, ids: &[i64]) -> Converts {
    match cx.stores.get(&key.account) {
        Some(store) => open.convert(store, &HostWords, unix_now() as i64, ids, false),
        None => Converts::default(),
    }
}

/// Converts the messages again: only those that came out different are put.
fn convert_changed(cx: &Cx, key: &ChatKey, open: &mut Open, ids: &[i64]) -> Converts {
    match cx.stores.get(&key.account) {
        Some(store) => open.convert(store, &HostWords, unix_now() as i64, ids, true),
        None => Converts::default(),
    }
}

impl Open {
    /// Converts the messages (and the albums that lost items): a message by itself, an album's
    /// items together as one message under its first item's id — when that id changed (an older
    /// item came, the first went or was sent), the old one goes.
    fn convert(
        &mut self,
        store: &Store,
        words: &dyn Words,
        now: i64,
        ids: &[i64],
        only_changed: bool,
    ) -> Converts {
        let mut albums = std::mem::take(&mut self.dirty_albums);
        let mut singles = Vec::new();
        for id in ids {
            match self.album_of.get(id) {
                Some(album) => {
                    albums.insert(*album);
                }
                None => singles.push(*id),
            }
        }
        let mut removed = Vec::new();
        let mut messages = Vec::new();
        for id in singles {
            let Some(raw) = self.raw.get(&id) else {
                continue;
            };
            let context = Context {
                store,
                words,
                now,
                replied: self.replied_to(raw),
            };
            if let Some(converted) = convert::convert(&context, raw) {
                self.keep(store, id, converted, only_changed, &mut messages);
            }
        }
        for album in albums {
            let items: Vec<i64> = self
                .albums
                .get(&album)
                .map(|items| items.iter().copied().collect())
                .unwrap_or_default();
            let shown_as = items.first().copied().filter(|_| !self.held(album, &items));
            if let Some(before) = self.album_shown.get(&album).copied()
                && Some(before) != shown_as
            {
                self.album_shown.remove(&album);
                if self.shown.remove(&before).is_some() {
                    removed.push(before.to_string());
                }
            }
            let Some(id) = shown_as else {
                continue;
            };
            let raws: Vec<&Value> = items.iter().filter_map(|item| self.raw.get(item)).collect();
            let context = Context {
                store,
                words,
                now,
                replied: raws.first().and_then(|first| self.replied_to(first)),
            };
            if let Some(converted) = convert::album(&context, &raws) {
                self.keep(store, id, converted, only_changed, &mut messages);
                self.album_shown.insert(album, id);
            }
        }
        messages.sort_by_key(|message| message.order);
        (removed, messages)
    }

    /// The message a loaded one answers, when the feed has it.
    fn replied_to(&self, raw: &Value) -> Option<&Value> {
        let target = raw["reply_to"]["message_id"].as_i64()?;
        self.raw.get(&target).or_else(|| self.replied.get(&target))
    }

    /// A message converted, shown under `id`: what it shows is followed, and it is put when it
    /// came out different (`only_changed`) or always.
    fn keep(
        &mut self,
        store: &Store,
        id: i64,
        converted: Converted,
        only_changed: bool,
        messages: &mut Vec<flux_plugin_api::conversation::Message>,
    ) {
        for file in &converted.files {
            self.files.entry(*file).or_default().insert(id);
        }
        for user in &converted.missing_users {
            self.named.entry(*user).or_default().insert(id);
            self.wanted_users.push(*user);
        }
        for chat in &converted.missing_chats {
            self.named.entry(*chat).or_default().insert(id);
            self.wanted_chats.push(*chat);
        }
        if let Some(target) = converted.reply_to {
            self.wanted_replies.push(target);
        }
        if self
            .visible
            .is_some_and(|(first, last)| (first..=last).contains(&id))
        {
            let missing = (converted.auto.iter().map(|file| (*file, IN_SIGHT)))
                .chain(converted.full.iter().map(|file| (*file, FULL)))
                .filter(|(file, _)| store.file_path(*file).is_none());
            self.wanted_files.extend(missing);
        }
        let same = self
            .shown
            .get(&id)
            .is_some_and(|shown| shown.message == converted.message);
        if !(only_changed && same) {
            messages.push(converted.message.clone());
        }
        self.shown.insert(id, converted);
    }
}

/// Downloads a file once (`clicked` — again, at the top priority).
fn download(
    cx: &mut Cx,
    asked: &mut HashSet<(String, i64)>,
    account: &str,
    file: i64,
    priority: i64,
    clicked: bool,
) {
    let there = cx
        .stores
        .get(account)
        .is_some_and(|store| store.file_path(file).is_some());
    if there || (!asked.insert((account.to_string(), file)) && !clicked) {
        return;
    }
    cx.td.send(
        account,
        json!({"@type": "downloadFile", "file_id": file, "priority": priority, "offset": 0,
            "limit": 0, "synchronous": false}),
        Asked::Feed(Ask::Done {
            what: "downloadFile",
            told: clicked.then_some("Couldn't download the file"),
        }),
    );
}

fn send_action(cx: &mut Cx, key: &ChatKey, action: &str) {
    cx.td.send(
        &key.account,
        json!({"@type": "sendChatAction", "chat_id": key.chat, "message_thread_id": 0,
            "action": {"@type": action}}),
        Asked::Feed(Ask::Done {
            what: "sendChatAction",
            told: None,
        }),
    );
}

/// The draft to Telegram (an empty text — none).
fn save_draft(cx: &mut Cx, key: &ChatKey, text: &str) {
    let saved = cx
        .stores
        .get(&key.account)
        .and_then(|store| store.chats.get(&key.chat))
        .and_then(|chat| chat.draft.clone())
        .unwrap_or_default();
    if saved == text.trim_end() {
        return;
    }
    let draft = (!text.trim().is_empty()).then(|| {
        json!({"@type": "draftMessage", "reply_to": null, "date": unix_now(),
            "input_message_text": {"@type": "inputMessageText", "text": formatted_text(text),
            "link_preview_options": null, "clear_draft": false}})
    });
    cx.td.send(
        &key.account,
        json!({"@type": "setChatDraftMessage", "chat_id": key.chat, "message_thread_id": 0,
            "draft_message": draft}),
        Asked::Feed(Ask::Done {
            what: "setChatDraftMessage",
            told: None,
        }),
    );
}

/// Plain text as TDLib's `formattedText`: TDLib finds the links itself.
fn formatted_text(text: &str) -> Value {
    json!({"@type": "formattedText", "text": text, "entities": []})
}

// --- The header ---

/// The chat's header: the title, the picture, the status line, who is typing.
pub fn header(
    store: &Store,
    chat: i64,
    typing: &[(i64, &str)],
    now: i64,
    offset: i32,
    words: &dyn Words,
) -> ChatHeader {
    let found = store.chats.get(&chat);
    let saved = store.is_saved_messages(chat);
    let user = match found.map(|chat| chat.kind) {
        Some(ChatKind::Private { user }) => store.users.get(&user),
        _ => None,
    };
    let title = if saved {
        words.tr("Saved Messages")
    } else if user.is_some_and(|user| user.kind == UserKind::Deleted) {
        words.tr("Deleted Account")
    } else {
        found.map(|chat| chat.title.clone()).unwrap_or_default()
    };
    let (status, online) = if saved {
        (String::new(), false)
    } else {
        status_line(store, chat, now, offset, words)
    };
    ChatHeader {
        title,
        avatar: store.chat_avatar(chat),
        status,
        online,
        typing: typing_line(store, chat, typing, words),
        can_send: store.can_send(chat),
        muted: store.muted(chat),
    }
}

/// The line under a chat's title: a person's "online" / "last seen…" (and whether it is online),
/// a group's members.
pub fn status_line(
    store: &Store,
    chat: i64,
    now: i64,
    offset: i32,
    words: &dyn Words,
) -> (String, bool) {
    let Some(found) = store.chats.get(&chat) else {
        return (String::new(), false);
    };
    let user = match found.kind {
        ChatKind::Private { user } | ChatKind::Secret { user } => user,
        ChatKind::BasicGroup { .. } | ChatKind::Supergroup { .. } => {
            let members = store.group(chat).map_or(0, |group| group.member_count);
            let line = if members == 0 {
                words.tr("group")
            } else {
                words.count(u64::from(members), "{0} member", "{0} members")
            };
            return (line, false);
        }
        ChatKind::Unknown => return (String::new(), false),
    };
    let Some(user) = store.users.get(&user) else {
        return (String::new(), false);
    };
    match user.kind {
        UserKind::Bot => return (words.tr("bot"), false),
        UserKind::Deleted => return (String::new(), false),
        _ => {}
    }
    let line = match user.status {
        Status::Online { expires } if expires > now => return (words.tr("online"), true),
        Status::Online { expires } => last_seen(expires, now, offset, words),
        Status::Offline { was } => last_seen(was, now, offset, words),
        Status::Recently => words.tr("last seen recently"),
        Status::LastWeek => words.tr("last seen within a week"),
        Status::LastMonth => words.tr("last seen within a month"),
        Status::Empty => words.tr("last seen a long time ago"),
    };
    (line, false)
}

/// "last seen just now", "… 5 minutes ago", "… at 14:32", "… yesterday at 14:32", "… 12.11.26".
fn last_seen(was: i64, now: i64, offset: i32, words: &dyn Words) -> String {
    let ago = now - was;
    if ago < 60 {
        return words.tr("last seen just now");
    }
    if ago < 3600 {
        return words.count(
            (ago / 60) as u64,
            "last seen {0} minute ago",
            "last seen {0} minutes ago",
        );
    }
    let then = LocalTime::at(was, offset);
    let day = |at: i64| (at + i64::from(offset)).div_euclid(86_400);
    let time = format!("{}:{:02}", then.hour, then.minute);
    match day(now) - day(was) {
        0 => words.trf("last seen at {0}", &[&time]),
        1 => words.trf("last seen yesterday at {0}", &[&time]),
        _ => {
            let date = format!("{:02}.{:02}.{:02}", then.day, then.month, then.year % 100);
            words.trf("last seen {0}", &[&date])
        }
    }
}

/// Who is typing, in the header's place of the status: "typing…" in a person's chat, "Petya is
/// typing…", "Petya and Anna are typing…", "Petya and 2 others are typing…" in a group.
pub fn typing_line(
    store: &Store,
    chat: i64,
    typing: &[(i64, &str)],
    words: &dyn Words,
) -> Option<String> {
    let (first, action) = typing.first()?;
    let private = matches!(
        store.chats.get(&chat).map(|chat| chat.kind),
        Some(ChatKind::Private { .. })
    );
    let what = |action: &str| match action {
        "chatActionRecordingVoiceNote" | "chatActionUploadingVoiceNote" => (
            "recording a voice message…",
            "{0} is recording a voice message…",
        ),
        "chatActionRecordingVideoNote" | "chatActionUploadingVideoNote" => (
            "recording a video message…",
            "{0} is recording a video message…",
        ),
        "chatActionUploadingPhoto" => ("sending a photo…", "{0} is sending a photo…"),
        "chatActionRecordingVideo" | "chatActionUploadingVideo" => {
            ("sending a video…", "{0} is sending a video…")
        }
        "chatActionUploadingDocument" => ("sending a file…", "{0} is sending a file…"),
        "chatActionChoosingSticker" => ("choosing a sticker…", "{0} is choosing a sticker…"),
        _ => ("typing…", "{0} is typing…"),
    };
    if private {
        return Some(words.tr(what(action).0));
    }
    let name = |who: i64| {
        let message = json!({"sender_id": if who > 0 {
            json!({"@type": "messageSenderUser", "user_id": who})
        } else {
            json!({"@type": "messageSenderChat", "chat_id": who})
        }});
        summary::sender(store, &message, words)
            .first_name()
            .to_string()
    };
    Some(match typing {
        [_] => words.trf(what(action).1, &[&name(*first)]),
        [_, (second, _)] => words.trf("{0} and {1} are typing…", &[&name(*first), &name(*second)]),
        _ => words
            .count(
                (typing.len() - 1) as u64,
                "{1} and {0} other are typing…",
                "{1} and {0} others are typing…",
            )
            .replace("{1}", &name(*first)),
    })
}

impl Feed {
    /// A chat's search (`Request::Search`): a page of its matches, to the window that asked. The
    /// window jumps to them itself (`JumpTo` for one that isn't loaded) and marks the words.
    pub fn search(&mut self, cx: &mut Cx, window: u64, ask: SearchAsk) {
        match ask {
            SearchAsk::Find {
                key,
                query,
                request,
                from,
            } => {
                let query = query.trim();
                if query.is_empty() {
                    return;
                }
                // TDLib 1.8.61: the whole chat (`topic_id` null), anyone's, every kind.
                cx.td.send(
                    &key.account,
                    json!({"@type": "searchChatMessages", "chat_id": key.chat, "topic_id": null,
                        "query": query, "sender_id": null, "from_message_id": from, "offset": 0,
                        "limit": SEARCH_PAGE, "filter": null}),
                    Asked::Feed(Ask::Search {
                        window,
                        chat: key.chat,
                        request,
                        from,
                    }),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::English;

    const NOW: i64 = 1_763_380_000;

    fn store() -> Store {
        let mut store = Store {
            me: Some(777),
            ..Store::default()
        };
        for user in [
            json!({"@type": "user", "id": 101, "first_name": "Anna", "last_name": "Smirnova",
                "status": {"@type": "userStatusOnline", "expires": NOW + 300}}),
            json!({"@type": "user", "id": 102, "first_name": "Dima", "last_name": "Kim",
                "status": {"@type": "userStatusOffline", "was_online": NOW - 1800}}),
            json!({"@type": "user", "id": 103, "first_name": "Petya", "last_name": "Smirnov"}),
            json!({"@type": "user", "id": 105, "first_name": "Weather",
                "type": {"@type": "userTypeBot"}}),
        ] {
            store.apply(&user);
        }
        for (id, user) in [(101, 101), (102, 102), (103, 103), (105, 105), (777, 777)] {
            store.apply(
                &json!({"@type": "chat", "id": id, "title": format!("chat {id}"),
                "type": {"@type": "chatTypePrivate", "user_id": user}}),
            );
        }
        store.apply(
            &json!({"@type": "chat", "id": -1000000001001i64, "title": "Backend",
            "type": {"@type": "chatTypeSupergroup", "supergroup_id": 1001, "is_channel": false},
            "permissions": {"can_send_basic_messages": true}}),
        );
        store.apply(
            &json!({"@type": "supergroup", "id": 1001, "member_count": 5,
            "status": {"@type": "chatMemberStatusMember"}}),
        );
        store
    }

    #[test]
    fn tells_statuses() {
        let store = store();
        let status = |chat, now| status_line(&store, chat, now, 3 * 3600, &English);
        assert_eq!(status(101, NOW), ("online".to_string(), true));
        assert_eq!(status(102, NOW).0, "last seen 30 minutes ago");
        // 1_763_380_000 is 11:46:40 UTC, 14:46 at +3.
        assert_eq!(status(102, NOW + 3 * 3600).0, "last seen at 14:16");
        assert_eq!(status(102, NOW + 86_400).0, "last seen yesterday at 14:16");
        assert_eq!(status(102, NOW + 5 * 86_400).0, "last seen 17.11.25");
        assert_eq!(status(103, NOW).0, "last seen a long time ago");
        assert_eq!(status(105, NOW).0, "bot");
        assert_eq!(status(-1000000001001, NOW).0, "5 members");
        // Online that has expired: when it was.
        assert_eq!(status(101, NOW + 330).0, "last seen just now");
        let saved = header(&store, 777, &[], NOW, 0, &English);
        assert_eq!(
            (saved.title.as_str(), saved.status.as_str()),
            ("Saved Messages", "")
        );
    }

    const GROUP: i64 = -1000000001001;

    /// An album's photo in the group (its file: the id × 10), by Anna or the user (`mine`).
    fn item(id: i64, album: &str, caption: &str, mine: bool) -> Value {
        json!({"@type": "message", "id": id, "chat_id": GROUP, "date": NOW,
            "sender_id": {"@type": "messageSenderUser", "user_id": if mine { 777 } else { 101 }},
            "is_outgoing": mine, "media_album_id": album,
            "content": {"@type": "messagePhoto", "caption": {"text": caption, "entities": []},
                "photo": {"sizes": [{"type": "x", "width": 800, "height": 600,
                "photo": {"@type": "file", "id": id * 10, "size": 10, "local": {"path": ""}}}]}}})
    }

    fn line(id: i64) -> Value {
        json!({"@type": "message", "id": id, "chat_id": GROUP, "date": NOW,
            "sender_id": {"@type": "messageSenderUser", "user_id": 101}, "is_outgoing": false,
            "content": {"@type": "messageText", "text": {"text": "Hi", "entities": []}}})
    }

    /// The end of a batch, as [`Feed::flush`] has it: the ids removed, the messages put (id,
    /// pictures).
    fn flushed(open: &mut Open, store: &Store) -> (Vec<String>, Vec<(String, usize)>) {
        let mut removed = Vec::new();
        for change in std::mem::take(&mut open.queue) {
            if let Change::Remove(ids) = change {
                removed.extend(ids);
            }
        }
        let ids: Vec<i64> = std::mem::take(&mut open.dirty).into_iter().collect();
        let (gone, put) = open.convert(store, &English, NOW, &ids, true);
        removed.extend(gone);
        let put = put
            .into_iter()
            .map(|message| {
                let pictures = message
                    .parts
                    .iter()
                    .filter(|part| matches!(part, conv::Part::Image(_)))
                    .count();
                (message.id, pictures)
            })
            .collect();
        (removed, put)
    }

    fn loaded() -> Open {
        let mut open = Open::new(1);
        open.load = Load::Ready;
        open.older = History::Complete;
        open
    }

    #[test]
    fn shows_an_album_as_one_message() {
        let store = store();
        let mut open = loaded();
        open.take(line(5));
        // TDLib's items come one by one (here in one batch); the caption is on the last one.
        for id in [10, 11, 12] {
            open.take(item(
                id,
                "77",
                if id == 12 { "Moodboard" } else { "" },
                false,
            ));
        }
        open.take(line(20));
        let (removed, put) = flushed(&mut open, &store);
        assert!(removed.is_empty());
        assert_eq!(
            put,
            [("5".into(), 0), ("10".into(), 3), ("20".into(), 0)],
            "one message for the album, under its first item's id"
        );
        let album = &open.shown[&10];
        assert_eq!(album.part_files, [Some(100), Some(110), Some(120), None]);
        assert_eq!(open.items_of(11), [10, 11, 12]);
        assert_eq!(open.shown_id(12), 10);
        assert_eq!(open.items_of(5), [5]);
        // Edit goes to the captioned item; Delete takes them all.
        assert_eq!(open.captioned(10), 12);
        // Nothing changed: nothing put again.
        open.dirty.insert(11);
        assert_eq!(flushed(&mut open, &store), (vec![], vec![]));
    }

    #[test]
    fn puts_an_album_again_as_its_items_come_and_go() {
        let store = store();
        let mut open = loaded();
        open.take(item(10, "77", "", false));
        open.take(item(11, "77", "", false));
        assert_eq!(flushed(&mut open, &store).1, [("10".into(), 2)]);
        // An item in a later batch: the same message, grown.
        open.take(item(12, "77", "", false));
        assert_eq!(flushed(&mut open, &store), (vec![], vec![("10".into(), 3)]));
        // An older item: the album moves to its id.
        open.take(item(9, "77", "", false));
        assert_eq!(
            flushed(&mut open, &store),
            (vec!["10".into()], vec![("9".into(), 4)])
        );
        // Deleted: the first one — under the next id; another — the same id; the last — gone.
        open.drop_messages(&[9]);
        assert_eq!(
            flushed(&mut open, &store),
            (vec!["9".into()], vec![("10".into(), 3)])
        );
        open.drop_messages(&[12, 11]);
        assert_eq!(flushed(&mut open, &store), (vec![], vec![("10".into(), 1)]));
        open.drop_messages(&[10]);
        assert_eq!(flushed(&mut open, &store), (vec!["10".into()], vec![]));
        assert!(open.shown.is_empty() && open.albums.is_empty());
    }

    #[test]
    fn holds_a_half_album_at_a_page_edge() {
        let store = store();
        let mut open = loaded();
        open.older = History::More;
        // The first fill: its oldest messages are an album's last items.
        for message in [
            item(11, "77", "", false),
            item(12, "77", "", false),
            line(20),
        ] {
            open.insert(message);
        }
        let ids: Vec<i64> = open.raw.keys().copied().collect();
        let (removed, put) = open.convert(&store, &English, NOW, &ids, false);
        assert!(removed.is_empty());
        assert_eq!(put.len(), 1, "the half album waits: {put:?}");
        // The older page brings the rest: one message, under the first item.
        for message in [line(5), item(10, "77", "", false)] {
            open.take(message);
        }
        open.dirty.clear();
        let (removed, put) = open.convert(&store, &English, NOW, &[5, 10], false);
        assert!(removed.is_empty());
        let put: Vec<(&str, usize)> = put
            .iter()
            .map(|message| (message.id.as_str(), message.parts.len()))
            .collect();
        assert_eq!(put, [("5", 1), ("10", 3)]);
        // An album at the start of the history is shown as it is once the start is known.
        let mut open = loaded();
        open.older = History::More;
        open.insert(item(11, "78", "", false));
        open.insert(line(20));
        open.convert(&store, &English, NOW, &[11, 20], false);
        assert!(!open.shown.contains_key(&11));
        open.older = History::Complete;
        open.dirty.insert(11);
        assert_eq!(flushed(&mut open, &store).1, [("11".into(), 1)]);
    }

    #[test]
    fn keeps_a_sent_album_one_message() {
        let store = store();
        let mut open = loaded();
        // `sendMessageAlbum`: pending items with temporary ids and TDLib's local album id.
        let pending = |id: i64| {
            let mut message = item(id, "-5", if id == 1001 { "Ours" } else { "" }, true);
            message["sending_state"] = json!({"@type": "messageSendingStatePending"});
            message
        };
        for id in [1001, 1002, 1003] {
            open.take(pending(id));
        }
        assert_eq!(flushed(&mut open, &store).1, [("1001".into(), 3)]);
        assert_eq!(
            open.shown[&1001].message.delivery,
            Some(conv::Delivery::Sending)
        );
        // Sent one by one: the server's ids, the server's album id.
        open.replace(1001, item(100, "99", "Ours", true), true);
        assert_eq!(
            flushed(&mut open, &store),
            (vec!["1001".into()], vec![("100".into(), 3)])
        );
        open.replace(1002, item(101, "99", "", true), true);
        open.replace(1003, item(102, "99", "", true), true);
        assert_eq!(
            flushed(&mut open, &store),
            (vec![], vec![("100".into(), 3)])
        );
        assert_eq!(open.items_of(102), [100, 101, 102]);
        assert_eq!(
            open.shown[&100].message.delivery,
            Some(conv::Delivery::Sent)
        );
        assert_eq!(open.shown.len(), 1);
    }

    #[test]
    fn tells_who_types() {
        let store = store();
        let line = |chat, typing: &[(i64, &str)]| typing_line(&store, chat, typing, &English);
        assert_eq!(line(101, &[]), None);
        assert_eq!(line(101, &[(101, "chatActionTyping")]).unwrap(), "typing…");
        assert_eq!(
            line(101, &[(101, "chatActionUploadingPhoto")]).unwrap(),
            "sending a photo…"
        );
        let group = -1000000001001;
        assert_eq!(
            line(group, &[(103, "chatActionTyping")]).unwrap(),
            "Petya is typing…"
        );
        assert_eq!(
            line(group, &[(103, "chatActionRecordingVoiceNote")]).unwrap(),
            "Petya is recording a voice message…"
        );
        assert_eq!(
            line(
                group,
                &[(103, "chatActionTyping"), (101, "chatActionTyping")]
            )
            .unwrap(),
            "Petya and Anna are typing…"
        );
        assert_eq!(
            line(group, &[(103, "a"), (101, "b"), (102, "c")]).unwrap(),
            "Petya and 2 others are typing…"
        );
    }
}
