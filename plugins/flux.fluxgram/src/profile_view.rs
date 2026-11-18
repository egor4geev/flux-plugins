//! The profile dialog (part 3+): the window `profile` (`location = "dialog"`) over Flux, as
//! Telegram's profile box — a person's or a group's top (a large picture, the name, "online" /
//! "5 members"), the info rows, the buttons (Send Message, Add to FluxGram or Remove, Mute), and
//! the parts under them, a chip each with its count: Members (groups: admins first, a search),
//! Photos and Videos (a gallery by month), Files, Links, Voice. A member opens their own profile
//! in the dialog (Back returns).
//!
//! Opened by a chat's header (a click), its ⋯ menu (Profile, Media), by [`ProfileView::open`].
//! The service makes what the dialog shows ([`crate::profile`]); the dialog keeps what it was
//! told of the profile on show and asks for the rest as the user goes: the members when their
//! part opens (a search after a pause in typing), the media page by page, the pictures of the
//! squares in sight, a file when it is clicked — opened by the system once it is there.
//!
//! A scenario reaches the dialog through `ui:<plugin>/profile/<element>`: the buttons by their
//! ids (`profile-message`, `profile-list`, `profile-mute`, `profile-back`, `part/<part>`), a
//! member by `member/<user id>`, a media item by `item/<message id>`, and
//! `ui-submit:…/profile/members-search=<text>` searches the members.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use base64::Engine;
use flux_plugin_api::UiEvent;
use flux_plugin_api::conversation::{self as conv, History};
use flux_plugin_api::host::ui::{self, View};
use flux_plugin_api::view::*;
use flux_plugin_api::{log, notify, service, system, timers, tr, trf};

use crate::model::{ChatKey, Request};
use crate::profile::{MediaItem, MediaKind, Member, Profile, ProfileAsk, ProfileTold};

/// The dialog's window.
pub const PROFILE: &str = "profile";
/// The members' search waits this long after the last key.
const SEARCH_AFTER: Duration = Duration::from_millis(300);

/// Which part of the profile opens first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// The info (and the members of a group).
    Info,
    /// The media.
    Media,
}

/// What the dialog asks the window for.
pub enum ProfileGo {
    /// Open the chat's tab (Send Message).
    Open(ChatKey),
}

/// A part of the dialog under the info: a chip each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Members,
    Media(MediaKind),
}

impl Part {
    const ALL: [Part; 5] = [
        Part::Members,
        Part::Media(MediaKind::Photos),
        Part::Media(MediaKind::Files),
        Part::Media(MediaKind::Links),
        Part::Media(MediaKind::Voice),
    ];

    /// The part's chip: `part/<id>`.
    fn id(self) -> &'static str {
        match self {
            Part::Members => "members",
            Part::Media(MediaKind::Photos) => "photos",
            Part::Media(MediaKind::Files) => "files",
            Part::Media(MediaKind::Links) => "links",
            Part::Media(MediaKind::Voice) => "voice",
        }
    }

    fn of(id: &str) -> Option<Part> {
        Part::ALL.into_iter().find(|part| part.id() == id)
    }

    fn label(self) -> String {
        tr(match self {
            Part::Members => "Members",
            Part::Media(MediaKind::Photos) => "Photos & Videos",
            Part::Media(MediaKind::Files) => "Files",
            Part::Media(MediaKind::Links) => "Links",
            Part::Media(MediaKind::Voice) => "Voice",
        })
    }
}

/// A profile of the dialog's stack: a member's opens over the group's.
struct Page {
    key: ChatKey,
    /// The part chosen; none — the default: a group's members, else the photos.
    part: Option<Part>,
}

/// An item as told, with its minithumbnail decoded once.
struct Item {
    item: MediaItem,
    preview: Option<Vec<u8>>,
}

/// The loaded media of a kind.
#[derive(Default)]
struct Media {
    /// Newest first.
    order: Vec<i64>,
    items: HashMap<i64, Item>,
    more: bool,
    loading: bool,
    /// The first page is asked.
    asked: bool,
    /// The squares in sight, as told the service.
    visible: Option<(i64, i64)>,
}

/// The members as told.
struct Members {
    query: String,
    list: Vec<Member>,
    problem: Option<String>,
}

pub struct ProfileView {
    /// The dialog is open.
    shown: bool,
    /// The profiles opened, the one on show last.
    stack: Vec<Page>,
    profile: Option<Profile>,
    members: Option<Members>,
    /// The members' search field.
    typed: String,
    /// The query asked last.
    asked_query: Option<String>,
    search_timer: Option<u64>,
    media: HashMap<MediaKind, Media>,
    /// A clicked file downloading: the system opens it once it is there.
    open_when_ready: Option<(MediaKind, i64)>,
    /// The chats in each account's list (Add to FluxGram, or Remove).
    listed: HashMap<String, HashSet<i64>>,
    /// The view set last: an unchanged one isn't set again.
    view: Option<View>,
}

impl ProfileView {
    pub fn new() -> Self {
        ProfileView {
            shown: false,
            stack: Vec::new(),
            profile: None,
            members: None,
            typed: String::new(),
            asked_query: None,
            search_timer: None,
            media: HashMap::new(),
            open_when_ready: None,
            listed: HashMap::new(),
            view: None,
        }
    }

    /// Opens the dialog on a chat's profile (a person's chat, or a group).
    pub fn open(&mut self, key: &ChatKey, section: Section) {
        let part = match section {
            Section::Info => None,
            Section::Media => Some(Part::Media(MediaKind::Photos)),
        };
        let same = self.shown && self.stack.last().is_some_and(|page| page.key == *key);
        if same {
            // The dialog shows it already: the part asked for comes forward.
            if let Some(page) = self.stack.last_mut().filter(|_| part.is_some()) {
                page.part = part;
            }
        } else {
            self.stack = vec![Page {
                key: key.clone(),
                part,
            }];
            self.start();
        }
        self.ensure_part();
        self.render();
        ui::show(PROFILE);
    }

    /// The page on top is new (opened, or come back to): what was shown goes, the service is
    /// asked for it.
    fn start(&mut self) {
        let Some(key) = self.key() else {
            return;
        };
        self.profile = None;
        self.members = None;
        self.typed.clear();
        self.asked_query = None;
        self.media.clear();
        self.open_when_ready = None;
        if let Some(timer) = self.search_timer.take() {
            timers::cancel(timer);
        }
        ask(ProfileAsk::Open { key });
    }

    fn key(&self) -> Option<ChatKey> {
        self.stack.last().map(|page| page.key.clone())
    }

    /// The part on show: the chosen one, else a group's members, else the photos.
    fn part(&self) -> Part {
        let chosen = self.stack.last().and_then(|page| page.part);
        let group = self.profile.as_ref().is_some_and(|profile| profile.group);
        match chosen {
            Some(Part::Members) if !group && self.profile.is_some() => {
                Part::Media(MediaKind::Photos)
            }
            Some(part) => part,
            None if group => Part::Members,
            None => Part::Media(MediaKind::Photos),
        }
    }

    /// The part on show has what it shows asked for. The default part waits for the profile (a
    /// group's is its members).
    fn ensure_part(&mut self) {
        let Some(key) = self.key() else {
            return;
        };
        let chosen = self.stack.last().and_then(|page| page.part).is_some();
        if !chosen && self.profile.is_none() {
            return;
        }
        match self.part() {
            Part::Members => {
                if self.asked_query.is_none() {
                    self.asked_query = Some(self.typed.trim().to_string());
                    ask(ProfileAsk::Members {
                        key,
                        query: self.typed.trim().to_string(),
                    });
                }
            }
            Part::Media(kind) => {
                let media = self.media.entry(kind).or_default();
                if !media.asked {
                    media.asked = true;
                    media.loading = true;
                    media.more = true;
                    ask(ProfileAsk::Media {
                        key,
                        kind,
                        more: false,
                    });
                }
            }
        }
    }

    /// The service's answer.
    pub fn told(&mut self, told: ProfileTold) {
        let Some(current) = self.key() else {
            return;
        };
        match told {
            ProfileTold::Profile { key, profile } if key == current => {
                self.profile = Some(profile);
                self.ensure_part();
            }
            ProfileTold::Members {
                key,
                query,
                members,
                problem,
            } if key == current && self.asked_query.as_deref() == Some(query.as_str()) => {
                self.members = Some(Members {
                    query,
                    list: members,
                    problem,
                });
            }
            ProfileTold::Media {
                key,
                kind,
                items,
                more,
                reset,
            } if key == current => {
                let media = self.media.entry(kind).or_default();
                if reset {
                    media.order.clear();
                    media.items.clear();
                    media.visible = None;
                }
                for item in items {
                    let id = item.message;
                    if !media.items.contains_key(&id) {
                        media.order.push(id);
                    }
                    let preview = item.preview.as_deref().and_then(|data| {
                        base64::engine::general_purpose::STANDARD.decode(data).ok()
                    });
                    if self.open_when_ready == Some((kind, id))
                        && let Some(path) = &item.path
                    {
                        self.open_when_ready = None;
                        open_file(path);
                    }
                    media.items.insert(id, Item { item, preview });
                }
                media.more = more;
                media.loading = false;
            }
            _ => return,
        }
        self.render();
    }

    /// The account's list changed: the chats in it (Add to FluxGram, or Remove).
    pub fn listed(&mut self, account: &str, chats: impl IntoIterator<Item = i64>) {
        let chats: HashSet<i64> = chats.into_iter().collect();
        if self.listed.get(account) == Some(&chats) {
            return;
        }
        self.listed.insert(account.to_string(), chats);
        if self.key().is_some_and(|key| key.account == account) {
            self.render();
        }
    }

    /// The user did something in the dialog.
    pub fn ui(&mut self, element: &str, event: UiEvent) -> Vec<ProfileGo> {
        let Some(key) = self.key() else {
            return Vec::new();
        };
        let mut go = Vec::new();
        match (element, event) {
            ("profile-back", UiEvent::Clicked) => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                    self.start();
                    self.ensure_part();
                }
            }
            ("profile-message", UiEvent::Clicked) => {
                go.push(ProfileGo::Open(key));
                ui::hide(PROFILE);
                return go;
            }
            ("profile-list", UiEvent::Clicked) => {
                let chats = self.listed.entry(key.account.clone()).or_default();
                // Shown at once; the list's own answer follows.
                if chats.remove(&key.chat) {
                    request(Request::RemoveChat { key });
                } else {
                    chats.insert(key.chat);
                    request(Request::AddChat { key });
                }
            }
            ("profile-mute", UiEvent::Clicked) => {
                if let Some(profile) = &mut self.profile {
                    profile.muted = !profile.muted;
                    ask(ProfileAsk::Mute {
                        key,
                        muted: profile.muted,
                    });
                }
            }
            (MEMBERS_SEARCH, UiEvent::Changed(text)) => {
                if text != self.typed {
                    self.typed = text;
                    if let Some(timer) = self.search_timer.take() {
                        timers::cancel(timer);
                    }
                    self.search_timer = Some(timers::after(SEARCH_AFTER));
                }
                return go;
            }
            (MEMBERS_SEARCH, UiEvent::Submitted(text)) => {
                self.typed = text;
                self.search();
            }
            (MEMBERS, UiEvent::Activated(user)) => self.member(&user),
            (GALLERY, UiEvent::Activated(message)) => self.activate(MediaKind::Photos, &message),
            (GALLERY, UiEvent::Visible((first, last))) => self.visible(&first, &last),
            (GALLERY, UiEvent::LoadOlder) | ("media-more", UiEvent::Clicked) => self.more(),
            (FILES, UiEvent::Activated(message)) => self.activate(MediaKind::Files, &message),
            (LINKS, UiEvent::Activated(message)) => self.activate(MediaKind::Links, &message),
            (VOICE, UiEvent::Activated(message)) => self.activate(MediaKind::Voice, &message),
            (FILES, UiEvent::RowAction((message, action))) if action == SHOW => {
                let path = message
                    .parse()
                    .ok()
                    .and_then(|id| self.item(MediaKind::Files, id))
                    .and_then(|item| item.path.clone());
                if let Some(path) = path
                    && let Err(err) = system::show_file(&path)
                {
                    log::warn(&format!("Couldn't show {path}: {err}"));
                }
            }
            (element, UiEvent::Clicked) => {
                if let Some(part) = element.strip_prefix("part/").and_then(Part::of) {
                    if let Some(page) = self.stack.last_mut() {
                        page.part = Some(part);
                    }
                    self.ensure_part();
                } else if let Some(user) = element.strip_prefix("member/") {
                    // A scenario's member.
                    self.member(user);
                } else if let Some(message) = element.strip_prefix("item/") {
                    // A scenario's media item: the one of the part on show.
                    if let Part::Media(kind) = self.part() {
                        self.activate(kind, message);
                    }
                } else {
                    return go;
                }
            }
            _ => return go,
        }
        self.render();
        go
    }

    /// A member's row: their profile, over the group's.
    fn member(&mut self, user: &str) {
        let (Some(key), Ok(user)) = (self.key(), user.parse::<i64>()) else {
            return;
        };
        self.stack.push(Page {
            key: ChatKey::new(&key.account, user),
            part: None,
        });
        self.start();
    }

    /// The members' search asks now.
    fn search(&mut self) {
        if let Some(timer) = self.search_timer.take() {
            timers::cancel(timer);
        }
        let Some(key) = self.key() else {
            return;
        };
        let query = self.typed.trim().to_string();
        if self.asked_query.as_deref() == Some(query.as_str()) {
            return;
        }
        self.asked_query = Some(query.clone());
        ask(ProfileAsk::Members { key, query });
    }

    fn item(&self, kind: MediaKind, message: i64) -> Option<&MediaItem> {
        Some(&self.media.get(&kind)?.items.get(&message)?.item)
    }

    /// A click on an item: a link opens; a photo without its full size downloads it (Flux's
    /// viewer opens one whose file is there by itself); a video, a file, a voice message opens
    /// with the system's app — once downloaded.
    fn activate(&mut self, kind: MediaKind, message: &str) {
        let (Some(key), Ok(message)) = (self.key(), message.parse::<i64>()) else {
            return;
        };
        let Some(item) = self.item(kind, message) else {
            return;
        };
        if let Some(url) = &item.url {
            if let Err(err) = system::open_url(url) {
                log::warn(&format!("Couldn't open {url}: {err}"));
            }
            return;
        }
        if kind == MediaKind::Photos && !item.play {
            if item.full.is_none() {
                ask(ProfileAsk::Download { key, kind, message });
            }
            return;
        }
        match item.path.clone() {
            Some(path) => open_file(&path),
            None => {
                self.open_when_ready = Some((kind, message));
                ask(ProfileAsk::Download { key, kind, message });
            }
        }
    }

    /// The gallery's squares in sight: their pictures download first.
    fn visible(&mut self, first: &str, last: &str) {
        let (Some(key), Ok(first), Ok(last)) = (self.key(), first.parse(), last.parse()) else {
            return;
        };
        let Some(media) = self.media.get_mut(&MediaKind::Photos) else {
            return;
        };
        if media.visible == Some((first, last)) {
            return;
        }
        media.visible = Some((first, last));
        ask(ProfileAsk::Visible {
            key,
            kind: MediaKind::Photos,
            first,
            last,
        });
    }

    /// The next page of the part on show.
    fn more(&mut self) {
        let (Some(key), Part::Media(kind)) = (self.key(), self.part()) else {
            return;
        };
        let Some(media) = self.media.get_mut(&kind) else {
            return;
        };
        if media.loading || !media.more {
            return;
        }
        media.loading = true;
        ask(ProfileAsk::Media {
            key,
            kind,
            more: true,
        });
    }

    /// The dialog came into sight, or was closed (Esc, ×).
    pub fn shown(&mut self, shown: bool) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        if shown {
            self.render();
            return;
        }
        // Closed: the service stops telling, the next opening starts afresh.
        ask(ProfileAsk::Close);
        self.stack.clear();
        self.profile = None;
        self.members = None;
        self.media.clear();
        self.open_when_ready = None;
        if let Some(timer) = self.search_timer.take() {
            timers::cancel(timer);
        }
    }

    /// Mute or unmute a chat (a chat tab's ⋯ menu).
    pub fn mute(&mut self, key: &ChatKey, muted: bool) {
        ask(ProfileAsk::Mute {
            key: key.clone(),
            muted,
        });
        if self.key().as_ref() == Some(key)
            && let Some(profile) = &mut self.profile
        {
            profile.muted = muted;
            self.render();
        }
    }

    /// A timer of the dialog fired (one it started); false — not its.
    pub fn timer(&mut self, timer: u64) -> bool {
        if self.search_timer != Some(timer) {
            return false;
        }
        self.search_timer = None;
        self.search();
        true
    }

    // --- The view ---

    fn render(&mut self) {
        let Some(key) = self.key() else {
            return;
        };
        let mut children: Vec<Element> = Vec::new();
        if self.stack.len() > 1 {
            children.push(row(
                "profile-nav",
                [button("profile-back", &tr("Back"))
                    .icon("arrow-left")
                    .into()],
            ));
        }
        match &self.profile {
            None => {
                // Until the service tells: the header's place, and a bar.
                children.push(header("profile-header", "").large().into());
                children.push(progress("profile-loading", None));
            }
            Some(profile) => {
                let listed = self
                    .listed
                    .get(&key.account)
                    .is_some_and(|chats| chats.contains(&key.chat));
                children.extend(top(profile, listed));
                children.push(chips(profile, self.part()));
                children.extend(self.part_view());
            }
        }
        let view = column("profile-root", children).into_view();
        if self.view.as_ref() == Some(&view) {
            return;
        }
        ui::set_view(PROFILE, &view);
        self.view = Some(view);
    }

    /// The part on show: the members, the gallery, the rows of files, links or voice messages.
    fn part_view(&self) -> Vec<Element> {
        match self.part() {
            Part::Members => {
                let mut tree = Tree::new().two_line();
                let empty = match &self.members {
                    None => tr("Loading…"),
                    Some(members) => members.problem.clone().unwrap_or_else(|| {
                        if members.query.is_empty() {
                            tr("No members to show")
                        } else {
                            tr("No members found")
                        }
                    }),
                };
                tree = tree.empty_text(&empty);
                for member in self.members.iter().flat_map(|members| &members.list) {
                    tree.add(member_row(member));
                }
                vec![
                    text_field(MEMBERS_SEARCH, &self.typed, Some(&tr("Search members"))),
                    tree.into_element(MEMBERS),
                ]
            }
            Part::Media(kind) => {
                let media = self.media.get(&kind);
                let items: Vec<&Item> = media
                    .map(|media| {
                        media
                            .order
                            .iter()
                            .filter_map(|id| media.items.get(id))
                            .collect()
                    })
                    .unwrap_or_default();
                let loading = media.is_none_or(|media| media.loading);
                let more = media.is_some_and(|media| media.more);
                if kind == MediaKind::Photos {
                    let squares = items.iter().map(|item| square(item)).collect();
                    let history = if loading {
                        History::Loading
                    } else if more {
                        History::More
                    } else {
                        History::Complete
                    };
                    let empty = tr("No photos or videos here yet");
                    return vec![gallery(GALLERY, squares, history, Some(&empty))];
                }
                let (id, empty) = match kind {
                    MediaKind::Files => (FILES, tr("No files here yet")),
                    MediaKind::Links => (LINKS, tr("No links here yet")),
                    _ => (VOICE, tr("No voice messages here yet")),
                };
                let empty = if loading && items.is_empty() {
                    tr("Loading…")
                } else {
                    empty
                };
                let mut tree = Tree::new().two_line().empty_text(&empty);
                for item in &items {
                    tree.add(media_row(kind, &item.item));
                }
                let mut part = vec![tree.into_element(id)];
                if loading && !items.is_empty() {
                    part.push(progress("media-loading", None));
                } else if more && !items.is_empty() {
                    part.push(row(
                        "media-buttons",
                        [button("media-more", &tr("Show More")).into()],
                    ));
                }
                part
            }
        }
    }
}

/// The dialog's elements that events name.
const MEMBERS_SEARCH: &str = "members-search";
const MEMBERS: &str = "members";
const GALLERY: &str = "gallery";
const FILES: &str = "files";
const LINKS: &str = "links";
const VOICE: &str = "voice";
/// A file row's action: Show in Finder.
const SHOW: &str = "show";

fn ask(ask: ProfileAsk) {
    request(Request::Profile(ask));
}

fn request(request: Request) {
    let _ = service::send_json(&request);
}

/// Opens a downloaded file with the system's app.
fn open_file(path: &str) {
    if let Err(err) = system::open_file(path) {
        log::warn(&format!("Couldn't open {path}: {err}"));
        notify::error(&tr("Couldn't open the file"), Some(&err));
    }
}

/// The top: the header, the info, the buttons.
fn top(profile: &Profile, listed: bool) -> Vec<Element> {
    let status = (!profile.status.is_empty()).then(|| {
        let tone = if profile.online {
            Tone::Accent
        } else {
            Tone::Dim
        };
        span(&profile.status).tone(tone)
    });
    let mut parts: Vec<Element> = vec![
        header("profile-header", &profile.title)
            .avatar(profile.avatar.clone())
            .subtitle(status)
            .large()
            .into(),
    ];
    // The bio or the description: whole, as it wraps.
    if let Some(about) = &profile.about {
        parts.push(text("profile-about", [span(about)]));
    }
    let mut info = Vec::new();
    if let Some(username) = &profile.username {
        info.push(property("username", &tr("Username"), vec![span(username)]));
    }
    if let Some(phone) = &profile.phone {
        info.push(property("phone", &tr("Phone"), vec![span(phone)]));
    }
    if let Some(link) = &profile.link {
        info.push(property("link", &tr("Link"), vec![span(link)]));
    }
    if !info.is_empty() {
        parts.push(properties("profile-info", info));
    }
    let mut buttons: Vec<Element> = Vec::new();
    if profile.can_message {
        buttons.push(
            button("profile-message", &tr("Send Message"))
                .primary()
                .into(),
        );
    }
    let (label, icon) = if listed {
        ("Remove from FluxGram", "minus")
    } else {
        ("Add to FluxGram", "plus")
    };
    buttons.push(button("profile-list", &tr(label)).icon(icon).into());
    let (label, icon) = if profile.muted {
        ("Unmute", "bell")
    } else {
        ("Mute", "icons/muted.svg")
    };
    buttons.push(button("profile-mute", &tr(label)).icon(icon).into());
    parts.push(row("profile-actions", buttons));
    parts.push(divider("profile-divider"));
    parts
}

/// The parts' chips, the one on show primary, each with its count; a kind of media the chat has
/// none of isn't offered (unless it is on show).
fn chips(profile: &Profile, current: Part) -> Element {
    let count = |kind: MediaKind| {
        profile
            .counts
            .iter()
            .find(|(of, _)| *of == kind)
            .map(|(_, count)| *count)
    };
    let mut buttons: Vec<Element> = Vec::new();
    for part in Part::ALL {
        let badge = match part {
            Part::Members if !profile.group => continue,
            Part::Members => Some(profile.members).filter(|members| *members > 0),
            Part::Media(kind) => match count(kind) {
                Some(0) | None if part != current => continue,
                found => found.filter(|count| *count > 0),
            },
        };
        let mut chip = button(&format!("part/{}", part.id()), &part.label());
        if let Some(badge) = badge {
            chip = chip.badge(&badge.to_string());
        }
        if part == current {
            chip = chip.primary();
        }
        buttons.push(chip.into());
    }
    row("profile-parts", buttons)
}

fn member_row(member: &Member) -> RowSpec {
    let tone = if member.online {
        Tone::Accent
    } else {
        Tone::Dim
    };
    let mut row = RowSpec::new(&member.user.to_string(), &member.name)
        .avatar(member.avatar.clone())
        .subtitle(vec![span(&member.status).tone(tone)]);
    // On the right of the name, as Telegram shows it.
    if let Some(role) = &member.role {
        row = row.detail(role);
    }
    row
}

/// A gallery's square: the small picture (or the blurred minithumbnail), the full size for the
/// viewer, a download's progress; a video (or a GIF) Flux plays in its viewer, its length in the
/// corner (part 4).
fn square(item: &Item) -> flux_plugin_api::host::ui::GalleryItem {
    let media = &item.item;
    let mut spec = match &media.picture {
        Some(path) => conv::picture(path, 0, 0),
        None => conv::pending_picture(0, 0),
    };
    if spec.path.is_none()
        && let Some(preview) = &item.preview
    {
        spec = spec.preview(preview.clone());
    }
    if let Some(full) = &media.full {
        spec = spec.full(full);
    }
    if let Some(progress) = media.progress {
        spec = spec.progress(progress);
    }
    let key = media.message.to_string();
    if media.play {
        let kind = match media.gif {
            true => conv::VideoKind::Gif,
            false => conv::VideoKind::Video,
        };
        let video = conv::video_spec(kind, spec, media.path.as_deref(), media.duration);
        return gallery_video(&key, video, media.group.as_deref());
    }
    gallery_item(&key, spec, media.group.as_deref())
}

/// A row of files, links or voice messages: what it is, its date, its size or length (or the
/// download's progress).
fn media_row(kind: MediaKind, item: &MediaItem) -> RowSpec {
    let subtitle = match item.progress {
        Some(done) => trf("Downloading… {0}%", &[&((done * 100.0).round() as u32)]),
        None => item.subtitle.clone(),
    };
    let mut row = RowSpec::new(&item.message.to_string(), &item.title)
        .detail(&item.detail)
        .subtitle(vec![span(&subtitle).tone(Tone::Dim)]);
    row = match kind {
        MediaKind::Files => row.icon(&format!("file:{}", item.title)),
        MediaKind::Links => row.icon("globe"),
        _ => match &item.avatar {
            Some(avatar) => row.avatar(avatar.clone()),
            None => row.icon("icons/mic.svg"),
        },
    };
    if kind == MediaKind::Files
        && let Some(path) = &item.path
    {
        row = row.action(SHOW, Some(&show_label(path)), Some("folder"), false);
    }
    row
}

/// Show in Finder, or in Explorer: a Windows path has a drive.
fn show_label(path: &str) -> String {
    let windows = path.as_bytes().get(1) == Some(&b':') || path.contains('\\');
    tr(if windows {
        "Show in Explorer"
    } else {
        "Show in Finder"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_parts() {
        for part in Part::ALL {
            assert_eq!(Part::of(part.id()), Some(part));
        }
        assert_eq!(Part::of("nothing"), None);
    }
}
