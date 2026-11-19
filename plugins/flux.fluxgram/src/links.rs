//! Links (part 5): one router for every link FluxGram meets — the feed's (its conversation owns
//! its links, `own-links`: the web ones come to the plugin too), a link preview's, a quote's, an
//! advertisement's button, a bot's link button, a link of the chat's media in the profile.
//!
//! - Telegram's links — `t.me`, `telegram.me`, `telegram.dog` (any path, `<name>.t.me` too) and
//!   `tg:` — open in FluxGram, with the account of the chat they were clicked in (never another):
//!   the service has TDLib parse them (`getInternalLinkType`) and opens what they name — a public
//!   chat's tab (a person, a bot, a group, a channel; one not in the list as well, its ⋯ offers
//!   Add to FluxGram, a channel the user isn't in its Join; the link's `profile` opens the
//!   profile, its draft goes into the field), a message (its chat, the message revealed), an
//!   invite (asked first: "Join «Title»?" and how many are in it; a chat the user is in opens at
//!   once), a bot's start (its chat, whose Start sends the parameter), a phone number's or a user
//!   token's person, a sticker set (asked first, then added). Any other kind (a proxy, a theme, a
//!   game, a payment…) is told calmly, with Open in Browser.
//! - The text's entities, which `convert.rs` gives the plugin's links: a @username opens a
//!   person's profile and any other chat's tab, as Telegram's apps do; a mention without a
//!   username the person's profile; a #hashtag or a $cashtag the chat's search with it; a bot's
//!   /command is sent as written (`/start@bot` in a group); an e-mail opens the mail app; a phone
//!   number and a bank card's are copied.
//! - The web: as Settings → FluxGram → Links say ([`Policy`]) — asked always, never, or when a
//!   link's text isn't its address ([`same_address`]). A host that may pretend to be another
//!   ([`spoofed`]: letters of different alphabets in one label, or look-alikes of Latin ones
//!   under a Latin top-level domain) is asked about whatever the setting, its address shown in
//!   punycode. Telegram's own sites (`telegram.org`, `telegra.ph`…) open at once.
//!
//! The decisions need no host and are tested natively; the window ([`crate::chat`]) asks the
//! questions, the service's part ([`request`], [`answer`]) talks to TDLib.

use flux_plugin_api::conversation::{Message, Part};
use flux_plugin_api::service;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::convert::{CARD_LINK, COMMAND_LINK, MENTION_LINK, TAG_LINK, USER_LINK};
use crate::feed::Feed;
use crate::model::{ChatKey, Note};
use crate::service::{Asked, Cx};
use crate::store::ChatKind;

/// Where a link goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Telegram's own: parsed by TDLib in the service.
    Telegram(String),
    /// A @username in a text (without the "@").
    Mention(String),
    /// A mention of someone without a username.
    User(i64),
    /// A bot's command as written: "/start", "/help@bot".
    Command(String),
    /// A #hashtag or a $cashtag: the chat's search.
    Search(String),
    /// A phone number, a bank card's: copied.
    Phone(String),
    Card(String),
    /// `mailto:`: the mail app.
    Mail(String),
    /// Anything else.
    Web(String),
}

impl Target {
    pub fn of(link: &str) -> Target {
        let link = link.trim();
        if let Some(name) = link.strip_prefix(MENTION_LINK) {
            return Target::Mention(name.to_string());
        }
        if let Some(user) = link
            .strip_prefix(USER_LINK)
            .and_then(|id| id.parse::<i64>().ok())
        {
            return Target::User(user);
        }
        if let Some(command) = link.strip_prefix(COMMAND_LINK) {
            return Target::Command(command.to_string());
        }
        if let Some(tag) = link.strip_prefix(TAG_LINK) {
            return Target::Search(tag.to_string());
        }
        if let Some(card) = link.strip_prefix(CARD_LINK) {
            return Target::Card(card.to_string());
        }
        if let Some(number) = strip_scheme(link, "tel:") {
            return Target::Phone(number.to_string());
        }
        if strip_scheme(link, "mailto:").is_some() {
            return Target::Mail(link.to_string());
        }
        if is_telegram(link) {
            return Target::Telegram(link.to_string());
        }
        Target::Web(link.to_string())
    }
}

/// What a message's link reads as: the text of its spans (a link's runs of different styles
/// joined), in the text or a quote; none — it isn't in the text (a link preview's).
pub fn link_text(message: &Message, link: &str) -> Option<String> {
    for part in &message.parts {
        let (Part::Text(spans) | Part::Quote(spans)) = part else {
            continue;
        };
        let mut text = String::new();
        let mut found = false;
        for span in spans {
            if span.link.as_deref() == Some(link) {
                text.push_str(&span.text);
                found = true;
            } else if found {
                break;
            }
        }
        if found {
            return Some(text);
        }
    }
    None
}

/// The rest of `link` after `scheme`, in any case.
fn strip_scheme<'a>(link: &'a str, scheme: &str) -> Option<&'a str> {
    link.get(..scheme.len())
        .filter(|start| start.eq_ignore_ascii_case(scheme))
        .map(|_| &link[scheme.len()..])
}

/// The link without `http://` or `https://`.
fn without_web_scheme(link: &str) -> &str {
    strip_scheme(link, "https://")
        .or_else(|| strip_scheme(link, "http://"))
        .unwrap_or(link)
}

/// Telegram's links, which TDLib opens: `tg:`, and `t.me`, `telegram.me`, `telegram.dog` (or a
/// `<name>.t.me`) with `http`, `https` or no scheme.
pub fn is_telegram(link: &str) -> bool {
    if strip_scheme(link, "tg:").is_some() {
        return true;
    }
    let rest = without_web_scheme(link);
    if rest.contains("://") {
        return false;
    }
    let host = host_of(rest);
    let host = host.strip_prefix("www.").unwrap_or(&host);
    matches!(host, "t.me" | "telegram.me" | "telegram.dog") || host.ends_with(".t.me")
}

/// Telegram's own sites, opened without a question (but when the setting asks always): Telegram's
/// list of them.
fn telegram_site(host: &str) -> bool {
    const HOSTS: &[&str] = &[
        "telegram.org",
        "telegram.me",
        "telegram.dog",
        "t.me",
        "telegra.ph",
        "te.legra.ph",
        "graph.org",
        "fragment.com",
        "telesco.pe",
    ];
    HOSTS
        .iter()
        .any(|known| host == *known || host.ends_with(&format!(".{known}")))
}

/// The host of an address, in lower case, without the user and the port:
/// `https://anna@Example.com:8080/x` → `example.com`.
pub fn host_of(address: &str) -> String {
    let rest = address.split_once("://").map_or(address, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    host.split(':')
        .next()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_lowercase()
}

/// Ask before opening a web link (Settings → FluxGram → Links).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Always,
    /// When the link's text isn't its address: the default.
    Mismatch,
    Never,
}

impl Policy {
    /// The setting's value (`ask-links`).
    pub fn of(value: Option<&str>) -> Policy {
        match value {
            Some("always") => Policy::Always,
            Some("never") => Policy::Never,
            _ => Policy::Mismatch,
        }
    }
}

/// What a click on a web link does.
#[derive(Debug, Clone, PartialEq)]
pub enum Web {
    /// Opens in the browser at once.
    Open,
    /// Asked first: the address to show (its host in punycode when it is `spoofed`).
    Ask { shown: String, spoofed: bool },
}

/// What a clicked link reads as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown<'a> {
    /// Its address, or nothing to compare (a link preview).
    Address,
    /// A text: the address, or a text link's words.
    Text(&'a str),
    /// A button's label (a bot's, an advertisement's): never the address.
    Button,
}

/// A web link's click.
pub fn web(url: &str, shown: Shown, policy: Policy) -> Web {
    let host = host_of(url);
    if spoofed(&host) {
        return Web::Ask {
            shown: with_ascii_host(url),
            spoofed: true,
        };
    }
    let ask = match policy {
        Policy::Always => true,
        Policy::Never => false,
        Policy::Mismatch => {
            !telegram_site(&host)
                && match shown {
                    Shown::Address => false,
                    Shown::Text(text) => !same_address(text, url),
                    Shown::Button => true,
                }
        }
    };
    match ask {
        true => Web::Ask {
            shown: url.to_string(),
            spoofed: false,
        },
        false => Web::Open,
    }
}

/// The text reads as the address: the same but for the scheme, `www.`, a closing `/`, the
/// host's case and its punycode form — "example.com" for `https://www.example.com/`,
/// "пример.рф" for `http://xn--e1afmkfd.xn--p1ai`.
pub fn same_address(text: &str, url: &str) -> bool {
    bare(text) == bare(url)
}

fn bare(address: &str) -> String {
    let address = address.trim();
    let rest = strip_scheme(address, "mailto:").unwrap_or_else(|| without_web_scheme(address));
    let split = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(split);
    let host = host.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    format!("{}{}", ascii_host(host), tail.trim_end_matches('/'))
}

/// The address with its host in punycode: `https://аpple.com/x` → `https://xn--pple-43d.com/x`.
pub fn with_ascii_host(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), url),
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (user, host_port) = match authority.rsplit_once('@') {
        Some((user, host)) => (format!("{user}@"), host),
        None => (String::new(), authority),
    };
    let (host, port) = match host_port.split_once(':') {
        Some((host, port)) => (host, format!(":{port}")),
        None => (host_port, String::new()),
    };
    format!(
        "{scheme}{user}{}{port}{tail}",
        ascii_host(&host.to_lowercase())
    )
}

/// A host's labels in ASCII: the non-ASCII ones in punycode (`xn--…`).
fn ascii_host(host: &str) -> String {
    host.split('.')
        .map(|label| match label.is_ascii() {
            true => label.to_string(),
            false => {
                punycode::encode(label).map_or_else(|| label.to_string(), |e| format!("xn--{e}"))
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// A label as it reads: a punycode one (`xn--…`) decoded.
fn unicode_label(label: &str) -> String {
    label
        .strip_prefix("xn--")
        .and_then(punycode::decode)
        .unwrap_or_else(|| label.to_string())
}

/// The host may pretend to be another: a label with letters of different alphabets ("аpple"
/// with a Cyrillic "а"), or one whose letters all are Latin or look like Latin ones, some not
/// ASCII ("аррӏе", "ɑpple"), under a Latin top-level domain. An ordinary name in one alphabet
/// ("пример.рф", "bücher.de") is not.
pub fn spoofed(host: &str) -> bool {
    let labels: Vec<String> = host.split('.').map(unicode_label).collect();
    let latin_top = labels.last().is_some_and(|top| top.is_ascii());
    labels
        .iter()
        .any(|label| mixed_scripts(label) || (latin_top && looks_latin(label)))
}

/// The alphabets whose letters pass for one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    Latin,
    Greek,
    Cyrillic,
    Armenian,
}

fn script(c: char) -> Option<Script> {
    match c as u32 {
        0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x2AF | 0x1D00..=0x1D7F | 0x1E00..=0x1EFF => {
            Some(Script::Latin)
        }
        0x370..=0x3FF | 0x1F00..=0x1FFF => Some(Script::Greek),
        0x400..=0x52F | 0x1C80..=0x1C8F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F => {
            Some(Script::Cyrillic)
        }
        0x531..=0x58F => Some(Script::Armenian),
        _ => None,
    }
}

fn mixed_scripts(label: &str) -> bool {
    let mut scripts = label.chars().filter_map(script);
    let Some(first) = scripts.next() else {
        return false;
    };
    scripts.any(|script| script != first)
}

/// Letters of other alphabets (and Latin's beyond ASCII) that read as Latin ones, lower case.
const LOOKALIKES: &str = "асԁеһіјӏорԛѕԝхуъьҽпгѵѡαικνορτυϲϳɑɡɩıȷɪʏᴅ";

fn looks_latin(label: &str) -> bool {
    let mut lookalike = false;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            continue;
        }
        if !LOOKALIKES.contains(c) {
            return false;
        }
        lookalike = true;
    }
    lookalike
}

/// Punycode (RFC 3492): a label's non-ASCII letters in ASCII.
mod punycode {
    const BASE: u32 = 36;
    const T_MIN: u32 = 1;
    const T_MAX: u32 = 26;
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    const INITIAL_BIAS: u32 = 72;
    const INITIAL_N: u32 = 128;

    fn adapt(mut delta: u32, points: u32, first: bool) -> u32 {
        delta /= if first { DAMP } else { 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - T_MIN) * T_MAX) / 2 {
            delta /= BASE - T_MIN;
            k += BASE;
        }
        k + (BASE - T_MIN + 1) * delta / (delta + SKEW)
    }

    fn threshold(k: u32, bias: u32) -> u32 {
        if k <= bias {
            T_MIN
        } else if k >= bias + T_MAX {
            T_MAX
        } else {
            k - bias
        }
    }

    fn digit(d: u32) -> char {
        match d {
            0..=25 => (b'a' + d as u8) as char,
            _ => (b'0' + (d - 26) as u8) as char,
        }
    }

    pub fn encode(input: &str) -> Option<String> {
        let points: Vec<u32> = input.chars().map(u32::from).collect();
        let mut out: String = input.chars().filter(char::is_ascii).collect();
        let basic = out.len() as u32;
        let mut handled = basic;
        if basic > 0 {
            out.push('-');
        }
        let (mut n, mut delta, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
        while (handled as usize) < points.len() {
            let m = points.iter().copied().filter(|&c| c >= n).min()?;
            delta = delta.checked_add((m - n).checked_mul(handled + 1)?)?;
            n = m;
            for &c in &points {
                if c < n {
                    delta = delta.checked_add(1)?;
                }
                if c == n {
                    let mut q = delta;
                    let mut k = BASE;
                    loop {
                        let t = threshold(k, bias);
                        if q < t {
                            break;
                        }
                        out.push(digit(t + (q - t) % (BASE - t)));
                        q = (q - t) / (BASE - t);
                        k += BASE;
                    }
                    out.push(digit(q));
                    bias = adapt(delta, handled + 1, handled == basic);
                    delta = 0;
                    handled += 1;
                }
            }
            delta += 1;
            n += 1;
        }
        Some(out)
    }

    pub fn decode(input: &str) -> Option<String> {
        let (basic, encoded) = input.rsplit_once('-').unwrap_or(("", input));
        if !basic.is_ascii() {
            return None;
        }
        let mut out: Vec<char> = basic.chars().collect();
        let (mut n, mut i, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
        let mut bytes = encoded.bytes().peekable();
        while bytes.peek().is_some() {
            let old = i;
            let mut w = 1u32;
            let mut k = BASE;
            loop {
                let d = match bytes.next()? {
                    c @ b'a'..=b'z' => u32::from(c - b'a'),
                    c @ b'A'..=b'Z' => u32::from(c - b'A'),
                    c @ b'0'..=b'9' => u32::from(c - b'0') + 26,
                    _ => return None,
                };
                i = i.checked_add(d.checked_mul(w)?)?;
                let t = threshold(k, bias);
                if d < t {
                    break;
                }
                w = w.checked_mul(BASE - t)?;
                k += BASE;
            }
            let length = out.len() as u32 + 1;
            bias = adapt(i - old, length, old == 0);
            n = n.checked_add(i / length)?;
            i %= length;
            out.insert(i as usize, char::from_u32(n)?);
            i += 1;
        }
        Some(out.into_iter().collect())
    }
}

// --- The service's part ---

/// What a window asks of the service about a link: `Request::Link`. The account is the chat's
/// the link was clicked in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "link-ask", rename_all = "kebab-case")]
pub enum LinkAsk {
    /// Telegram's link: parsed by TDLib, then opened.
    Open { key: ChatKey, url: String },
    /// A @username of a text: a person's profile, any other chat's tab.
    Mention { key: ChatKey, username: String },
    /// Join the chat of an invite link (confirmed).
    Join { key: ChatKey, link: String },
    /// Add a sticker set (confirmed).
    AddStickers { key: ChatKey, set: String },
}

/// What the service tells the window that asked: `Note::Link`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "told", rename_all = "kebab-case")]
pub enum LinkTold {
    /// Open the chat's tab: a message revealed in it, a text into its field.
    Open {
        key: ChatKey,
        title: String,
        message: Option<String>,
        draft: Option<String>,
    },
    /// A person's (or a chat's) profile.
    Profile { key: ChatKey },
    /// An invite to a chat the user isn't in: asked first. `members` — how many are in it,
    /// `channel` — subscribers; `request` — joining asks the admins.
    Invite {
        key: ChatKey,
        link: String,
        title: String,
        members: u64,
        channel: bool,
        request: bool,
    },
    /// A sticker set: asked first; one the user has is said so.
    Stickers {
        key: ChatKey,
        set: String,
        title: String,
        count: u64,
        installed: bool,
    },
    /// Done, or couldn't be: a line to the user ("Request sent", "This message isn't
    /// available") — in English, the window translates it.
    Said { text: String },
    /// A kind FluxGram doesn't open: told, with Open in Browser for a web address.
    Unsupported { url: String },
}

/// What a TDLib answer of a link is for.
pub enum Ask {
    /// `getInternalLinkType`.
    Kind {
        window: u64,
        key: ChatKey,
        url: String,
    },
    /// `searchPublicChat`: `profile` — a person opens as a profile; `draft`, `start` — the
    /// link's text for the field, a bot's start parameter.
    Public {
        window: u64,
        key: ChatKey,
        profile: bool,
        draft: Option<String>,
        start: Option<String>,
    },
    /// `getMessageLinkInfo`.
    Message { window: u64, key: ChatKey },
    /// `checkChatInviteLink`.
    Invite {
        window: u64,
        key: ChatKey,
        link: String,
    },
    /// `joinChatByInviteLink`.
    Joined { window: u64, key: ChatKey },
    /// `searchUserByPhoneNumber`, `searchUserByToken`: the person's chat follows.
    User {
        window: u64,
        key: ChatKey,
        profile: bool,
        draft: Option<String>,
    },
    /// `createPrivateChat`.
    Private {
        window: u64,
        key: ChatKey,
        profile: bool,
        draft: Option<String>,
    },
    /// `searchStickerSet`.
    Stickers {
        window: u64,
        key: ChatKey,
        url: String,
    },
    /// `changeStickerSet`.
    StickersAdded { window: u64 },
}

fn tell(window: u64, told: LinkTold) {
    let _ = service::send_json_to(window, &Note::Link(told));
}

/// A window's ask.
pub fn request(cx: &mut Cx, window: u64, ask: LinkAsk) {
    let (account, request, asked) = match ask {
        LinkAsk::Open { key, url } => (
            key.account.clone(),
            json!({"@type": "getInternalLinkType", "link": url}),
            Ask::Kind { window, key, url },
        ),
        LinkAsk::Mention { key, username } => (
            key.account.clone(),
            json!({"@type": "searchPublicChat", "username": username}),
            Ask::Public {
                window,
                key,
                profile: true,
                draft: None,
                start: None,
            },
        ),
        LinkAsk::Join { key, link } => (
            key.account.clone(),
            json!({"@type": "joinChatByInviteLink", "invite_link": link}),
            Ask::Joined { window, key },
        ),
        LinkAsk::AddStickers { key, set } => (
            key.account.clone(),
            json!({"@type": "changeStickerSet", "set_id": set, "is_installed": true,
                "is_archived": false}),
            Ask::StickersAdded { window },
        ),
    };
    cx.td.send(&account, request, Asked::Link(asked));
}

/// A TDLib answer of a link.
pub fn answer(cx: &mut Cx, feed: &mut Feed, account: &str, ask: Ask, value: &Value) {
    let error = value["@type"] == "error";
    let text = |value: &Value| {
        value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    match ask {
        Ask::Kind { window, key, url } => {
            if error {
                return tell(window, LinkTold::Unsupported { url });
            }
            internal(cx, window, key, url, value);
        }
        Ask::Public {
            window,
            key,
            profile,
            draft,
            start,
        } => {
            let Some(chat) = value["id"].as_i64().filter(|_| !error) else {
                return tell(
                    window,
                    LinkTold::Said {
                        text: "This username doesn't exist".into(),
                    },
                );
            };
            let target = ChatKey::new(&key.account, chat);
            if let Some(start) = start {
                feed.set_start_parameter(&target, &start);
            }
            // A person opens as a profile where the link says so (a mention); a bot, a group, a
            // channel as its tab.
            let person = cx.stores.get(account).is_some_and(|store| {
                store
                    .chats
                    .get(&chat)
                    .is_some_and(|found| matches!(found.kind, ChatKind::Private { .. }))
                    && !store.is_bot(chat)
            });
            if profile && person {
                return tell(window, LinkTold::Profile { key: target });
            }
            tell(
                window,
                LinkTold::Open {
                    key: target,
                    title: text(&value["title"]).unwrap_or_default(),
                    message: None,
                    draft,
                },
            );
        }
        Ask::Message { window, key } => {
            let chat = value["chat_id"].as_i64().unwrap_or_default();
            if error || chat == 0 {
                return tell(
                    window,
                    LinkTold::Said {
                        text: "This message isn't available".into(),
                    },
                );
            }
            let title = cx
                .stores
                .get(account)
                .and_then(|store| store.chats.get(&chat))
                .map(|found| found.title.clone())
                .unwrap_or_default();
            tell(
                window,
                LinkTold::Open {
                    key: ChatKey::new(&key.account, chat),
                    title,
                    message: value["message"]["id"].as_i64().map(|id| id.to_string()),
                    draft: None,
                },
            );
        }
        Ask::Invite { window, key, link } => {
            if error {
                return tell(
                    window,
                    LinkTold::Said {
                        text: "This invite link is invalid or expired".into(),
                    },
                );
            }
            let chat = value["chat_id"].as_i64().unwrap_or_default();
            let title = text(&value["title"]).unwrap_or_default();
            // A chat the user is in: opened at once.
            if chat != 0 && value["accessible_for"].as_i64().unwrap_or_default() == 0 {
                return tell(
                    window,
                    LinkTold::Open {
                        key: ChatKey::new(&key.account, chat),
                        title,
                        message: None,
                        draft: None,
                    },
                );
            }
            tell(
                window,
                LinkTold::Invite {
                    key,
                    link,
                    title,
                    members: value["member_count"].as_u64().unwrap_or_default(),
                    channel: value["type"]["@type"] == "inviteLinkChatTypeChannel",
                    request: value["creates_join_request"] == true,
                },
            );
        }
        Ask::Joined { window, key } => {
            if error {
                let told = match value["message"].as_str() {
                    Some("INVITE_REQUEST_SENT") => "Request to join sent",
                    _ => "Couldn't join the chat",
                };
                return tell(window, LinkTold::Said { text: told.into() });
            }
            if let Some(chat) = value["id"].as_i64() {
                tell(
                    window,
                    LinkTold::Open {
                        key: ChatKey::new(&key.account, chat),
                        title: text(&value["title"]).unwrap_or_default(),
                        message: None,
                        draft: None,
                    },
                );
            }
        }
        Ask::User {
            window,
            key,
            profile,
            draft,
        } => {
            let Some(user) = value["id"].as_i64().filter(|_| !error) else {
                return tell(
                    window,
                    LinkTold::Said {
                        text: "This person isn't on Telegram".into(),
                    },
                );
            };
            let account = key.account.clone();
            cx.td.send(
                &account,
                json!({"@type": "createPrivateChat", "user_id": user, "force": false}),
                Asked::Link(Ask::Private {
                    window,
                    key,
                    profile,
                    draft,
                }),
            );
        }
        Ask::Private {
            window,
            key,
            profile,
            draft,
        } => {
            let Some(chat) = value["id"].as_i64().filter(|_| !error) else {
                return;
            };
            let target = ChatKey::new(&key.account, chat);
            if profile {
                return tell(window, LinkTold::Profile { key: target });
            }
            tell(
                window,
                LinkTold::Open {
                    key: target,
                    title: text(&value["title"]).unwrap_or_default(),
                    message: None,
                    draft,
                },
            );
        }
        Ask::Stickers { window, key, url } => {
            let set = match &value["id"] {
                Value::String(id) => id.clone(),
                Value::Number(id) => id.to_string(),
                _ => String::new(),
            };
            if error || set.is_empty() {
                return tell(window, LinkTold::Unsupported { url });
            }
            tell(
                window,
                LinkTold::Stickers {
                    key,
                    set,
                    title: text(&value["title"]).unwrap_or_default(),
                    count: value["stickers"].as_array().map_or(0, |s| s.len() as u64),
                    installed: value["is_installed"] == true,
                },
            );
        }
        Ask::StickersAdded { window } => tell(
            window,
            LinkTold::Said {
                text: match error {
                    true => "Couldn't add the stickers",
                    false => "Stickers added",
                }
                .into(),
            },
        ),
    }
}

/// `getInternalLinkType`'s answer: the next request, or what the window is told.
fn internal(cx: &mut Cx, window: u64, key: ChatKey, url: String, kind: &Value) {
    let text = |value: &Value| {
        value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let account = key.account.clone();
    let (request, asked) = match kind["@type"].as_str().unwrap_or_default() {
        "internalLinkTypePublicChat" => (
            json!({"@type": "searchPublicChat", "username": kind["chat_username"]}),
            Ask::Public {
                window,
                key,
                profile: kind["open_profile"] == true,
                draft: text(&kind["draft_text"]),
                start: None,
            },
        ),
        "internalLinkTypeBotStart" => (
            json!({"@type": "searchPublicChat", "username": kind["bot_username"]}),
            Ask::Public {
                window,
                key,
                profile: false,
                draft: None,
                start: Some(
                    kind["start_parameter"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ),
            },
        ),
        "internalLinkTypeMessage" => (
            json!({"@type": "getMessageLinkInfo", "url": kind["url"]}),
            Ask::Message { window, key },
        ),
        "internalLinkTypeChatInvite" => {
            let link = kind["invite_link"].as_str().unwrap_or_default().to_string();
            (
                json!({"@type": "checkChatInviteLink", "invite_link": link}),
                Ask::Invite { window, key, link },
            )
        }
        "internalLinkTypePhoneNumber" => (
            json!({"@type": "searchUserByPhoneNumber", "phone_number": kind["phone_number"],
                "only_local": false}),
            Ask::User {
                window,
                key,
                profile: kind["open_profile"] == true,
                draft: text(&kind["draft_text"]),
            },
        ),
        "internalLinkTypeUserToken" => (
            json!({"@type": "searchUserByToken", "token": kind["token"]}),
            Ask::User {
                window,
                key,
                profile: false,
                draft: None,
            },
        ),
        "internalLinkTypeStickerSet" if kind["expect_custom_emoji"] != true => (
            json!({"@type": "searchStickerSet", "name": kind["sticker_set_name"]}),
            Ask::Stickers { window, key, url },
        ),
        // A proxy, a theme, the settings, a payment, a web app, a game, a story, Premium…
        _ => return tell(window, LinkTold::Unsupported { url }),
    };
    cx.td.send(&account, request, Asked::Link(asked));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_link_finds_its_way() {
        assert_eq!(
            Target::of("https://t.me/fluxnews"),
            Target::Telegram("https://t.me/fluxnews".into())
        );
        for link in [
            "http://telegram.me/anna",
            "https://telegram.dog/joinchat/abc",
            "t.me/+AbCdEf",
            "tg://resolve?domain=anna",
            "TG://join?invite=abc",
            "https://anna.t.me/",
            "https://www.t.me/anna",
        ] {
            assert!(matches!(Target::of(link), Target::Telegram(_)), "{link}");
        }
        for link in [
            "https://example.com/t.me",
            "https://t.me.example.com/",
            "https://nott.me/",
            "https://example.com/?u=https://t.me/x",
        ] {
            assert!(matches!(Target::of(link), Target::Web(_)), "{link}");
        }
        assert_eq!(
            Target::of("fluxgram:mention/anna"),
            Target::Mention("anna".into())
        );
        assert_eq!(Target::of("fluxgram:user/101"), Target::User(101));
        assert_eq!(
            Target::of("fluxgram:command//start@fluxbot"),
            Target::Command("/start@fluxbot".into())
        );
        assert_eq!(
            Target::of("fluxgram:tag/#release"),
            Target::Search("#release".into())
        );
        assert_eq!(
            Target::of("fluxgram:card/4242424242424242"),
            Target::Card("4242424242424242".into())
        );
        assert_eq!(
            Target::of("tel:+79991234567"),
            Target::Phone("+79991234567".into())
        );
        assert_eq!(
            Target::of("mailto:anna@example.com"),
            Target::Mail("mailto:anna@example.com".into())
        );
    }

    #[test]
    fn a_link_reads_as_its_spans() {
        use flux_plugin_api::conversation::{self as conv, rich};
        let message = Message::new("1", 1, 0).part(conv::text(vec![
            rich("see "),
            rich("Read ").link("https://evil.example/"),
            rich("more").bold().link("https://evil.example/"),
            rich(" and example.com").link("https://example.com"),
        ]));
        assert_eq!(
            link_text(&message, "https://evil.example/").as_deref(),
            Some("Read more")
        );
        assert_eq!(link_text(&message, "https://other.example/"), None);
    }

    #[test]
    fn telegrams_own_sites_open_at_once() {
        for url in [
            "https://telegram.org/blog",
            "http://ads.fragment.com/x?y=1",
            "https://telegra.ph/Release-11-19",
        ] {
            assert_eq!(
                web(url, Shown::Button, Policy::Mismatch),
                Web::Open,
                "{url}"
            );
        }
    }

    #[test]
    fn hosts_come_without_users_ports_and_case() {
        assert_eq!(
            host_of("https://anna@Example.COM:8080/x?y#z"),
            "example.com"
        );
        assert_eq!(host_of("example.com/path"), "example.com");
        assert_eq!(host_of("https://пример.рф/"), "пример.рф");
    }

    #[test]
    fn a_text_is_the_address_but_for_scheme_www_slash_and_punycode() {
        assert!(same_address("example.com", "https://www.example.com/"));
        assert!(same_address(
            "HTTP://Example.com/Path",
            "https://example.com/Path/"
        ));
        assert!(same_address("пример.рф", "http://xn--e1afmkfd.xn--p1ai"));
        assert!(same_address("anna@example.com", "mailto:anna@example.com"));
        assert!(!same_address(
            "example.com/Path",
            "https://example.com/path"
        ));
        assert!(!same_address("Read more", "https://example.com/"));
        assert!(!same_address("google.com", "https://g00gle.com/"));
    }

    #[test]
    fn punycode_goes_both_ways() {
        assert_eq!(punycode::encode("аpple").as_deref(), Some("pple-43d"));
        assert_eq!(punycode::encode("пример").as_deref(), Some("e1afmkfd"));
        assert_eq!(punycode::encode("bücher").as_deref(), Some("bcher-kva"));
        assert_eq!(punycode::decode("pple-43d").as_deref(), Some("аpple"));
        assert_eq!(punycode::decode("80ak6aa92e").as_deref(), Some("аррӏе"));
        assert_eq!(punycode::decode("bcher-kva").as_deref(), Some("bücher"));
        assert_eq!(punycode::decode("!!"), None);
        assert_eq!(
            with_ascii_host("https://аpple.com/login?x=1"),
            "https://xn--pple-43d.com/login?x=1"
        );
        assert_eq!(
            with_ascii_host("http://anna@пример.рф:8080/"),
            "http://anna@xn--e1afmkfd.xn--p1ai:8080/"
        );
    }

    #[test]
    fn look_alike_hosts_are_spoofed_and_ordinary_ones_are_not() {
        // Letters of two alphabets in one label.
        assert!(spoofed("аpple.com"));
        assert!(spoofed("xn--pple-43d.com"));
        assert!(spoofed("pаypal.example.org"));
        assert!(spoofed("gοogle.com"));
        // One alphabet that passes for Latin, under a Latin top-level domain.
        assert!(spoofed("аррӏе.com"));
        assert!(spoofed("xn--80ak6aa92e.com"));
        assert!(spoofed("ɑpple.com"));
        // Ordinary names.
        assert!(!spoofed("example.com"));
        assert!(!spoofed("пример.рф"));
        assert!(!spoofed("xn--e1afmkfd.xn--p1ai"));
        assert!(!spoofed("bücher.de"));
        assert!(!spoofed("сайт.рф"));
        assert!(!spoofed("яндекс.com"));
        assert!(!spoofed("日本.jp"));
        assert!(!spoofed("127.0.0.1"));
    }

    #[test]
    fn web_links_are_asked_as_the_setting_says() {
        let open = |url: &str, text: Option<&str>, policy| {
            web(url, text.map_or(Shown::Address, Shown::Text), policy)
        };
        let asked = |url: &str| Web::Ask {
            shown: url.to_string(),
            spoofed: false,
        };
        // A plain address typed in a message opens at once; a text link is asked.
        assert_eq!(
            open("https://example.com", Some("example.com"), Policy::Mismatch),
            Web::Open
        );
        assert_eq!(
            open("https://example.com", None, Policy::Mismatch),
            Web::Open
        );
        assert_eq!(
            open("https://evil.example/", Some("Read more"), Policy::Mismatch),
            asked("https://evil.example/")
        );
        assert_eq!(
            open("https://example.com", Some("example.com"), Policy::Always),
            asked("https://example.com")
        );
        assert_eq!(
            open("https://evil.example/", Some("Read more"), Policy::Never),
            Web::Open
        );
        assert_eq!(
            web("https://shop.example/", Shown::Button, Policy::Mismatch),
            asked("https://shop.example/")
        );
        // Telegram's sites open at once, unless every link is asked.
        assert_eq!(
            open("https://telegra.ph/x", Some("Article"), Policy::Mismatch),
            Web::Open
        );
        // A spoofed host is asked whatever the setting, in punycode.
        assert_eq!(
            open("https://аpple.com/", Some("аpple.com"), Policy::Never),
            Web::Ask {
                shown: "https://xn--pple-43d.com/".into(),
                spoofed: true
            }
        );
        assert_eq!(Policy::of(Some("always")), Policy::Always);
        assert_eq!(Policy::of(Some("never")), Policy::Never);
        assert_eq!(Policy::of(None), Policy::Mismatch);
    }

    #[test]
    fn asks_and_tolds_travel_as_json() {
        let ask = crate::model::Request::Link(LinkAsk::Open {
            key: ChatKey::new("a", 1),
            url: "https://t.me/anna".into(),
        });
        let line = serde_json::to_string(&ask).unwrap();
        assert_eq!(
            serde_json::from_str::<crate::model::Request>(&line).unwrap(),
            ask
        );
        let told = Note::Link(LinkTold::Invite {
            key: ChatKey::new("a", 1),
            link: "https://t.me/+abc".into(),
            title: "Flux".into(),
            members: 5,
            channel: false,
            request: false,
        });
        let line = serde_json::to_string(&told).unwrap();
        assert_eq!(serde_json::from_str::<Note>(&line).unwrap(), told);
    }
}
