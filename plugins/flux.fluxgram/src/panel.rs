//! The field's panel (part 4): what a chat's field offers besides Flux's own emoji — the
//! account's stickers (the favorite ones, the recent ones, then its sets; a search by emoji or by
//! word) and its GIFs (the saved ones; a search through Telegram's inline bot @gif). A window asks
//! ([`PanelAsk`]) and gets the conversation's changes in its chat's feed ([`Note::Feed`]): the
//! tab's content (`panel`), then its items again as their pictures come (`panel-put`).
//!
//! - Stickers: `getFavoriteStickers`, `getRecentStickers`, `getInstalledStickerSets` once per
//!   account (again on every opening, so the tab is fresh); the first sets' stickers at once
//!   (`getStickerSet`), the rest as their covers come into sight (`panel-visible`). An item is
//!   keyed `<section>:<sticker file id>`; its picture is the sticker's thumbnail, downloaded when
//!   seen; an animated one plays its animation once the sticker's file is downloaded and rendered
//!   at the panel's size (the feed's [`crate::feed::Frames`]).
//! - GIFs: `getSavedAnimations`; a search asks @gif (`searchPublicChat` once for the bot, then
//!   `getInlineQueryResults` page by page). A GIF's poster is its thumbnail, its video the MP4 once
//!   downloaded — both when seen.
//! - A choice is sent to the chat (`inputMessageSticker`, `inputMessageAnimation` with the remote
//!   file, `sendInlineQueryResultMessage` for a GIF found), answering the quote over the field.
//!
//! Needs the host only to send: the content is built from TDLib's JSON ([`content`]), tested
//! natively.

use std::collections::{HashMap, HashSet};

use flux_plugin_api::conversation::{
    self as conv, History, ImageSpec, PanelContent, PanelItem, PanelTab,
};
use flux_plugin_api::{log, service};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::convert::MediaOptions;
use crate::feed::Feed;
use crate::model::{ChatKey, Note, Words, downloaded};
use crate::service::{Asked, Cx, HostWords};
use crate::store::{Store, Touched};

/// A sticker's side in the panel, in pixels: twice its 72 points — its animation's size.
pub const PANEL_SIDE: u32 = 144;
/// The sets whose stickers load at once; the others as their covers come into sight.
const FIRST_SETS: usize = 4;
/// A search's stickers at most.
const FOUND: i32 = 100;
/// Download priorities: the pictures seen, then the stickers' files (for their animations).
const SEEN: i64 = 16;
const ANIMATION: i64 = 8;

/// What the field's panel asks the service: `Request::Panel`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "panel", rename_all = "kebab-case")]
pub enum PanelAsk {
    /// A tab was opened: its content, fresh.
    Opened { key: ChatKey, tab: PanelTab },
    /// A search in a tab (empty — back to the tab's own content).
    Searched {
        key: ChatKey,
        tab: PanelTab,
        text: String,
    },
    /// The items on screen: the first key and the last, in the content's order.
    Visible {
        key: ChatKey,
        tab: PanelTab,
        first: String,
        last: String,
    },
    /// The next page of a search (GIFs).
    LoadMore { key: ChatKey, tab: PanelTab },
    /// An item was chosen: sent, answering `reply_to` (the quote over the field).
    Chosen {
        key: ChatKey,
        tab: PanelTab,
        item: String,
        reply_to: Option<String>,
    },
}

/// What a request of the panel asked: its answer comes to [`Panels::answer`].
#[derive(Debug)]
pub enum Ask {
    Favorites,
    Recent,
    Sets,
    /// A set's stickers.
    Set(i64),
    Saved,
    /// @gif's chat (`searchPublicChat`): a search waits for it.
    Bot,
    /// A sticker search of a window's tab: stickers by emoji or word, or the sets matching.
    Found {
        window: u64,
        key: ChatKey,
        query: String,
        sets: bool,
    },
    /// A page of @gif's results for a window's search (`offset` — "" for the first).
    Gifs {
        window: u64,
        key: ChatKey,
        query: String,
        offset: String,
    },
    Done(&'static str),
}

/// What the panel knows of an account's stickers and GIFs.
#[derive(Default)]
struct Account {
    favorites: Option<Vec<Value>>,
    recent: Option<Vec<Value>>,
    /// The installed sets (`stickerSetInfo`), in Telegram's order.
    sets: Option<Vec<Value>>,
    /// The sets' stickers, once loaded.
    stickers: HashMap<i64, Vec<Value>>,
    loading: HashSet<i64>,
    saved: Option<Vec<Value>>,
    /// @gif's user id, once found; asked for already.
    bot: Option<i64>,
    bot_asked: bool,
    /// Files asked to download.
    downloading: HashSet<i64>,
}

/// A tab on show in a window's chat.
struct Shown {
    tab: PanelTab,
    /// The search it shows; empty — the tab's own content.
    query: String,
    /// A search's results: stickers, or @gif's `inlineQueryResultAnimation`s.
    found: Vec<Value>,
    /// The sticker search's answers still to come.
    waiting: u8,
    /// @gif's query id and next page ("" — none); a page on its way.
    query_id: String,
    next: String,
    loading: bool,
    /// The content as last sent, by item key: what `panel-put` replaces.
    items: HashMap<String, PanelItem>,
    /// The keys in order, as last sent: another order (or keys) — the whole content again.
    order: Vec<String>,
    /// Items seen (their files download).
    seen: HashSet<String>,
    /// Something it shows changed: built again at the end of the batch.
    dirty: bool,
}

impl Shown {
    fn new(tab: PanelTab) -> Self {
        Shown {
            tab,
            query: String::new(),
            found: Vec::new(),
            waiting: 0,
            query_id: String::new(),
            next: String::new(),
            loading: false,
            items: HashMap::new(),
            order: Vec::new(),
            seen: HashSet::new(),
            dirty: true,
        }
    }
}

pub struct Panels {
    accounts: HashMap<String, Account>,
    /// The tab each window shows in a chat's field.
    shown: HashMap<(u64, ChatKey), Shown>,
    /// How stickers loop (the plugin's setting).
    loop_stickers: bool,
}

impl Panels {
    pub fn new() -> Self {
        Panels {
            accounts: HashMap::new(),
            shown: HashMap::new(),
            loop_stickers: true,
        }
    }

    pub fn set_media(&mut self, media: &MediaOptions) {
        if self.loop_stickers != media.loop_stickers {
            self.loop_stickers = media.loop_stickers;
            self.touch_all();
        }
    }

    pub fn account_ready(&mut self, account: &str) {
        self.accounts
            .insert(account.to_string(), Account::default());
    }

    pub fn account_gone(&mut self, account: &str) {
        self.accounts.remove(account);
        self.shown.retain(|(_, key), _| key.account != account);
    }

    pub fn window_closed(&mut self, window: u64) {
        self.shown.retain(|(shown, _), _| *shown != window);
    }

    pub fn cache_cleared(&mut self) {
        for account in self.accounts.values_mut() {
            account.downloading.clear();
        }
        self.touch_all();
    }

    /// The animations' folder was trimmed: the tabs are built again (an animation gone renders
    /// again).
    pub fn animations_trimmed(&mut self) {
        self.touch_all();
    }

    /// An animation was rendered (or failed): the tabs showing it are built again.
    pub fn rendered(&mut self, _output: &str, _ok: bool) {
        self.touch_all();
    }

    fn touch_all(&mut self) {
        for shown in self.shown.values_mut() {
            shown.dirty = true;
        }
    }

    /// A file of the account changed: the tabs of its chats are built again (only the items that
    /// changed go).
    pub fn update(&mut self, account: &str, touched: Touched) {
        if let Touched::File(_) = touched {
            for ((_, key), shown) in self.shown.iter_mut() {
                if key.account == account {
                    shown.dirty = true;
                }
            }
        }
    }

    /// A window's request.
    pub fn request(&mut self, cx: &mut Cx, feed: &mut Feed, window: u64, ask: PanelAsk) {
        match ask {
            PanelAsk::Opened { key, tab } => {
                let account = self.accounts.entry(key.account.clone()).or_default();
                // Fresh on every opening: Telegram's lists may have changed.
                match tab {
                    PanelTab::Stickers => {
                        let asks = [
                            (json!({"@type": "getFavoriteStickers"}), Ask::Favorites),
                            (
                                json!({"@type": "getRecentStickers", "is_attached": false}),
                                Ask::Recent,
                            ),
                            (
                                json!({"@type": "getInstalledStickerSets",
                                    "sticker_type": {"@type": "stickerTypeRegular"}}),
                                Ask::Sets,
                            ),
                        ];
                        for (request, asked) in asks {
                            cx.td.send(&key.account, request, Asked::Panel(asked));
                        }
                    }
                    PanelTab::Gifs => cx.td.send(
                        &key.account,
                        json!({"@type": "getSavedAnimations"}),
                        Asked::Panel(Ask::Saved),
                    ),
                    PanelTab::Emoji => return,
                }
                let _ = account;
                self.shown.insert((window, key), Shown::new(tab));
            }
            PanelAsk::Searched { key, tab, text } => {
                let query = text.trim().to_string();
                let shown = self
                    .shown
                    .entry((window, key.clone()))
                    .or_insert_with(|| Shown::new(tab));
                shown.tab = tab;
                shown.query = query.clone();
                shown.found.clear();
                shown.query_id.clear();
                shown.next.clear();
                shown.loading = false;
                shown.dirty = true;
                if query.is_empty() {
                    return;
                }
                match tab {
                    PanelTab::Stickers => {
                        shown.waiting = 2;
                        let stickers = if is_emoji(&query) {
                            json!({"@type": "getStickers",
                                "sticker_type": {"@type": "stickerTypeRegular"},
                                "query": query, "limit": FOUND, "chat_id": key.chat})
                        } else {
                            json!({"@type": "searchStickers",
                                "sticker_type": {"@type": "stickerTypeRegular"}, "emojis": "",
                                "query": query, "input_language_codes": [], "offset": 0,
                                "limit": FOUND})
                        };
                        let sets = json!({"@type": "searchInstalledStickerSets",
                            "sticker_type": {"@type": "stickerTypeRegular"}, "query": query,
                            "limit": 20});
                        for (request, of_sets) in [(stickers, false), (sets, true)] {
                            cx.td.send(
                                &key.account,
                                request,
                                Asked::Panel(Ask::Found {
                                    window,
                                    key: key.clone(),
                                    query: query.clone(),
                                    sets: of_sets,
                                }),
                            );
                        }
                    }
                    PanelTab::Gifs => {
                        shown.loading = true;
                        self.search_gifs(cx, window, &key, &query, "");
                    }
                    PanelTab::Emoji => {}
                }
            }
            PanelAsk::Visible {
                key,
                tab,
                first,
                last,
            } => self.visible(cx, feed, window, &key, tab, &first, &last),
            PanelAsk::LoadMore { key, tab } => {
                let Some(shown) = self
                    .shown
                    .get_mut(&(window, key.clone()))
                    .filter(|shown| shown.tab == tab && tab == PanelTab::Gifs)
                else {
                    return;
                };
                if shown.loading || shown.next.is_empty() || shown.query.is_empty() {
                    return;
                }
                shown.loading = true;
                shown.dirty = true;
                let (query, next) = (shown.query.clone(), shown.next.clone());
                self.search_gifs(cx, window, &key, &query, &next);
            }
            PanelAsk::Chosen {
                key,
                tab,
                item,
                reply_to,
            } => self.choose(cx, feed, window, &key, tab, &item, reply_to),
        }
    }

    /// A page of @gif's results: the bot's chat first, once.
    fn search_gifs(&mut self, cx: &mut Cx, window: u64, key: &ChatKey, query: &str, offset: &str) {
        let account = self.accounts.entry(key.account.clone()).or_default();
        let Some(bot) = account.bot else {
            if !account.bot_asked {
                account.bot_asked = true;
                cx.td.send(
                    &key.account,
                    json!({"@type": "searchPublicChat", "username": "gif"}),
                    Asked::Panel(Ask::Bot),
                );
            }
            return;
        };
        cx.td.send(
            &key.account,
            json!({"@type": "getInlineQueryResults", "bot_user_id": bot, "chat_id": key.chat,
                "user_location": null, "query": query, "offset": offset}),
            Asked::Panel(Ask::Gifs {
                window,
                key: key.clone(),
                query: query.to_string(),
                offset: offset.to_string(),
            }),
        );
    }

    /// The answer to a request of the panel.
    pub fn answer(&mut self, cx: &mut Cx, feed: &mut Feed, account: &str, ask: Ask, value: &Value) {
        let error = (value["@type"] == "error").then(|| value["message"].clone());
        if let Some(error) = &error {
            let what = match &ask {
                Ask::Done(what) => what.to_string(),
                other => format!("{other:?}"),
            };
            log::warn(&format!("{account}: the panel's {what} failed: {error}"));
        }
        let _ = feed;
        let mine = self.accounts.entry(account.to_string()).or_default();
        let list = |value: &Value, key: &str| -> Vec<Value> {
            value[key].as_array().cloned().unwrap_or_default()
        };
        match ask {
            Ask::Favorites => mine.favorites = Some(list(value, "stickers")),
            Ask::Recent => mine.recent = Some(list(value, "stickers")),
            Ask::Sets => {
                let sets = list(value, "sets");
                // The first sets' stickers at once; the rest as they come into sight.
                for set in sets.iter().take(FIRST_SETS) {
                    if let Some(id) = int64(&set["id"]) {
                        load_set(cx, account, mine, id);
                    }
                }
                mine.sets = Some(sets);
            }
            Ask::Set(id) => {
                mine.loading.remove(&id);
                if error.is_none() {
                    mine.stickers.insert(id, list(value, "stickers"));
                }
            }
            Ask::Saved => mine.saved = Some(list(value, "animations")),
            Ask::Bot => {
                // A private chat's id is its user's.
                mine.bot = value["id"].as_i64().filter(|_| error.is_none());
                if mine.bot.is_none() {
                    mine.bot_asked = false;
                }
                let waiting: Vec<(u64, ChatKey, String)> = self
                    .shown
                    .iter()
                    .filter(|((_, key), shown)| {
                        key.account == account
                            && shown.tab == PanelTab::Gifs
                            && shown.loading
                            && !shown.query.is_empty()
                    })
                    .map(|((window, key), shown)| (*window, key.clone(), shown.query.clone()))
                    .collect();
                for (window, key, query) in waiting {
                    if self.accounts.get(account).is_some_and(|a| a.bot.is_some()) {
                        self.search_gifs(cx, window, &key, &query, "");
                    } else if let Some(shown) = self.shown.get_mut(&(window, key)) {
                        shown.loading = false;
                        shown.dirty = true;
                    }
                }
            }
            Ask::Found {
                window,
                key,
                query,
                sets,
            } => {
                let Some(shown) = self
                    .shown
                    .get_mut(&(window, key))
                    .filter(|shown| shown.query == query && shown.tab == PanelTab::Stickers)
                else {
                    return;
                };
                let found = if sets {
                    // The sets matching: their covers.
                    list(value, "sets")
                        .iter()
                        .flat_map(|set| list(set, "covers"))
                        .collect()
                } else {
                    list(value, "stickers")
                };
                for sticker in found {
                    let id = sticker["sticker"]["id"].as_i64();
                    if !shown
                        .found
                        .iter()
                        .any(|known| known["sticker"]["id"].as_i64() == id)
                    {
                        shown.found.push(sticker);
                    }
                }
                shown.waiting = shown.waiting.saturating_sub(1);
                shown.dirty = true;
            }
            Ask::Gifs {
                window,
                key,
                query,
                offset,
            } => {
                let Some(shown) = self
                    .shown
                    .get_mut(&(window, key))
                    .filter(|shown| shown.query == query && shown.tab == PanelTab::Gifs)
                else {
                    return;
                };
                shown.loading = false;
                shown.dirty = true;
                if error.is_some() {
                    shown.next.clear();
                    return;
                }
                if offset.is_empty() {
                    shown.found.clear();
                }
                shown.query_id = int64(&value["inline_query_id"])
                    .map(|id| id.to_string())
                    .unwrap_or_default();
                shown.next = value["next_offset"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                // Each result is sent with the query id of its own page.
                let query_id = shown.query_id.clone();
                shown.found.extend(
                    list(value, "results")
                        .into_iter()
                        .filter(|result| result["@type"] == "inlineQueryResultAnimation")
                        .map(|mut result| {
                            result[QUERY_ID] = json!(query_id);
                            result
                        }),
                );
            }
            Ask::Done(_) => return,
        }
        // Whatever came: the account's tabs are built again.
        for ((_, key), shown) in self.shown.iter_mut() {
            if key.account == account {
                shown.dirty = true;
            }
        }
    }

    /// The items on screen: their pictures download (and the animated stickers' files, for their
    /// animations); a set whose covers came into sight loads.
    #[allow(clippy::too_many_arguments)]
    fn visible(
        &mut self,
        cx: &mut Cx,
        feed: &mut Feed,
        window: u64,
        key: &ChatKey,
        tab: PanelTab,
        first: &str,
        last: &str,
    ) {
        let Some(shown) = self
            .shown
            .get_mut(&(window, key.clone()))
            .filter(|shown| shown.tab == tab)
        else {
            return;
        };
        let (Some(from), Some(to)) = (
            shown.order.iter().position(|item| item == first),
            shown.order.iter().position(|item| item == last),
        ) else {
            return;
        };
        let (from, to) = (from.min(to), from.max(to));
        let keys: Vec<String> = shown.order[from..=to].to_vec();
        shown.seen.extend(keys.iter().cloned());
        shown.dirty = true;
        let Some(account) = self.accounts.get_mut(&key.account) else {
            return;
        };
        let Some(store) = cx.stores.get(&key.account) else {
            return;
        };
        let mut wanted: Vec<(i64, i64)> = Vec::new();
        let mut sets: Vec<i64> = Vec::new();
        for item in &keys {
            let Some(media) = find(account, shown, item) else {
                continue;
            };
            match media {
                Found::Sticker(sticker, set) => {
                    if let Some(set) = set.filter(|set| !account.stickers.contains_key(set)) {
                        sets.push(set);
                    }
                    if let Some(file) = sticker_picture_file(sticker)
                        && store.file_path(file).is_none()
                    {
                        wanted.push((file, SEEN));
                    }
                    if animated(sticker)
                        && let Some(file) = sticker["sticker"]["id"].as_i64()
                        && store.file_path(file).is_none()
                    {
                        wanted.push((file, ANIMATION));
                    }
                }
                Found::Gif(animation) => {
                    for (file, priority) in [
                        (animation["thumbnail"]["file"]["id"].as_i64(), SEEN),
                        (animation["animation"]["id"].as_i64(), ANIMATION),
                    ] {
                        if let Some(file) = file.filter(|file| store.file_path(*file).is_none()) {
                            wanted.push((file, priority));
                        }
                    }
                }
            }
        }
        for set in sets {
            load_set(cx, &key.account, account, set);
        }
        for (file, priority) in wanted {
            if account.downloading.insert(file) {
                cx.td.send(
                    &key.account,
                    json!({"@type": "downloadFile", "file_id": file, "priority": priority,
                        "offset": 0, "limit": 0, "synchronous": false}),
                    Asked::Panel(Ask::Done("downloadFile")),
                );
            }
        }
        let _ = feed;
    }

    /// An item chosen: sent to the chat.
    #[allow(clippy::too_many_arguments)]
    fn choose(
        &mut self,
        cx: &mut Cx,
        feed: &mut Feed,
        window: u64,
        key: &ChatKey,
        tab: PanelTab,
        item: &str,
        reply_to: Option<String>,
    ) {
        let (Some(shown), Some(account)) = (
            self.shown.get(&(window, key.clone())),
            self.accounts.get(&key.account),
        ) else {
            return;
        };
        if shown.tab != tab {
            return;
        }
        let Some(media) = find(account, shown, item) else {
            return;
        };
        match media {
            Found::Sticker(sticker, _) => {
                let Some(remote) = remote_id(&sticker["sticker"]) else {
                    return;
                };
                let content = json!({"@type": "inputMessageSticker",
                    "sticker": {"@type": "inputFileRemote", "id": remote}, "thumbnail": null,
                    "width": sticker["width"], "height": sticker["height"],
                    "emoji": sticker["emoji"].as_str().unwrap_or_default()});
                feed.send_content(cx, key, content, reply_to.as_deref());
            }
            Found::Gif(animation) => {
                // A found one: Telegram sends the bot's result itself.
                if let Some(result) = item
                    .strip_prefix(FOUND_SECTION)
                    .and_then(|rest| rest.strip_prefix(':'))
                    && let Some(found) = shown
                        .found
                        .iter()
                        .find(|found| found["id"].as_str() == Some(result))
                {
                    let query = found[QUERY_ID].as_str().unwrap_or_default();
                    feed.send_inline_result(cx, key, query, result, reply_to.as_deref());
                    return;
                }
                let Some(remote) = remote_id(&animation["animation"]) else {
                    return;
                };
                let content = json!({"@type": "inputMessageAnimation",
                    "animation": {"@type": "inputFileRemote", "id": remote}, "thumbnail": null,
                    "added_sticker_file_ids": [], "duration": animation["duration"],
                    "width": animation["width"], "height": animation["height"], "caption": null,
                    "show_caption_above_media": false, "has_spoiler": false});
                feed.send_content(cx, key, content, reply_to.as_deref());
            }
        }
    }

    /// The end of a batch: the tabs that changed go to their windows — only the items that
    /// changed (`panel-put`), or the whole content when its items or their order changed.
    pub fn flush(&mut self, cx: &mut Cx, feed: &mut Feed) {
        for ((window, key), shown) in self.shown.iter_mut() {
            if !shown.dirty {
                continue;
            }
            shown.dirty = false;
            let (Some(account), Some(store)) =
                (self.accounts.get(&key.account), cx.stores.get(&key.account))
            else {
                continue;
            };
            // The animations of the stickers seen whose files are there: rendered once.
            let mut renders: Vec<(String, String)> = Vec::new();
            for item in &shown.seen {
                if let Some(Found::Sticker(sticker, _)) = find(account, shown, item)
                    && animated(sticker)
                    && let Some(output) = feed.media.frames_of(&sticker["sticker"], PANEL_SIDE)
                    && !feed.frames.found(&mut feed.media, &output)
                    && let Some(input) = sticker["sticker"]["id"]
                        .as_i64()
                        .and_then(|file| store.file_path(file))
                {
                    renders.push((input.to_string(), output));
                }
            }
            let built = content(
                account,
                shown,
                store,
                &feed.media,
                self.loop_stickers,
                &HostWords,
            );
            for (input, output) in renders {
                feed.frames.render(cx, &input, &output, PANEL_SIDE);
            }
            let order: Vec<String> = built
                .sections
                .iter()
                .flat_map(|section| section.items.iter().map(|item| item.key.clone()))
                .collect();
            let change = if order == shown.order && !shown.items.is_empty() {
                let changed: Vec<PanelItem> = built
                    .sections
                    .iter()
                    .flat_map(|section| section.items.iter())
                    .filter(|item| shown.items.get(&item.key) != Some(*item))
                    .cloned()
                    .collect();
                (!changed.is_empty()).then(|| conv::panel_put(shown.tab, changed))
            } else {
                Some(conv::panel(built.clone()))
            };
            shown.order = order;
            shown.items = built
                .sections
                .into_iter()
                .flat_map(|section| section.items)
                .map(|item| (item.key.clone(), item))
                .collect();
            if let Some(change) = change {
                let _ = service::send_json_to(
                    *window,
                    &Note::Feed {
                        key: key.clone(),
                        changes: vec![change],
                    },
                );
            }
        }
    }
}

/// The key prefix of a search's section.
const FOUND_SECTION: &str = "found";
/// The query id an @gif result came with (kept in the result): what sends it.
const QUERY_ID: &str = "fluxgram_query_id";

/// What an item stands for: a sticker (with its set, when it is a set's cover not loaded), a GIF.
enum Found<'a> {
    Sticker(&'a Value, Option<i64>),
    Gif(&'a Value),
}

/// The sticker or the GIF of an item's key (`<section>:<id>`).
fn find<'a>(account: &'a Account, shown: &'a Shown, item: &str) -> Option<Found<'a>> {
    let (section, id) = item.rsplit_once(':')?;
    if shown.tab == PanelTab::Gifs {
        if section == FOUND_SECTION {
            return shown
                .found
                .iter()
                .find(|result| result["id"].as_str() == Some(id))
                .map(|result| Found::Gif(&result["animation"]));
        }
        let file = id.parse::<i64>().ok()?;
        return account
            .saved
            .iter()
            .flatten()
            .find(|animation| animation["animation"]["id"].as_i64() == Some(file))
            .map(Found::Gif);
    }
    let file = id.parse::<i64>().ok()?;
    let by_file = |sticker: &&Value| sticker["sticker"]["id"].as_i64() == Some(file);
    let list: Option<(&Vec<Value>, Option<i64>)> = match section {
        FOUND_SECTION => Some((&shown.found, None)),
        "favorites" => account.favorites.as_ref().map(|list| (list, None)),
        "recent" => account.recent.as_ref().map(|list| (list, None)),
        set => {
            let set = set.strip_prefix("set-")?.parse::<i64>().ok()?;
            match account.stickers.get(&set) {
                Some(stickers) => Some((stickers, None)),
                None => account
                    .sets
                    .iter()
                    .flatten()
                    .find(|info| int64(&info["id"]) == Some(set))
                    .and_then(|info| info["covers"].as_array())
                    .map(|covers| (covers, Some(set))),
            }
        }
    };
    let (list, set) = list?;
    list.iter()
        .find(by_file)
        .map(|sticker| Found::Sticker(sticker, set))
}

/// The tab's content: its sections and their items, as the window shows them.
fn content(
    account: &Account,
    shown: &Shown,
    store: &Store,
    media: &MediaOptions,
    looped: bool,
    words: &dyn Words,
) -> PanelContent {
    let mut sections = Vec::new();
    let more = |shown: &Shown| {
        if shown.loading {
            History::Loading
        } else if shown.next.is_empty() {
            History::Complete
        } else {
            History::More
        }
    };
    if shown.tab == PanelTab::Gifs {
        if shown.query.is_empty() {
            let items = account
                .saved
                .iter()
                .flatten()
                .filter_map(|animation| {
                    let file = animation["animation"]["id"].as_i64()?;
                    Some(gif_item(&format!("saved:{file}"), animation, store, shown))
                })
                .collect();
            sections.push(conv::panel_section(
                "saved",
                &words.tr("Saved"),
                None,
                items,
            ));
            return conv::panel_content(PanelTab::Gifs, "", sections, History::Complete);
        }
        let items = shown
            .found
            .iter()
            .filter_map(|result| {
                let id = result["id"].as_str()?;
                Some(gif_item(
                    &format!("{FOUND_SECTION}:{id}"),
                    &result["animation"],
                    store,
                    shown,
                ))
            })
            .collect();
        sections.push(conv::panel_section(
            FOUND_SECTION,
            &words.tr("Found"),
            None,
            items,
        ));
        return conv::panel_content(PanelTab::Gifs, &shown.query, sections, more(shown));
    }
    let stickers = |section: &str, list: &[Value]| -> Vec<PanelItem> {
        list.iter()
            .filter_map(|sticker| {
                let file = sticker["sticker"]["id"].as_i64()?;
                let key = format!("{section}:{file}");
                let seen = shown.seen.contains(&key);
                Some(sticker_item(&key, sticker, store, media, looped, seen))
            })
            .collect()
    };
    if !shown.query.is_empty() {
        let items = stickers(FOUND_SECTION, &shown.found);
        sections.push(conv::panel_section(
            FOUND_SECTION,
            &words.tr("Found"),
            None,
            items,
        ));
        let more = if shown.waiting > 0 {
            History::Loading
        } else {
            History::Complete
        };
        return conv::panel_content(PanelTab::Stickers, &shown.query, sections, more);
    }
    for (key, title, list) in [
        ("favorites", "Favorites", &account.favorites),
        ("recent", "Recent", &account.recent),
    ] {
        let items = stickers(key, list.as_deref().unwrap_or_default());
        if !items.is_empty() {
            sections.push(conv::panel_section(key, &words.tr(title), None, items));
        }
    }
    for set in account.sets.iter().flatten() {
        let Some(id) = int64(&set["id"]) else {
            continue;
        };
        let key = format!("set-{id}");
        let list = account
            .stickers
            .get(&id)
            .cloned()
            .unwrap_or_else(|| set["covers"].as_array().cloned().unwrap_or_default());
        let items = stickers(&key, &list);
        let title = set["title"].as_str().unwrap_or_default();
        // The set's thumbnail in the strip (a still one); none — its first sticker's.
        let icon = set["thumbnail"]["file"]
            .as_object()
            .and_then(|_| still_path(store, &set["thumbnail"]))
            .map(|path| conv::picture(&path, 100, 100));
        sections.push(conv::panel_section(&key, title, icon, items));
    }
    let more = if account.sets.is_none() || account.favorites.is_none() || account.recent.is_none()
    {
        History::Loading
    } else {
        History::Complete
    };
    conv::panel_content(PanelTab::Stickers, "", sections, more)
}

/// A sticker of the panel: its thumbnail (downloaded when seen), its animation once rendered
/// at the panel's size, its emoji.
fn sticker_item(
    key: &str,
    sticker: &Value,
    store: &Store,
    media: &MediaOptions,
    looped: bool,
    seen: bool,
) -> PanelItem {
    let width = sticker["width"].as_u64().unwrap_or(512) as u32;
    let height = sticker["height"].as_u64().unwrap_or(512) as u32;
    let path = sticker_picture_file(sticker).and_then(|file| store.file_path(file));
    let mut image = match path {
        Some(path) => conv::picture(path, width, height),
        None => conv::pending_picture(width, height),
    };
    if seen
        && animated(sticker)
        && let Some(output) = media.frames_of(&sticker["sticker"], PANEL_SIDE)
        && media.rendered.contains(&output)
    {
        image = image.animation(&output, looped);
    }
    let mut item = conv::panel_item(key, image);
    if let Some(emoji) = sticker["emoji"].as_str().filter(|emoji| !emoji.is_empty()) {
        item = item.emoji(emoji);
    }
    item
}

/// A GIF of the panel: its poster (the thumbnail, else the blurred minithumbnail), its MP4 once
/// downloaded.
fn gif_item(key: &str, animation: &Value, store: &Store, shown: &Shown) -> PanelItem {
    let width = animation["width"].as_u64().unwrap_or(320) as u32;
    let height = animation["height"].as_u64().unwrap_or(240) as u32;
    let poster = still_path(store, &animation["thumbnail"]);
    let image: ImageSpec = match poster {
        Some(path) => conv::picture(&path, width, height),
        None => {
            let mut spec = conv::pending_picture(width, height);
            if let Some(bytes) = animation["minithumbnail"]["data"]
                .as_str()
                .and_then(|data| {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.decode(data).ok()
                })
            {
                spec = spec.preview(bytes);
            }
            spec
        }
    };
    let mut item = conv::panel_item(key, image);
    let video = animation["animation"]["id"]
        .as_i64()
        .and_then(|file| store.file_path(file))
        .map(str::to_string)
        .or_else(|| downloaded(&animation["animation"]));
    if let Some(video) = video.filter(|_| shown.seen.contains(key)) {
        item = item.video(&video);
    }
    item
}

/// The picture of a sticker in the panel: its thumbnail's file (a static one without a
/// thumbnail — its own WebP).
fn sticker_picture_file(sticker: &Value) -> Option<i64> {
    let thumbnail = &sticker["thumbnail"];
    let still = matches!(
        thumbnail["format"]["@type"].as_str().unwrap_or_default(),
        "thumbnailFormatJpeg" | "thumbnailFormatPng" | "thumbnailFormatWebp"
    );
    if still && let Some(file) = thumbnail["file"]["id"].as_i64() {
        return Some(file);
    }
    (sticker["format"]["@type"] == "stickerFormatWebp")
        .then(|| sticker["sticker"]["id"].as_i64())
        .flatten()
}

/// A still thumbnail's path once downloaded (the store's knowledge first).
fn still_path(store: &Store, thumbnail: &Value) -> Option<String> {
    let still = matches!(
        thumbnail["format"]["@type"].as_str().unwrap_or_default(),
        "thumbnailFormatJpeg" | "thumbnailFormatPng" | "thumbnailFormatWebp" | "thumbnailFormatGif"
    );
    if !still {
        return None;
    }
    let file = &thumbnail["file"];
    match file["id"].as_i64().and_then(|id| store.file_path(id)) {
        Some(path) => Some(path.to_string()),
        None => downloaded(file),
    }
}

fn animated(sticker: &Value) -> bool {
    matches!(
        sticker["format"]["@type"].as_str().unwrap_or_default(),
        "stickerFormatTgs" | "stickerFormatWebm"
    )
}

/// A file's remote id: what sends it again without uploading.
fn remote_id(file: &Value) -> Option<&str> {
    file["remote"]["id"].as_str().filter(|id| !id.is_empty())
}

/// TDLib's int64: a string in its JSON (a number in older answers).
fn int64(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|text| text.parse().ok())
        .or_else(|| value.as_i64())
}

/// A set's stickers, asked once.
fn load_set(cx: &mut Cx, account_key: &str, account: &mut Account, set: i64) {
    if account.stickers.contains_key(&set) || !account.loading.insert(set) {
        return;
    }
    cx.td.send(
        account_key,
        json!({"@type": "getStickerSet", "set_id": set.to_string()}),
        Asked::Panel(Ask::Set(set)),
    );
}

/// The search is an emoji (stickers by it), not a word.
fn is_emoji(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_alphanumeric)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::English;

    fn file(id: i64, path: &str) -> Value {
        json!({"@type": "file", "id": id, "size": 1000, "expected_size": 1000,
            "local": {"path": path, "is_downloading_completed": !path.is_empty(),
                "is_downloading_active": false, "downloaded_size": 0},
            "remote": {"id": format!("remote-{id}"), "unique_id": format!("u{id}"),
                "is_uploading_active": false, "uploaded_size": 0}})
    }

    fn sticker(id: i64, emoji: &str, format: &str) -> Value {
        json!({"@type": "sticker", "id": id.to_string(), "set_id": "1", "width": 512,
            "height": 512, "emoji": emoji, "format": {"@type": format},
            "thumbnail": {"@type": "thumbnail", "format": {"@type": "thumbnailFormatWebp"},
                "width": 128, "height": 128, "file": file(id + 1000, "")},
            "sticker": file(id, "")})
    }

    #[test]
    fn builds_the_stickers_tab() {
        let mut store = Store::default();
        let account = Account {
            favorites: Some(vec![sticker(1, "😂", "stickerFormatWebp")]),
            recent: Some(vec![]),
            sets: Some(vec![
                json!({"@type": "stickerSetInfo", "id": "77", "title": "Cats",
                    "thumbnail": null, "covers": [sticker(2, "🐱", "stickerFormatTgs")]}),
                json!({"@type": "stickerSetInfo", "id": "78", "title": "Dogs",
                    "thumbnail": null, "covers": []}),
            ]),
            ..Account::default()
        };
        let mut shown = Shown::new(PanelTab::Stickers);
        let media = MediaOptions {
            frames: "/cache/frames".into(),
            ..MediaOptions::default()
        };
        let built = content(&account, &shown, &store, &media, true, &English);
        let keys: Vec<(&str, Vec<&str>)> = built
            .sections
            .iter()
            .map(|section| {
                (
                    section.key.as_str(),
                    section.items.iter().map(|item| item.key.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("favorites", vec!["favorites:1"]),
                ("set-77", vec!["set-77:2"]),
                ("set-78", vec![])
            ],
            "the empty Recent goes; an unloaded set shows its covers"
        );
        assert_eq!(built.sections[0].items[0].emoji.as_deref(), Some("😂"));
        assert_eq!(built.sections[0].items[0].image.path, None);
        assert_eq!(built.more, History::Complete);
        // A cover seen: it is a sticker of an unloaded set (loaded then).
        let Some(Found::Sticker(_, set)) = find(&account, &shown, "set-77:2") else {
            panic!()
        };
        assert_eq!(set, Some(77));
        // The thumbnail downloaded and the animation rendered: the picture and its animation.
        store.apply(&json!({"@type": "updateFile", "file": file(1002, "/f/2.webp")}));
        let mut media = media;
        media.rendered.insert("/cache/frames/u2-144.fxa".into());
        shown.seen.insert("set-77:2".into());
        let built = content(&account, &shown, &store, &media, false, &English);
        let item = &built.sections[1].items[0];
        assert_eq!(item.image.path.as_deref(), Some("/f/2.webp"));
        let animation = item.image.animation.as_ref().unwrap();
        assert_eq!(
            (animation.path.as_str(), animation.looped),
            ("/cache/frames/u2-144.fxa", false)
        );
        // A search: one "Found" section.
        shown.query = "cat".into();
        shown.found = vec![sticker(5, "🐱", "stickerFormatWebp")];
        let built = content(&account, &shown, &store, &media, true, &English);
        assert_eq!(built.query, "cat");
        assert_eq!(built.sections.len(), 1);
        assert_eq!(built.sections[0].items[0].key, "found:5");
        assert!(is_emoji("😂") && !is_emoji("cat"));
    }

    #[test]
    fn builds_the_gifs_tab() {
        let mut store = Store::default();
        let gif = |id: i64| {
            json!({"@type": "animation", "duration": 3, "width": 320, "height": 240,
                "thumbnail": {"format": {"@type": "thumbnailFormatJpeg"}, "file": file(id + 1, "")},
                "animation": file(id, "")})
        };
        let account = Account {
            saved: Some(vec![gif(10)]),
            ..Account::default()
        };
        let mut shown = Shown::new(PanelTab::Gifs);
        let built = content(
            &account,
            &shown,
            &store,
            &MediaOptions::default(),
            true,
            &English,
        );
        assert_eq!(built.sections[0].key, "saved");
        assert_eq!(built.sections[0].items[0].key, "saved:10");
        assert_eq!(built.sections[0].items[0].video, None);
        // Seen and downloaded: the poster and the video.
        store.apply(&json!({"@type": "updateFile", "file": file(10, "/f/10.mp4")}));
        store.apply(&json!({"@type": "updateFile", "file": file(11, "/f/11.jpg")}));
        shown.seen.insert("saved:10".into());
        let built = content(
            &account,
            &shown,
            &store,
            &MediaOptions::default(),
            true,
            &English,
        );
        let item = &built.sections[0].items[0];
        assert_eq!(item.video.as_deref(), Some("/f/10.mp4"));
        assert_eq!(item.image.path.as_deref(), Some("/f/11.jpg"));
        // A search's page: the found ones, more to come.
        shown.query = "cats".into();
        shown.found = vec![json!({"@type": "inlineQueryResultAnimation", "id": "r1",
            "animation": gif(20), "title": ""})];
        shown.next = "25".into();
        let built = content(
            &account,
            &shown,
            &store,
            &MediaOptions::default(),
            true,
            &English,
        );
        assert_eq!(built.sections[0].items[0].key, "found:r1");
        assert_eq!(built.more, History::More);
        assert!(matches!(
            find(&account, &shown, "found:r1"),
            Some(Found::Gif(_))
        ));
    }
}
