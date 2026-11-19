//! The service's profiles and media (part 3+): what the profile dialog shows — a person (the
//! picture, the name, "online", @username, the phone when Telegram shows it, the bio), a group
//! (its description, link, members with their roles, a search over them), and a chat's media by
//! kind (photos and videos by month, files, links, voice messages), page by page.
//!
//! The dialog asks with [`ProfileAsk`] (`Request::Profile`), the service answers the window that
//! asked with [`ProfileTold`] (`Note::Profile`); TDLib's answers come back to [`Profiles::answer`]
//! with this module's [`Ask`].
//!
//! Each window's dialog shows one profile at a time ([`ProfileAsk::Open`] replaces the one before,
//! [`ProfileAsk::Close`] ends it): the service keeps what TDLib told of it beyond the store (the
//! bio or the description, the phone, the counts of media, the members, the loaded media) and, at
//! the end of a batch ([`Profiles::flush`]), sends what changed — the profile and the members
//! only when they differ, the media as the items that changed (a page, a download's progress).
//! Its chat joins the helper's filter while it is on show ([`Profiles::chats`]): the person's
//! status and the chat's muting come as updates. Pictures download only for the squares in sight,
//! files only when clicked.
//!
//! What the dialog shows is made here, with the words ([`Words`]), and needs no host: tested
//! natively.

use std::collections::{BTreeMap, HashMap, HashSet};

use base64::Engine;
use flux_plugin_api::conversation::AvatarSpec;
use flux_plugin_api::{log, service, system};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{ChatKey, Note, Words, downloaded, unix_now};
use crate::service::{Asked, Cx, HostWords};
use crate::store::{ChatKind, Status, Store, Touched, UserKind};
use crate::{chats, convert, summary};

/// A page of media.
const PAGE: i64 = 60;
/// The members of a supergroup asked at once (TDLib's most).
const MEMBERS: i64 = 200;
/// The members a search finds.
const FOUND: i64 = 50;
/// A gallery's square: the photo's size nearest this side, not smaller.
const SQUARE_SIDE: i64 = 320;
/// Download priorities (TDLib's 1–32): a click; the squares in sight, the first ones higher; the
/// large picture of the profile.
const CLICKED: i64 = 32;
const IN_SIGHT: i64 = 24;
const AVATAR: i64 = 16;
/// The members whose pictures download: the first rows.
const AVATARS: usize = 40;
/// Muted "forever", as Telegram's apps mute.
pub const FOREVER: i64 = 2_147_483_647;

/// The kinds of a chat's media, each a section of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaKind {
    /// Photos and videos: a gallery by month.
    Photos,
    Files,
    Links,
    /// Voice messages.
    Voice,
}

impl MediaKind {
    pub const ALL: [MediaKind; 4] = [
        MediaKind::Photos,
        MediaKind::Files,
        MediaKind::Links,
        MediaKind::Voice,
    ];

    /// TDLib's `searchMessagesFilter…` of the kind.
    fn filter(self) -> &'static str {
        match self {
            MediaKind::Photos => "searchMessagesFilterPhotoAndVideo",
            MediaKind::Files => "searchMessagesFilterDocument",
            MediaKind::Links => "searchMessagesFilterUrl",
            MediaKind::Voice => "searchMessagesFilterVoiceNote",
        }
    }
}

/// What the profile dialog asks the service: `Request::Profile`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "profile", rename_all = "kebab-case")]
pub enum ProfileAsk {
    /// Show the chat's profile (a person's chat — the person, even one without a chat yet: a
    /// person's chat has their id): [`ProfileTold::Profile`] now and whenever it changes, until
    /// another `Open` or `Close`.
    Open { key: ChatKey },
    /// The dialog closed.
    Close,
    /// The group's members matching `query` (empty — the admins first, then the recent ones).
    Members { key: ChatKey, query: String },
    /// The media of a kind from the newest (`more: false`), or the next page.
    Media {
        key: ChatKey,
        kind: MediaKind,
        more: bool,
    },
    /// The items in sight, the first and the last (message ids): their pictures download first.
    Visible {
        key: ChatKey,
        kind: MediaKind,
        first: i64,
        last: i64,
    },
    /// An item was clicked whose file isn't there: a photo's full size, a video, a file, a voice
    /// message downloads.
    Download {
        key: ChatKey,
        kind: MediaKind,
        message: i64,
    },
    /// Mute the chat for good, or unmute it (a profile's button, a chat tab's menu).
    Mute { key: ChatKey, muted: bool },
    /// Mute the chat for this many seconds (part 5: Mute's choice — an hour, 8 hours, 2 days).
    MuteFor { key: ChatKey, seconds: i64 },
}

/// What the service tells the profile dialog: `Note::Profile`, to the window that asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "told", rename_all = "kebab-case")]
pub enum ProfileTold {
    /// The profile on show: when it is asked for, and whenever it changes.
    Profile { key: ChatKey, profile: Profile },
    /// The members found for `query`; `problem` — why there are none (Telegram hides them).
    Members {
        key: ChatKey,
        query: String,
        members: Vec<Member>,
        problem: Option<String>,
    },
    /// Items of a kind, newest first: a page's new ones and the ones that changed (a picture
    /// downloaded, a download's progress), by message; `reset` — the list starts again with
    /// these; `more` — older ones remain.
    Media {
        key: ChatKey,
        kind: MediaKind,
        items: Vec<MediaItem>,
        more: bool,
        reset: bool,
    },
}

/// A profile's top, info and state, in the interface's language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub title: String,
    pub avatar: AvatarSpec,
    /// "online", "last seen at 14:32", "5 members"; empty — nothing to say (Saved Messages).
    pub status: String,
    /// The status is "online": the accent.
    pub online: bool,
    /// A group: the members' section.
    pub group: bool,
    /// "@anna".
    pub username: Option<String>,
    /// "+7 900 123 45 67", only when Telegram shows it.
    pub phone: Option<String>,
    /// A person's bio, a group's description.
    pub about: Option<String>,
    /// A public group's link: "t.me/fluxdev".
    pub link: Option<String>,
    /// A group's members.
    pub members: u32,
    pub muted: bool,
    /// The user can write there (Message opens the chat).
    pub can_message: bool,
    /// The counts of the chat's media, by kind, once Telegram told them.
    pub counts: Vec<(MediaKind, u32)>,
}

/// A group's member as the dialog shows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub user: i64,
    pub name: String,
    pub avatar: AvatarSpec,
    pub status: String,
    pub online: bool,
    /// "owner", "admin", or the title the group gave them.
    pub role: Option<String>,
}

/// A media item: a gallery's square, or a row of files, links or voice messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaItem {
    pub message: i64,
    /// Photos and videos: the month's heading, "November 2026".
    pub group: Option<String>,
    /// The square's picture once downloaded (a small size).
    pub picture: Option<String>,
    /// A photo's full size once downloaded: the viewer's.
    pub full: Option<String>,
    /// The minithumbnail (a tiny JPEG, base64) while the picture isn't there.
    pub preview: Option<String>,
    /// A video's length, "GIF": in a pill with ▶.
    pub label: Option<String>,
    pub play: bool,
    /// A video's or a GIF's length in seconds (part 4: the square is a video Flux plays).
    #[serde(default)]
    pub duration: u32,
    /// The video is a GIF: plays by itself, without sound.
    #[serde(default)]
    pub gif: bool,
    /// A row: the file's name, the link's title, the voice's sender.
    pub title: String,
    /// The date: "12 Nov".
    pub detail: String,
    /// A row's second line: "1.2 MB", the site, "0:42".
    pub subtitle: String,
    /// A voice message's sender.
    pub avatar: Option<AvatarSpec>,
    /// A link's address.
    pub url: Option<String>,
    /// What a click opens, once downloaded: a video, a file, a voice message.
    pub path: Option<String>,
    /// What a click downloads, downloading: 0.0–1.0.
    pub progress: Option<f32>,
}

/// TDLib's updates the profiles need beyond the store's and the feed's: none — the store's users,
/// chats and files are theirs, and a profile's chat joins the helper's filter while on show.
pub const KEPT: &[&str] = &[];

/// What a request of the profiles asked TDLib: its answer comes to [`Profiles::answer`]. `epoch`
/// names the window's profile it was for: an answer for one gone is dropped.
#[derive(Debug)]
pub enum Ask {
    /// The profile's chat (`getChat`, `createPrivateChat`): the store took it.
    Chat { window: u64, epoch: u64 },
    /// A person (`getUser`): the phone; the store took the rest.
    User { window: u64, epoch: u64 },
    /// `getUserFullInfo`, `getBasicGroupFullInfo`, `getSupergroupFullInfo`.
    Full { window: u64, epoch: u64 },
    /// `getChatMessageCount` of a kind.
    Count {
        window: u64,
        epoch: u64,
        kind: MediaKind,
    },
    /// Members of a search (`epoch` — the members' own): the admins, or the rest.
    Members {
        window: u64,
        epoch: u64,
        admins: bool,
    },
    /// A page of media.
    Media {
        window: u64,
        epoch: u64,
        kind: MediaKind,
    },
    /// `setChatNotificationSettings`: the chat is asked again once it is done (its update may be
    /// filtered).
    Mute { key: ChatKey },
    /// Anything else: an error goes to the log.
    Done(&'static str),
}

/// A window's profile on show.
struct Shown {
    key: ChatKey,
    epoch: u64,
    /// The profile's large picture (`photo.big`): TDLib's file id.
    big_photo: Option<i64>,
    phone: Option<String>,
    about: Option<String>,
    /// A supergroup's count from its full info (the store's may lag).
    member_count: Option<u32>,
    /// A basic group's members, from its full info.
    basic_members: Option<Vec<Value>>,
    counts: BTreeMap<MediaKind, u32>,
    /// The group's info is asked (its chat came).
    asked_info: bool,
    sent: Option<Profile>,
    dirty: bool,
    members: Option<Members>,
    media: HashMap<MediaKind, MediaList>,
}

/// The members asked for.
struct Members {
    query: String,
    epoch: u64,
    /// A supergroup's admins (asked with an empty query only).
    admins: Option<Vec<Value>>,
    /// The recent ones, or the search's.
    found: Option<Vec<Value>>,
    problem: Option<String>,
    /// Users the store lacked, asked for.
    asked_users: HashSet<i64>,
    sent: Option<(Vec<Member>, Option<String>)>,
    dirty: bool,
}

/// The loaded media of a kind.
#[derive(Default)]
struct MediaList {
    epoch: u64,
    /// TDLib's messages, newest first.
    messages: Vec<Value>,
    /// `next_from_message_id`; 0 with `started` — no more.
    next: i64,
    started: bool,
    loading: bool,
    /// The items the window has, by message.
    sent: HashMap<i64, MediaItem>,
    /// The next send starts the window's list again.
    reset: bool,
    /// Every file the items show: a change of one converts them again.
    files: HashSet<i64>,
    dirty: bool,
}

impl MediaList {
    fn more(&self) -> bool {
        !self.started || self.next != 0
    }
}

pub struct Profiles {
    shown: HashMap<u64, Shown>,
    next_epoch: u64,
    /// Downloads asked, by account and file: asked once (a click asks again).
    downloads: HashSet<(String, i64)>,
}

impl Profiles {
    pub fn new() -> Self {
        Profiles {
            shown: HashMap::new(),
            next_epoch: 0,
            downloads: HashSet::new(),
        }
    }

    fn epoch(&mut self) -> u64 {
        self.next_epoch += 1;
        self.next_epoch
    }

    /// The chats of the account on show in a dialog: their updates pass the helper's filter.
    pub fn chats(&self, account: &str) -> Vec<i64> {
        self.shown
            .values()
            .filter(|shown| shown.key.account == account)
            .map(|shown| shown.key.chat)
            .collect()
    }

    /// The account is gone (signed out): what was asked for it is forgotten.
    pub fn account_gone(&mut self, account: &str) {
        self.shown.retain(|_, shown| shown.key.account != account);
        self.downloads.retain(|(of, _)| of != account);
    }

    /// An update of the account's client, after the store took it (`touched` — what it changed).
    pub fn update(&mut self, cx: &mut Cx, account: &str, update: &Value, touched: Touched) {
        for shown in self.shown.values_mut() {
            if shown.key.account != account {
                continue;
            }
            let chat = shown.key.chat;
            match touched {
                Touched::Nothing => {}
                Touched::Chat(id) if id == chat => shown.dirty = true,
                Touched::Chat(_) => {}
                Touched::User(user) => {
                    if person(cx.stores.get(account), chat) == Some(user) {
                        shown.dirty = true;
                        if update["@type"] == "updateUser" {
                            shown.phone = phone_of(&update["user"]);
                        }
                    }
                    if let Some(members) = &mut shown.members
                        && members.mentions(user)
                    {
                        members.dirty = true;
                    }
                    for list in shown.media.values_mut() {
                        list.dirty |= list
                            .messages
                            .iter()
                            .any(|message| message["sender_id"]["user_id"].as_i64() == Some(user));
                    }
                }
                Touched::File(file) => {
                    shown.dirty = true;
                    if let Some(members) = &mut shown.members {
                        members.dirty = true;
                    }
                    for list in shown.media.values_mut() {
                        list.dirty |= list.files.contains(&file);
                    }
                }
                Touched::Scope => shown.dirty = true,
            }
        }
    }

    /// The answer to a request of the profiles.
    pub fn answer(&mut self, cx: &mut Cx, account: &str, ask: Ask, answer: &Value) {
        let error = (answer["@type"] == "error").then(|| {
            format!(
                "{} {}",
                answer["code"].as_i64().unwrap_or_default(),
                answer["message"].as_str().unwrap_or_default()
            )
        });
        match ask {
            Ask::Chat { window, epoch } => {
                if let Some(error) = &error {
                    log::warn(&format!("The profile's chat: {error}"));
                }
                let Some(shown) = self
                    .shown
                    .get_mut(&window)
                    .filter(|shown| shown.epoch == epoch)
                else {
                    return;
                };
                shown.dirty = true;
                if error.is_none() {
                    let big = &answer["photo"]["big"];
                    shown.big_photo = big["id"].as_i64();
                    if downloaded(big).is_none()
                        && let Some(file) = shown.big_photo
                    {
                        download(cx, &mut self.downloads, account, file, AVATAR, false);
                    }
                }
                ask_info(cx, window, shown);
            }
            Ask::User { window, epoch } => {
                if let Some(shown) = self.current(window, epoch) {
                    if error.is_none() {
                        shown.phone = phone_of(answer);
                    }
                    shown.dirty = true;
                }
                // A member the store lacked.
                for shown in self.shown.values_mut() {
                    if let Some(members) = &mut shown.members
                        && answer["id"]
                            .as_i64()
                            .is_some_and(|id| members.asked_users.contains(&id))
                    {
                        members.dirty = true;
                    }
                }
            }
            Ask::Full { window, epoch } => {
                let Some(shown) = self.current(window, epoch) else {
                    return;
                };
                if let Some(error) = error {
                    log::warn(&format!("The profile's full info: {error}"));
                    return;
                }
                shown.about = about_of(answer);
                if let Some(count) = answer["member_count"].as_u64() {
                    shown.member_count = Some(count.min(u32::MAX as u64) as u32);
                }
                if answer["@type"] == "basicGroupFullInfo" {
                    shown.basic_members = answer["members"].as_array().cloned();
                    if let Some(members) = &mut shown.members {
                        members.dirty = true;
                    }
                }
                shown.dirty = true;
            }
            Ask::Count {
                window,
                epoch,
                kind,
            } => {
                let Some(shown) = self.current(window, epoch) else {
                    return;
                };
                match error {
                    Some(error) => log::warn(&format!("getChatMessageCount: {error}")),
                    None => {
                        let count = answer["count"].as_u64().unwrap_or_default();
                        shown.counts.insert(kind, count.min(u32::MAX as u64) as u32);
                        shown.dirty = true;
                    }
                }
            }
            Ask::Members {
                window,
                epoch,
                admins,
            } => {
                let Some(members) = self
                    .shown
                    .get_mut(&window)
                    .and_then(|shown| shown.members.as_mut())
                    .filter(|members| members.epoch == epoch)
                else {
                    return;
                };
                members.dirty = true;
                match error {
                    Some(error) => {
                        log::info(&format!("The members: {error}"));
                        members.problem =
                            Some(HostWords.tr("Telegram doesn't show the members of this group."));
                        if admins {
                            members.admins = Some(Vec::new());
                        } else {
                            members.found = Some(Vec::new());
                        }
                    }
                    None => {
                        let list = answer["members"].as_array().cloned().unwrap_or_default();
                        if admins {
                            members.admins = Some(list);
                        } else {
                            members.found = Some(list);
                        }
                    }
                }
            }
            Ask::Media {
                window,
                epoch,
                kind,
            } => {
                let Some(list) = self
                    .shown
                    .get_mut(&window)
                    .and_then(|shown| shown.media.get_mut(&kind))
                    .filter(|list| list.epoch == epoch)
                else {
                    return;
                };
                list.loading = false;
                list.started = true;
                list.dirty = true;
                match error {
                    Some(error) => {
                        log::warn(&format!("searchChatMessages: {error}"));
                        list.next = 0;
                    }
                    None => {
                        let page = answer["messages"].as_array().cloned().unwrap_or_default();
                        let known: HashSet<i64> = list
                            .messages
                            .iter()
                            .filter_map(|message| message["id"].as_i64())
                            .collect();
                        let fresh: Vec<Value> = page
                            .into_iter()
                            .filter(|message| {
                                message["id"]
                                    .as_i64()
                                    .is_some_and(|id| !known.contains(&id))
                            })
                            .collect();
                        let next = answer["next_from_message_id"].as_i64().unwrap_or_default();
                        // No new messages: the end, whatever TDLib says next.
                        list.next = if fresh.is_empty() { 0 } else { next };
                        list.messages.extend(fresh);
                    }
                }
            }
            Ask::Mute { key } => match error {
                Some(error) => log::warn(&format!("setChatNotificationSettings: {error}")),
                None => cx.td.send(
                    &key.account,
                    json!({"@type": "getChat", "chat_id": key.chat}),
                    Asked::Profile(Ask::Done("getChat")),
                ),
            },
            Ask::Done(what) => {
                if let Some(error) = error {
                    log::warn(&format!("{what} of {account}: {error}"));
                }
            }
        }
        // The chat or a user asked for again: the profiles showing it look again.
        if matches!(answer["@type"].as_str(), Some("chat")) {
            let chat = answer["id"].as_i64().unwrap_or_default();
            for shown in self.shown.values_mut() {
                shown.dirty |= shown.key.account == account && shown.key.chat == chat;
            }
        }
    }

    /// The window's profile, when `epoch` is still the one on show.
    fn current(&mut self, window: u64, epoch: u64) -> Option<&mut Shown> {
        self.shown
            .get_mut(&window)
            .filter(|shown| shown.epoch == epoch)
    }

    /// A window's request (`Request::Profile`).
    pub fn request(&mut self, cx: &mut Cx, window: u64, request: ProfileAsk) {
        match request {
            ProfileAsk::Open { key } => self.open(cx, window, key),
            ProfileAsk::Close => {
                self.shown.remove(&window);
            }
            ProfileAsk::Members { key, query } => {
                let epoch = self.epoch();
                let Some(shown) = self.shown.get_mut(&window).filter(|s| s.key == key) else {
                    return;
                };
                shown.members = Some(Members {
                    query: query.trim().to_string(),
                    epoch,
                    admins: None,
                    found: None,
                    problem: None,
                    asked_users: HashSet::new(),
                    sent: None,
                    dirty: true,
                });
                ask_members(cx, window, shown);
            }
            ProfileAsk::Media { key, kind, more } => {
                let epoch = self.epoch();
                let Some(shown) = self.shown.get_mut(&window).filter(|s| s.key == key) else {
                    return;
                };
                let list = shown.media.entry(kind).or_default();
                if !more {
                    *list = MediaList {
                        epoch,
                        reset: true,
                        dirty: true,
                        ..MediaList::default()
                    };
                } else if list.loading || !list.more() {
                    return;
                }
                list.loading = true;
                cx.td.send(
                    &key.account,
                    json!({"@type": "searchChatMessages", "chat_id": key.chat, "topic_id": null,
                        "query": "", "sender_id": null, "from_message_id": list.next, "offset": 0,
                        "limit": PAGE, "filter": {"@type": kind.filter()}}),
                    Asked::Profile(Ask::Media {
                        window,
                        epoch: list.epoch,
                        kind,
                    }),
                );
            }
            ProfileAsk::Visible {
                key,
                kind,
                first,
                last,
            } => {
                let Some(list) = self
                    .shown
                    .get(&window)
                    .filter(|shown| shown.key == key)
                    .and_then(|shown| shown.media.get(&kind))
                else {
                    return;
                };
                let position = |id: i64| {
                    list.messages
                        .iter()
                        .position(|message| message["id"].as_i64() == Some(id))
                };
                let (Some(a), Some(b)) = (position(first), position(last)) else {
                    return;
                };
                let files: Vec<i64> = list.messages[a.min(b)..=a.max(b)]
                    .iter()
                    .filter_map(|message| square_file(&message["content"]))
                    .collect();
                for (index, file) in files.into_iter().enumerate() {
                    // The first squares in sight first.
                    let priority = (IN_SIGHT - index as i64 / 4).max(8);
                    download(cx, &mut self.downloads, &key.account, file, priority, false);
                }
            }
            ProfileAsk::Download { key, kind, message } => {
                let file = self
                    .shown
                    .get(&window)
                    .filter(|shown| shown.key == key)
                    .and_then(|shown| shown.media.get(&kind))
                    .and_then(|list| {
                        list.messages
                            .iter()
                            .find(|found| found["id"].as_i64() == Some(message))
                    })
                    .and_then(|found| clicked_file(&found["content"]));
                if let Some(file) = file {
                    download(cx, &mut self.downloads, &key.account, file, CLICKED, true);
                }
            }
            ProfileAsk::Mute { key, muted } => {
                cx.td.send(
                    &key.account,
                    json!({"@type": "setChatNotificationSettings", "chat_id": key.chat,
                        "notification_settings": notification_settings(muted)}),
                    Asked::Profile(Ask::Mute { key: key.clone() }),
                );
            }
            ProfileAsk::MuteFor { key, seconds } => {
                let mut settings = notification_settings(true);
                settings["mute_for"] = json!(seconds.clamp(0, FOREVER));
                cx.td.send(
                    &key.account,
                    json!({"@type": "setChatNotificationSettings", "chat_id": key.chat,
                        "notification_settings": settings}),
                    Asked::Profile(Ask::Mute { key: key.clone() }),
                );
            }
        }
    }

    /// A profile opens in the window: its chat (made for a person without one), the person or the
    /// group, the counts of its media.
    fn open(&mut self, cx: &mut Cx, window: u64, key: ChatKey) {
        let epoch = self.epoch();
        let mut shown = Shown {
            key: key.clone(),
            epoch,
            big_photo: None,
            phone: None,
            about: None,
            member_count: None,
            basic_members: None,
            counts: BTreeMap::new(),
            asked_info: false,
            sent: None,
            dirty: true,
            members: None,
            media: HashMap::new(),
        };
        let store = cx.stores.get(&key.account);
        let known = store.is_some_and(|store| store.chats.contains_key(&key.chat));
        // TDLib's current chat (its large picture too); a person's chat is made if there is none
        // (locally: Message opens it then).
        let request = if key.chat > 0 && !known {
            json!({"@type": "createPrivateChat", "user_id": key.chat, "force": false})
        } else {
            json!({"@type": "getChat", "chat_id": key.chat})
        };
        cx.td.send(
            &key.account,
            request,
            Asked::Profile(Ask::Chat { window, epoch }),
        );
        if known {
            ask_info(cx, window, &mut shown);
        }
        for kind in MediaKind::ALL {
            cx.td.send(
                &key.account,
                json!({"@type": "getChatMessageCount", "chat_id": key.chat, "topic_id": null,
                    "filter": {"@type": kind.filter()}, "return_local": false}),
                Asked::Profile(Ask::Count {
                    window,
                    epoch,
                    kind,
                }),
            );
        }
        self.shown.insert(window, shown);
    }

    /// A window's instance stopped: what it asked for goes.
    pub fn window_closed(&mut self, window: u64) {
        self.shown.remove(&window);
    }

    /// The end of a batch: what changed goes to the windows that show it.
    pub fn flush(&mut self, cx: &mut Cx) {
        if self.shown.is_empty() {
            return;
        }
        let now = unix_now() as i64;
        let offset = system::utc_offset(now);
        let at = When { now, offset };
        // The members' pictures, downloaded after the lists are made.
        let mut pictures: Vec<(String, i64)> = Vec::new();
        for (&window, shown) in self.shown.iter_mut() {
            let Some(store) = cx.stores.get(&shown.key.account) else {
                continue;
            };
            if shown.dirty {
                shown.dirty = false;
                let profile = profile(store, shown, at, &HostWords);
                if shown.sent.as_ref() != Some(&profile) {
                    tell(
                        window,
                        ProfileTold::Profile {
                            key: shown.key.clone(),
                            profile: profile.clone(),
                        },
                    );
                    shown.sent = Some(profile);
                }
            }
            let group = matches!(
                store.chats.get(&shown.key.chat).map(|chat| chat.kind),
                Some(ChatKind::BasicGroup { .. } | ChatKind::Supergroup { .. })
            );
            if let Some(members) = shown.members.as_mut().filter(|members| members.dirty) {
                let values = match &shown.basic_members {
                    _ if !group => None,
                    Some(basic) if members.found.is_none() => Some(basic.clone()),
                    _ => members.ready(),
                };
                if let Some(values) = values {
                    members.dirty = false;
                    let list = members_of(store, &values, &members.query, at, &HostWords);
                    // The pictures of the first rows (a list has no rows in sight to tell).
                    let photos: Vec<i64> = list
                        .iter()
                        .take(AVATARS)
                        .filter_map(|member| store.users.get(&member.user)?.photo)
                        .filter(|photo| store.file_path(*photo).is_none())
                        .collect();
                    pictures.extend(
                        photos
                            .into_iter()
                            .map(|photo| (shown.key.account.clone(), photo)),
                    );
                    // Users the store lacks: asked for, once.
                    for user in member_ids(&values) {
                        if !store.users.contains_key(&user) && members.asked_users.insert(user) {
                            cx.td.send(
                                &shown.key.account,
                                json!({"@type": "getUser", "user_id": user}),
                                Asked::Profile(Ask::User { window, epoch: 0 }),
                            );
                        }
                    }
                    let sent = (list, members.problem.clone());
                    if members.sent.as_ref() != Some(&sent) {
                        tell(
                            window,
                            ProfileTold::Members {
                                key: shown.key.clone(),
                                query: members.query.clone(),
                                members: sent.0.clone(),
                                problem: sent.1.clone(),
                            },
                        );
                        members.sent = Some(sent);
                    }
                }
            }
            for (&kind, list) in shown.media.iter_mut() {
                if !list.dirty || (list.loading && list.messages.is_empty()) {
                    continue;
                }
                list.dirty = false;
                let mut changed = Vec::new();
                list.files.clear();
                for message in &list.messages {
                    let Some((item, files)) = media_item(store, message, kind, at, &HostWords)
                    else {
                        continue;
                    };
                    list.files.extend(files);
                    if list.sent.get(&item.message) != Some(&item) {
                        list.sent.insert(item.message, item.clone());
                        changed.push(item);
                    }
                }
                if changed.is_empty() && !list.reset && list.started {
                    // Nothing new, but the end may be: told with an empty change.
                    if !list.more() {
                        tell(
                            window,
                            ProfileTold::Media {
                                key: shown.key.clone(),
                                kind,
                                items: Vec::new(),
                                more: false,
                                reset: false,
                            },
                        );
                    }
                    continue;
                }
                tell(
                    window,
                    ProfileTold::Media {
                        key: shown.key.clone(),
                        kind,
                        items: changed,
                        more: list.more(),
                        reset: std::mem::take(&mut list.reset),
                    },
                );
            }
        }
        for (account, photo) in pictures {
            download(cx, &mut self.downloads, &account, photo, 4, false);
        }
    }
}

impl Members {
    /// The user is among the members shown.
    fn mentions(&self, user: i64) -> bool {
        self.sent
            .as_ref()
            .is_some_and(|(members, _)| members.iter().any(|member| member.user == user))
            || self.asked_users.contains(&user)
    }

    /// The members once every part of the request came: a supergroup's admins first.
    fn ready(&self) -> Option<Vec<Value>> {
        let found = self.found.as_ref()?;
        if self.query.is_empty() {
            let mut all = self.admins.clone()?;
            all.extend(found.iter().cloned());
            Some(all)
        } else {
            Some(found.clone())
        }
    }
}

fn tell(window: u64, told: ProfileTold) {
    let _ = service::send_json_to(window, &Note::Profile(told));
}

/// The person (TDLib's group info, the members) of a profile once its chat is known.
fn ask_info(cx: &mut Cx, window: u64, shown: &mut Shown) {
    if shown.asked_info {
        return;
    }
    let key = shown.key.clone();
    let kind = cx
        .stores
        .get(&key.account)
        .and_then(|store| store.chats.get(&key.chat))
        .map(|chat| chat.kind);
    let epoch = shown.epoch;
    let full = |request: Value| (request, Asked::Profile(Ask::Full { window, epoch }));
    let (request, asked) = match kind {
        Some(ChatKind::Private { user } | ChatKind::Secret { user }) => {
            cx.td.send(
                &key.account,
                json!({"@type": "getUser", "user_id": user}),
                Asked::Profile(Ask::User { window, epoch }),
            );
            full(json!({"@type": "getUserFullInfo", "user_id": user}))
        }
        Some(ChatKind::BasicGroup { group }) => {
            full(json!({"@type": "getBasicGroupFullInfo", "basic_group_id": group}))
        }
        Some(ChatKind::Supergroup { group, .. }) => {
            full(json!({"@type": "getSupergroupFullInfo", "supergroup_id": group}))
        }
        _ => return,
    };
    shown.asked_info = true;
    cx.td.send(&key.account, request, asked);
    if shown.members.is_some() {
        ask_members(cx, window, shown);
    }
}

/// The members of a profile's group: a basic group's come with its full info (a query filters
/// them here); a supergroup's are asked — the admins and the recent ones, or a search.
fn ask_members(cx: &mut Cx, window: u64, shown: &mut Shown) {
    let Some(members) = &mut shown.members else {
        return;
    };
    let key = &shown.key;
    let kind = cx
        .stores
        .get(&key.account)
        .and_then(|store| store.chats.get(&key.chat))
        .map(|chat| chat.kind);
    let epoch = members.epoch;
    match kind {
        Some(ChatKind::Supergroup { group, .. }) if members.query.is_empty() => {
            for (filter, admins) in [
                ("supergroupMembersFilterAdministrators", true),
                ("supergroupMembersFilterRecent", false),
            ] {
                cx.td.send(
                    &key.account,
                    json!({"@type": "getSupergroupMembers", "supergroup_id": group,
                        "filter": {"@type": filter}, "offset": 0, "limit": MEMBERS}),
                    Asked::Profile(Ask::Members {
                        window,
                        epoch,
                        admins,
                    }),
                );
            }
        }
        Some(ChatKind::Supergroup { .. }) => cx.td.send(
            &key.account,
            json!({"@type": "searchChatMembers", "chat_id": key.chat, "query": members.query,
                "limit": FOUND, "filter": null}),
            Asked::Profile(Ask::Members {
                window,
                epoch,
                admins: false,
            }),
        ),
        // A basic group: its full info has them; a chat not known yet asks once it is.
        _ => members.dirty = true,
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
        Asked::Profile(Ask::Done("downloadFile")),
    );
}

/// TDLib's `chatNotificationSettings` that mute the chat for good, or unmute it; the rest is the
/// default's.
pub fn notification_settings(muted: bool) -> Value {
    json!({"@type": "chatNotificationSettings",
        "use_default_mute_for": false, "mute_for": if muted { FOREVER } else { 0 },
        "use_default_sound": true, "sound_id": "0",
        "use_default_show_preview": true, "show_preview": false,
        "use_default_mute_stories": true, "mute_stories": false,
        "use_default_story_sound": true, "story_sound_id": "0",
        "use_default_show_story_poster": true, "show_story_poster": false,
        "use_default_show_story_sender": true, "show_story_sender": false,
        "use_default_disable_pinned_message_notifications": true,
        "disable_pinned_message_notifications": false,
        "use_default_disable_mention_notifications": true,
        "disable_mention_notifications": false})
}

// --- What the dialog shows ---

/// Now, and the user's offset from UTC: the dates' words.
#[derive(Debug, Clone, Copy)]
pub struct When {
    pub now: i64,
    pub offset: i32,
}

/// The person of a profile's chat: a person's chat, or a person without a chat yet (their id).
fn person(store: Option<&Store>, chat: i64) -> Option<i64> {
    match store
        .and_then(|store| store.chats.get(&chat))
        .map(|chat| chat.kind)
    {
        Some(ChatKind::Private { user } | ChatKind::Secret { user }) => Some(user),
        None if chat > 0 => Some(chat),
        _ => None,
    }
}

/// The number Telegram shows: "+79001234567"; none — hidden.
fn phone_of(user: &Value) -> Option<String> {
    let digits = user["phone_number"].as_str().unwrap_or_default().trim();
    (!digits.is_empty()).then(|| format!("+{}", digits.trim_start_matches('+')))
}

/// A person's bio or a group's description (TDLib 1.8.61: a `formattedText`; older — a string).
fn about_of(full: &Value) -> Option<String> {
    let about = match full["@type"].as_str().unwrap_or_default() {
        "userFullInfo" => &full["bio"],
        _ => &full["description"],
    };
    let text = about["text"].as_str().or_else(|| about.as_str())?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The profile as the dialog shows it.
fn profile(store: &Store, shown: &Shown, at: When, words: &dyn Words) -> Profile {
    let chat = shown.key.chat;
    let found = store.chats.get(&chat);
    let user = person(Some(store), chat);
    let saved = store.is_saved_messages(chat);
    let title = match (found, user) {
        (Some(_), _) => chats::title(store, chat, words),
        (None, Some(user)) => match store.user_name(user) {
            name if name.is_empty() => words.tr("Deleted Account"),
            name => name,
        },
        _ => String::new(),
    };
    let mut avatar = match (found, user) {
        (None, Some(user)) => store.user_avatar(user),
        _ => store.chat_avatar(chat),
    };
    if avatar.initials.is_empty() || saved {
        avatar.initials = crate::model::initials(&title);
    }
    if let Some(path) = shown.big_photo.and_then(|file| store.file_path(file)) {
        avatar.image = Some(path.to_string());
    }
    let group = store.group(chat);
    let is_group = matches!(
        found.map(|chat| chat.kind),
        Some(ChatKind::BasicGroup { .. } | ChatKind::Supergroup { .. })
    );
    let members = shown
        .member_count
        .or(group.map(|group| group.member_count))
        .unwrap_or_default();
    let (status, online) = if saved {
        (String::new(), false)
    } else if store.is_channel(chat) {
        // A channel's subscribers (part 5).
        (crate::feed::subscribers_line(members, words), false)
    } else if is_group {
        let line = if members == 0 {
            words.tr("group")
        } else {
            words.count(u64::from(members), "{0} member", "{0} members")
        };
        (line, false)
    } else {
        user.map_or((String::new(), false), |user| {
            person_status(store, user, at, words)
        })
    };
    let username = if is_group {
        None
    } else {
        user.and_then(|user| store.users.get(&user))
            .and_then(|user| user.username.clone())
            .map(|name| format!("@{name}"))
    };
    let link = group
        .filter(|_| is_group)
        .and_then(|group| group.username.clone())
        .map(|name| format!("t.me/{name}"));
    let can_message = match found {
        Some(_) => store.can_send(chat) || !is_group,
        None => user.is_some_and(|user| {
            store
                .users
                .get(&user)
                .is_none_or(|found| found.kind != UserKind::Deleted)
        }),
    };
    Profile {
        title,
        avatar,
        status,
        online,
        group: is_group,
        username,
        phone: shown.phone.clone().filter(|_| !is_group && !saved),
        about: shown.about.clone(),
        link,
        members,
        muted: store.muted(chat),
        can_message,
        counts: shown
            .counts
            .iter()
            .map(|(kind, count)| (*kind, *count))
            .collect(),
    }
}

/// A person's line: "online" (and that it is), "last seen…", "bot".
pub fn person_status(store: &Store, user: i64, at: When, words: &dyn Words) -> (String, bool) {
    let Some(user) = store.users.get(&user) else {
        return (String::new(), false);
    };
    match user.kind {
        UserKind::Bot => return (words.tr("bot"), false),
        UserKind::Deleted => return (String::new(), false),
        _ => {}
    }
    let line = match user.status {
        Status::Online { expires } if expires > at.now => return (words.tr("online"), true),
        Status::Online { expires } => last_seen(expires, at, words),
        Status::Offline { was } => last_seen(was, at, words),
        Status::Recently => words.tr("last seen recently"),
        Status::LastWeek => words.tr("last seen within a week"),
        Status::LastMonth => words.tr("last seen within a month"),
        Status::Empty => words.tr("last seen a long time ago"),
    };
    (line, false)
}

/// "last seen just now", "… 5 minutes ago", "… at 14:32", "… yesterday at 14:32", "… 12.11.26"
/// — the chat header's words.
fn last_seen(was: i64, at: When, words: &dyn Words) -> String {
    let ago = at.now - was;
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
    let then = system::LocalTime::at(was, at.offset);
    let day = |moment: i64| (moment + i64::from(at.offset)).div_euclid(86_400);
    let time = format!("{}:{:02}", then.hour, then.minute);
    match day(at.now) - day(was) {
        0 => words.trf("last seen at {0}", &[&time]),
        1 => words.trf("last seen yesterday at {0}", &[&time]),
        _ => {
            let date = format!("{:02}.{:02}.{:02}", then.day, then.month, then.year % 100);
            words.trf("last seen {0}", &[&date])
        }
    }
}

/// The user ids of TDLib's `chatMember`s (chats as members are skipped).
fn member_ids(values: &[Value]) -> Vec<i64> {
    values
        .iter()
        .filter_map(|member| member["member_id"]["user_id"].as_i64())
        .collect()
}

/// The members as the dialog lists them: the owner, the admins, then the rest in Telegram's
/// order, each once; those who left or were banned aren't; `query` filters by the name or the
/// username (a supergroup's search has done it already — it passes again).
fn members_of(
    store: &Store,
    values: &[Value],
    query: &str,
    at: When,
    words: &dyn Words,
) -> Vec<Member> {
    let query = query.trim().to_lowercase();
    let mut seen = HashSet::new();
    let mut members: Vec<(u8, Member)> = Vec::new();
    for value in values {
        let Some(user) = value["member_id"]["user_id"].as_i64() else {
            continue;
        };
        let status = &value["status"];
        let custom = status["custom_title"]
            .as_str()
            .filter(|title| !title.trim().is_empty())
            .map(str::to_string);
        let (rank, role) = match status["@type"].as_str().unwrap_or_default() {
            "chatMemberStatusCreator" => (0, Some(custom.unwrap_or_else(|| words.tr("owner")))),
            "chatMemberStatusAdministrator" => {
                (1, Some(custom.unwrap_or_else(|| words.tr("admin"))))
            }
            "chatMemberStatusLeft" | "chatMemberStatusBanned" => continue,
            "chatMemberStatusRestricted" if status["is_member"] == false => continue,
            _ => (2, None),
        };
        if !seen.insert(user) {
            continue;
        }
        let found = store.users.get(&user);
        let name = match store.user_name(user) {
            name if name.is_empty() && found.is_some() => words.tr("Deleted Account"),
            name => name,
        };
        if !query.is_empty() {
            let username = found
                .and_then(|found| found.username.as_deref())
                .unwrap_or_default()
                .to_lowercase();
            let lower = name.to_lowercase();
            let matches = lower.contains(&query)
                || lower
                    .split_whitespace()
                    .any(|word| word.starts_with(&query))
                || username.starts_with(query.trim_start_matches('@'));
            if !matches {
                continue;
            }
        }
        let (status, online) = person_status(store, user, at, words);
        members.push((
            rank,
            Member {
                user,
                name,
                avatar: store.user_avatar(user),
                status,
                online,
                role,
            },
        ));
    }
    // Stable: Telegram's order within a rank.
    members.sort_by_key(|(rank, _)| *rank);
    members.into_iter().map(|(_, member)| member).collect()
}

// --- The media ---

/// The sizes of a `photo` with a file, by their longest side.
fn photo_sizes(photo: &Value) -> Vec<(i64, &Value)> {
    let mut sizes: Vec<(i64, &Value)> = photo["sizes"]
        .as_array()
        .map(|sizes| {
            sizes
                .iter()
                .filter(|size| size["photo"]["id"].is_i64())
                .map(|size| {
                    let side = size["width"]
                        .as_i64()
                        .unwrap_or_default()
                        .max(size["height"].as_i64().unwrap_or_default());
                    (side, size)
                })
                .collect()
        })
        .unwrap_or_default();
    sizes.sort_by_key(|(side, _)| *side);
    sizes
}

/// A photo's square (the smallest size of at least [`SQUARE_SIDE`], else the largest) and its
/// full size, as files.
fn photo_files(photo: &Value) -> Option<(&Value, &Value)> {
    let sizes = photo_sizes(photo);
    let largest = sizes.last()?.1;
    let square = sizes
        .iter()
        .find(|(side, _)| *side >= SQUARE_SIDE)
        .map_or(largest, |(_, size)| *size);
    Some((&square["photo"], &largest["photo"]))
}

/// A video's or a GIF's still thumbnail, as a file.
fn thumbnail_file(media: &Value) -> Option<&Value> {
    let thumbnail = &media["thumbnail"];
    let still = matches!(
        thumbnail["format"]["@type"].as_str().unwrap_or_default(),
        "thumbnailFormatJpeg" | "thumbnailFormatPng" | "thumbnailFormatWebp" | "thumbnailFormatGif"
    );
    (still && thumbnail["file"]["id"].is_i64()).then_some(&thumbnail["file"])
}

/// The square's file of a gallery's message (what downloads in sight).
fn square_file(content: &Value) -> Option<i64> {
    match content["@type"].as_str().unwrap_or_default() {
        "messagePhoto" => {
            photo_files(&content["photo"]).and_then(|(square, _)| square["id"].as_i64())
        }
        "messageVideo" => thumbnail_file(&content["video"]).and_then(|file| file["id"].as_i64()),
        "messageAnimation" => {
            thumbnail_file(&content["animation"]).and_then(|file| file["id"].as_i64())
        }
        _ => None,
    }
}

/// What a click on an item downloads: a photo's full size, the video, the file, the voice.
fn clicked_file(content: &Value) -> Option<i64> {
    let file = match content["@type"].as_str().unwrap_or_default() {
        "messagePhoto" => photo_files(&content["photo"])?.1,
        "messageVideo" => &content["video"]["video"],
        "messageAnimation" => &content["animation"]["animation"],
        "messageDocument" => &content["document"]["document"],
        "messageVoiceNote" => &content["voice_note"]["voice"],
        "messageAudio" => &content["audio"]["audio"],
        _ => return None,
    };
    file["id"].as_i64()
}

/// A file's path: the store's knowledge first (it follows `updateFile`), else the message's.
fn path_of(store: &Store, file: &Value) -> Option<String> {
    match file["id"].as_i64().and_then(|id| store.files.get(&id)) {
        Some(known) => known.path.clone(),
        None => downloaded(file),
    }
}

/// A download's progress (or an upload's).
fn progress_of(store: &Store, file: &Value) -> Option<f32> {
    store.files.get(&file["id"].as_i64()?)?.progress()
}

/// The size of a file, the store's first.
fn size_of(store: &Store, file: &Value) -> u64 {
    file["id"]
        .as_i64()
        .and_then(|id| store.files.get(&id))
        .map(|known| known.size)
        .filter(|size| *size > 0)
        .or_else(|| file["size"].as_u64().filter(|size| *size > 0))
        .or_else(|| file["expected_size"].as_u64())
        .unwrap_or_default()
}

/// A message of the chat's media as the dialog shows it, and the files it shows (a change of one
/// converts it again); none — a message of another kind.
pub fn media_item(
    store: &Store,
    message: &Value,
    kind: MediaKind,
    at: When,
    words: &dyn Words,
) -> Option<(MediaItem, Vec<i64>)> {
    let id = message["id"].as_i64()?;
    let date = message["date"].as_i64().unwrap_or_default();
    let content = &message["content"];
    let mut item = MediaItem {
        message: id,
        group: None,
        picture: None,
        full: None,
        preview: None,
        label: None,
        play: false,
        duration: 0,
        gif: false,
        title: String::new(),
        detail: short_date(date, at, words),
        subtitle: String::new(),
        avatar: None,
        url: None,
        path: None,
        progress: None,
    };
    let mut files = Vec::new();
    let mut follow = |file: &Value| {
        if let Some(file) = file["id"].as_i64() {
            files.push(file);
        }
    };
    let content_kind = content["@type"].as_str().unwrap_or_default();
    match (kind, content_kind) {
        (MediaKind::Photos, "messagePhoto") => {
            let photo = &content["photo"];
            let (square, full) = photo_files(photo)?;
            follow(square);
            follow(full);
            item.picture = path_of(store, square);
            if full["id"] != square["id"] {
                item.full = path_of(store, full);
            }
            item.progress = progress_of(store, full);
            if item.picture.is_none() {
                item.preview = minithumbnail(&photo["minithumbnail"]);
            }
        }
        (MediaKind::Photos, "messageVideo" | "messageAnimation") => {
            let animation = content_kind == "messageAnimation";
            let media = if animation {
                &content["animation"]
            } else {
                &content["video"]
            };
            let file = if animation {
                &media["animation"]
            } else {
                &media["video"]
            };
            follow(file);
            if let Some(thumbnail) = thumbnail_file(media) {
                follow(thumbnail);
                item.picture = path_of(store, thumbnail);
            }
            if item.picture.is_none() {
                item.preview = minithumbnail(&media["minithumbnail"]);
            }
            item.label = Some(if animation {
                words.tr("GIF")
            } else {
                convert::duration(media["duration"].as_i64().unwrap_or_default())
            });
            item.play = true;
            item.duration = media["duration"].as_u64().unwrap_or_default() as u32;
            item.gif = animation;
            item.path = path_of(store, file);
            item.progress = progress_of(store, file);
        }
        (MediaKind::Files, "messageDocument" | "messageAudio") => {
            let (file, name) = if content_kind == "messageAudio" {
                let audio = &content["audio"];
                let name = audio["file_name"].as_str().unwrap_or_default();
                (&audio["audio"], name)
            } else {
                let document = &content["document"];
                let name = document["file_name"].as_str().unwrap_or_default();
                (&document["document"], name)
            };
            follow(file);
            item.title = if name.trim().is_empty() {
                words.tr("File")
            } else {
                name.to_string()
            };
            item.subtitle = size_text(size_of(store, file), words);
            item.path = path_of(store, file);
            item.progress = progress_of(store, file);
        }
        (MediaKind::Links, _) => {
            let (url, title, about) = link_of(content)?;
            item.title = title.unwrap_or_else(|| bare(&url));
            item.subtitle = about.unwrap_or_else(|| host(&url));
            item.url = Some(url);
        }
        (MediaKind::Voice, "messageVoiceNote") => {
            let voice = &content["voice_note"];
            let file = &voice["voice"];
            follow(file);
            let sender = summary::sender(store, message, words);
            item.title = if message["is_outgoing"] == true {
                words.tr("You")
            } else {
                sender.name.clone()
            };
            item.avatar = Some(if sender.user {
                store.user_avatar(sender.id)
            } else {
                store.chat_avatar(sender.id)
            });
            if let Some(photo) = sender
                .user
                .then(|| store.users.get(&sender.id))
                .flatten()
                .and_then(|user| user.photo)
            {
                follow(&json!({ "id": photo }));
            }
            item.subtitle = convert::duration(voice["duration"].as_i64().unwrap_or_default());
            item.path = path_of(store, file);
            item.progress = progress_of(store, file);
        }
        _ => return None,
    }
    if kind == MediaKind::Photos {
        item.group = Some(month_heading(date, at.offset, words));
    }
    Some((item, files))
}

/// The tiny JPEG of a `minithumbnail`, base64 as it is in TDLib's JSON.
fn minithumbnail(mini: &Value) -> Option<String> {
    let data = mini["data"].as_str()?;
    // Checked here: the window decodes it without asking.
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()
        .map(|_| data.to_string())
}

/// A link of a message: its preview's address, title and site (or description); else the first
/// address in its text (an entity's, or a bare `http…`), with the text's first line.
fn link_of(content: &Value) -> Option<(String, Option<String>, Option<String>)> {
    let text = |value: &Value| {
        value
            .as_str()
            .or_else(|| value["text"].as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let preview = &content["link_preview"];
    if let Some(url) = preview["url"].as_str().filter(|url| !url.is_empty()) {
        let about = text(&preview["site_name"]).or_else(|| text(&preview["description"]));
        return Some((url.to_string(), text(&preview["title"]), about));
    }
    let formatted = if content["text"].is_object() {
        &content["text"]
    } else {
        &content["caption"]
    };
    let body = formatted["text"].as_str().unwrap_or_default();
    let url = formatted["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|entity| {
            let kind = &entity["type"];
            match kind["@type"].as_str().unwrap_or_default() {
                "textEntityTypeTextUrl" => kind["url"].as_str().map(str::to_string),
                "textEntityTypeUrl" => {
                    let offset = entity["offset"].as_u64().unwrap_or_default() as usize;
                    let length = entity["length"].as_u64().unwrap_or_default() as usize;
                    let units: Vec<u16> = body.encode_utf16().skip(offset).take(length).collect();
                    Some(String::from_utf16_lossy(&units))
                }
                _ => None,
            }
        })
        .or_else(|| {
            body.split_whitespace()
                .find(|word| word.starts_with("http://") || word.starts_with("https://"))
                .map(str::to_string)
        })?;
    let url = if url.contains("://") {
        url
    } else {
        format!("https://{url}")
    };
    let line = summary::first_line(body);
    let about = (!line.is_empty() && line != url && line != bare(&url)).then_some(line);
    Some((url, None, about))
}

/// An address without its scheme: "example.com/news".
fn bare(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .trim_end_matches('/')
        .to_string()
}

/// An address's site: "example.com".
fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

/// The month's heading of a gallery: "November 2026".
pub fn month_heading(at: i64, offset: i32, words: &dyn Words) -> String {
    const MONTHS: [&str; 12] = [
        "January {0}",
        "February {0}",
        "March {0}",
        "April {0}",
        "May {0}",
        "June {0}",
        "July {0}",
        "August {0}",
        "September {0}",
        "October {0}",
        "November {0}",
        "December {0}",
    ];
    let time = system::LocalTime::at(at, offset);
    let month = MONTHS[(time.month.clamp(1, 12) - 1) as usize];
    words.trf(month, &[&time.year.to_string()])
}

/// A row's date: "12 Nov"; another year's "12 Nov 2025".
pub fn short_date(at: i64, when: When, words: &dyn Words) -> String {
    const MONTHS: [&str; 12] = [
        "{0} Jan", "{0} Feb", "{0} Mar", "{0} Apr", "{0} May", "{0} Jun", "{0} Jul", "{0} Aug",
        "{0} Sep", "{0} Oct", "{0} Nov", "{0} Dec",
    ];
    if at <= 0 {
        return String::new();
    }
    let time = system::LocalTime::at(at, when.offset);
    let this_year = system::LocalTime::at(when.now, when.offset).year;
    let month = MONTHS[(time.month.clamp(1, 12) - 1) as usize];
    let date = words.trf(month, &[&time.day.to_string()]);
    if time.year == this_year {
        date
    } else {
        format!("{date} {}", time.year)
    }
}

/// A file's size: "512 B", "18 KB", "1.2 MB" (the interface's decimal separator).
pub fn size_text(bytes: u64, words: &dyn Words) -> String {
    const KB: u64 = 1024;
    let tenths = |unit: u64| {
        let tenths = (bytes * 10 + unit / 2) / unit;
        let point = words.tr(".");
        if tenths.is_multiple_of(10) || tenths >= 1000 {
            (tenths / 10).to_string()
        } else {
            format!("{}{point}{}", tenths / 10, tenths % 10)
        }
    };
    match bytes {
        0 => String::new(),
        _ if bytes < KB => words.trf("{0} B", &[&bytes.to_string()]),
        _ if bytes < KB * KB => words.trf("{0} KB", &[&bytes.div_ceil(KB).to_string()]),
        _ if bytes < KB * KB * KB => words.trf("{0} MB", &[&tenths(KB * KB)]),
        _ => words.trf("{0} GB", &[&tenths(KB * KB * KB)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{English, Request};

    const NOW: i64 = 1_763_380_000;
    const AT: When = When {
        now: NOW,
        offset: 3 * 3600,
    };

    fn store() -> Store {
        let mut store = Store {
            me: Some(777),
            ..Store::default()
        };
        for user in [
            json!({"@type": "user", "id": 101, "first_name": "Anna", "last_name": "Smirnova",
                "usernames": {"active_usernames": ["anna"]},
                "status": {"@type": "userStatusOnline", "expires": NOW + 300}}),
            json!({"@type": "user", "id": 102, "first_name": "Dima", "last_name": "Kim",
                "status": {"@type": "userStatusOffline", "was_online": NOW - 1800}}),
            json!({"@type": "user", "id": 103, "first_name": "Petya", "last_name": "Smirnov"}),
            json!({"@type": "user", "id": 104, "first_name": "Oleg",
                "status": {"@type": "userStatusLastWeek"}}),
        ] {
            store.apply(&user);
        }
        store.apply(
            &json!({"@type": "chat", "id": 101, "title": "Anna Smirnova",
            "type": {"@type": "chatTypePrivate", "user_id": 101},
            "notification_settings": {"use_default_mute_for": false, "mute_for": 2147483647}}),
        );
        store.apply(
            &json!({"@type": "chat", "id": -1000000001001i64, "title": "Backend",
            "type": {"@type": "chatTypeSupergroup", "supergroup_id": 1001, "is_channel": false},
            "permissions": {"can_send_basic_messages": true}}),
        );
        store.apply(
            &json!({"@type": "supergroup", "id": 1001, "member_count": 5,
            "usernames": {"active_usernames": ["backend"]},
            "status": {"@type": "chatMemberStatusMember"}}),
        );
        store
    }

    fn shown(chat: i64) -> Shown {
        Shown {
            key: ChatKey::new("a1", chat),
            epoch: 1,
            big_photo: None,
            phone: None,
            about: None,
            member_count: None,
            basic_members: None,
            counts: BTreeMap::new(),
            asked_info: false,
            sent: None,
            dirty: false,
            members: None,
            media: HashMap::new(),
        }
    }

    #[test]
    fn asks_and_tells_as_json() {
        let ask = Request::Profile(ProfileAsk::Media {
            key: ChatKey::new("a1", -201),
            kind: MediaKind::Photos,
            more: true,
        });
        let text = serde_json::to_string(&ask).unwrap();
        assert!(text.contains(r#""do":"profile""#) && text.contains(r#""profile":"media""#));
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), ask);
        let close = serde_json::to_string(&Request::Profile(ProfileAsk::Close)).unwrap();
        assert_eq!(close, r#"{"do":"profile","profile":"close"}"#);
        let told = Note::Profile(ProfileTold::Members {
            key: ChatKey::new("a1", -201),
            query: "an".into(),
            members: Vec::new(),
            problem: None,
        });
        let text = serde_json::to_string(&told).unwrap();
        assert_eq!(serde_json::from_str::<Note>(&text).unwrap(), told);
    }

    #[test]
    fn shows_a_person_and_a_group() {
        let store = store();
        let mut anna = shown(101);
        anna.phone = Some("+79001234567".into());
        anna.about = Some("Designer".into());
        anna.counts.insert(MediaKind::Photos, 12);
        let profile = profile(&store, &anna, AT, &English);
        assert_eq!(profile.title, "Anna Smirnova");
        assert_eq!((profile.status.as_str(), profile.online), ("online", true));
        assert_eq!(profile.username.as_deref(), Some("@anna"));
        assert_eq!(profile.phone.as_deref(), Some("+79001234567"));
        assert!(profile.muted && !profile.group && profile.can_message);
        assert_eq!(profile.counts, [(MediaKind::Photos, 12)]);

        let mut backend = shown(-1000000001001);
        backend.member_count = Some(6);
        backend.phone = Some("+1".into());
        let group = super::profile(&store, &backend, AT, &English);
        assert_eq!(group.status, "6 members");
        assert_eq!(group.link.as_deref(), Some("t.me/backend"));
        assert!(group.group && group.username.is_none() && group.phone.is_none());

        // A person without a chat yet: their name, their status.
        let dima = super::profile(&store, &shown(102), AT, &English);
        assert_eq!(dima.title, "Dima Kim");
        assert_eq!(dima.status, "last seen 30 minutes ago");
        assert!(dima.can_message);
        assert_eq!(
            person_status(&store, 104, AT, &English).0,
            "last seen within a week"
        );
    }

    #[test]
    fn reads_full_info() {
        let user = json!({"@type": "userFullInfo", "bio": {"@type": "formattedText",
            "text": " Designer ", "entities": []}});
        assert_eq!(about_of(&user).as_deref(), Some("Designer"));
        let old = json!({"@type": "supergroupFullInfo", "description": "The team"});
        assert_eq!(about_of(&old).as_deref(), Some("The team"));
        assert_eq!(
            about_of(&json!({"@type": "userFullInfo", "bio": null})),
            None
        );
        assert_eq!(phone_of(&json!({"phone_number": ""})), None);
        assert_eq!(
            phone_of(&json!({"phone_number": "79001234567"})).as_deref(),
            Some("+79001234567")
        );
        let muted = notification_settings(true);
        assert_eq!(muted["mute_for"], FOREVER);
        assert_eq!(muted["use_default_mute_for"], false);
        assert_eq!(muted["use_default_sound"], true);
        assert_eq!(notification_settings(false)["mute_for"], 0);
    }

    #[test]
    fn lists_members_admins_first() {
        let store = store();
        let member = |user: i64, status: Value| {
            json!({"@type": "chatMember", "member_id": {"@type": "messageSenderUser", "user_id": user},
                "status": status})
        };
        let values = vec![
            member(102, json!({"@type": "chatMemberStatusMember"})),
            member(
                104,
                json!({"@type": "chatMemberStatusAdministrator", "custom_title": ""}),
            ),
            member(
                101,
                json!({"@type": "chatMemberStatusCreator", "custom_title": "boss"}),
            ),
            member(103, json!({"@type": "chatMemberStatusLeft"})),
            // An admin listed again among the recent ones: once.
            member(104, json!({"@type": "chatMemberStatusAdministrator"})),
            json!({"@type": "chatMember", "member_id": {"@type": "messageSenderChat", "chat_id": -5},
                "status": {"@type": "chatMemberStatusMember"}}),
        ];
        let list = members_of(&store, &values, "", AT, &English);
        let names: Vec<(&str, Option<&str>)> = list
            .iter()
            .map(|m| (m.name.as_str(), m.role.as_deref()))
            .collect();
        assert_eq!(
            names,
            [
                ("Anna Smirnova", Some("boss")),
                ("Oleg", Some("admin")),
                ("Dima Kim", None)
            ]
        );
        assert!(list[0].online);
        let found = members_of(&store, &values, "ki", AT, &English);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Dima Kim");
        assert_eq!(members_of(&store, &values, "@an", AT, &English).len(), 1);
        assert_eq!(member_ids(&values), [102, 104, 101, 103, 104]);
    }

    fn file(id: i64, path: &str) -> Value {
        json!({"@type": "file", "id": id, "size": 1_300_000, "expected_size": 1_300_000,
            "local": {"path": path, "is_downloading_completed": !path.is_empty(),
                "is_downloading_active": false, "downloaded_size": 0},
            "remote": {"is_uploading_active": false, "uploaded_size": 0}})
    }

    fn message(id: i64, content: Value) -> Value {
        json!({"@type": "message", "id": id, "chat_id": -201, "date": NOW - 86_400,
            "sender_id": {"@type": "messageSenderUser", "user_id": 101}, "is_outgoing": false,
            "content": content})
    }

    #[test]
    fn makes_gallery_squares() {
        let mut store = store();
        let photo = message(
            5,
            json!({"@type": "messagePhoto", "photo": {"minithumbnail": {"data": "AAEC"},
            "sizes": [
                {"type": "m", "width": 320, "height": 240, "photo": file(1, "")},
                {"type": "x", "width": 800, "height": 600, "photo": file(2, "")},
                {"type": "s", "width": 100, "height": 75, "photo": file(3, "")},
                {"type": "w", "width": 1600, "height": 1200, "photo": file(4, "")}]}}),
        );
        let (item, files) = media_item(&store, &photo, MediaKind::Photos, AT, &English).unwrap();
        assert_eq!(files, [1, 4]);
        assert_eq!((item.picture, item.full), (None, None));
        assert_eq!(item.preview.as_deref(), Some("AAEC"));
        assert_eq!(item.group.as_deref(), Some("November 2025"));
        assert_eq!(square_file(&photo["content"]), Some(1));
        assert_eq!(clicked_file(&photo["content"]), Some(4));
        store.apply(&json!({"@type": "updateFile", "file": file(1, "/f/1.jpg")}));
        let (item, _) = media_item(&store, &photo, MediaKind::Photos, AT, &English).unwrap();
        assert_eq!(item.picture.as_deref(), Some("/f/1.jpg"));
        assert_eq!(item.preview, None);

        let video = message(
            6,
            json!({"@type": "messageVideo", "video": {"duration": 65,
            "minithumbnail": null, "video": file(8, ""),
            "thumbnail": {"format": {"@type": "thumbnailFormatJpeg"}, "file": file(9, "/f/9.jpg")}}}),
        );
        let (item, files) = media_item(&store, &video, MediaKind::Photos, AT, &English).unwrap();
        assert_eq!(files, [8, 9]);
        assert_eq!(item.picture.as_deref(), Some("/f/9.jpg"));
        assert_eq!((item.label.as_deref(), item.play), (Some("1:05"), true));
        assert_eq!(clicked_file(&video["content"]), Some(8));
        // Not a kind of the section: none.
        assert!(media_item(&store, &video, MediaKind::Files, AT, &English).is_none());
    }

    #[test]
    fn makes_rows_of_files_links_and_voice() {
        let store = store();
        let document = message(
            7,
            json!({"@type": "messageDocument", "document": {"file_name": "report.pdf",
            "document": file(11, "")}}),
        );
        let (item, _) = media_item(&store, &document, MediaKind::Files, AT, &English).unwrap();
        assert_eq!(item.title, "report.pdf");
        assert_eq!(item.subtitle, "1.2 MB");
        assert_eq!(item.detail, "16 Nov");

        let preview = message(
            8,
            json!({"@type": "messageText", "text": {"text": "Notes: https://example.com/blog",
            "entities": []}, "link_preview": {"url": "https://example.com/blog", "site_name": "Example Blog",
            "title": "Release notes", "description": {"text": "What is new"}}}),
        );
        let (item, _) = media_item(&store, &preview, MediaKind::Links, AT, &English).unwrap();
        assert_eq!(item.title, "Release notes");
        assert_eq!(item.subtitle, "Example Blog");
        assert_eq!(item.url.as_deref(), Some("https://example.com/blog"));

        // A link in Cyrillic text: TDLib's offsets are UTF-16.
        let plain = "Смотри 🚀 example.com/news и всё";
        let entity = json!({"offset": 10, "length": 16, "type": {"@type": "textEntityTypeUrl"}});
        let text = message(
            9,
            json!({"@type": "messageText", "text": {"text": plain, "entities": [entity]}}),
        );
        let (item, _) = media_item(&store, &text, MediaKind::Links, AT, &English).unwrap();
        assert_eq!(item.url.as_deref(), Some("https://example.com/news"));
        assert_eq!(item.title, "example.com/news");
        assert_eq!(item.subtitle, plain);
        let named = json!({"@type": "messageText", "text": {"text": "the doc", "entities": [
            {"offset": 4, "length": 3, "type": {"@type": "textEntityTypeTextUrl", "url": "https://x.org/doc"}}]}});
        assert_eq!(link_of(&named).unwrap().0, "https://x.org/doc");
        assert!(link_of(&json!({"@type": "messageText", "text": {"text": "no link"}})).is_none());
        // A line with the address in it stays whole; a bare address says its site.
        let line = "Flux 0.5 is out https://example.com/news";
        let about = json!({"@type": "messageText", "text": {"text": line, "entities": []}});
        assert_eq!(link_of(&about).unwrap().2.as_deref(), Some(line));
        let bare_link = message(
            11,
            json!({"@type": "messageText", "text": {"text": "https://example.com/a/b?c", "entities": []}}),
        );
        let (item, _) = media_item(&store, &bare_link, MediaKind::Links, AT, &English).unwrap();
        assert_eq!(
            (item.title.as_str(), item.subtitle.as_str()),
            ("example.com/a/b?c", "example.com")
        );

        let voice = message(
            10,
            json!({"@type": "messageVoiceNote", "voice_note": {"duration": 42,
            "voice": file(12, "/f/12.ogg")}}),
        );
        let (item, _) = media_item(&store, &voice, MediaKind::Voice, AT, &English).unwrap();
        assert_eq!(
            (item.title.as_str(), item.subtitle.as_str()),
            ("Anna Smirnova", "0:42")
        );
        assert_eq!(item.path.as_deref(), Some("/f/12.ogg"));
        assert!(item.avatar.is_some());
    }

    #[test]
    fn tells_dates_and_sizes() {
        // 1_763_380_000 is 2025-11-17 14:46 at +3.
        assert_eq!(month_heading(NOW, 3 * 3600, &English), "November 2025");
        assert_eq!(short_date(NOW, AT, &English), "17 Nov");
        assert_eq!(short_date(NOW - 400 * 86_400, AT, &English), "13 Oct 2024");
        assert_eq!(size_text(512, &English), "512 B");
        assert_eq!(size_text(18_000, &English), "18 KB");
        assert_eq!(size_text(1_300_000, &English), "1.2 MB");
        assert_eq!(size_text(3 * 1024 * 1024, &English), "3 MB");
        assert_eq!(size_text(5 * 1024 * 1024 * 1024 / 2, &English), "2.5 GB");
        assert_eq!(size_text(0, &English), "");
    }
}
