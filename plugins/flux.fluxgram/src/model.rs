//! What needs no host, so it compiles and is tested natively (`cargo test`): the messages between
//! the service and the windows, TDLib's authorization states and errors read into what the
//! sign-in dialog shows, the masked phone number, the helper's output split into lines.
//!
//! Times that both sides read (when a code can be sent again) are the wall clock's seconds: the
//! service and the windows are separate instances, but they share the clock.
//!
//! The service sends the windows structured values, not text: each window translates what it
//! shows (`Problem::text` gives the English line, the window passes it through `tr`). The chats
//! (part 3) are the exception where the service has the words: a list's last line and a feed's
//! messages are made once, in the service, through [`Words`] (the host's `tr` there, English in
//! the tests).

use flux_plugin_api::conversation::{AvatarSpec, Change, Composed};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// --- The service ⇄ the windows ---

/// The accounts and the sign-in, small (a few accounts and the sign-in in progress): sent as
/// [`Note::State`] only when it changed — and to a window that has just started.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub accounts: Vec<AccountView>,
    /// The sign-in in progress; none — the dialog has nothing to show.
    pub sign_in: Option<SignIn>,
}

/// An account as the windows draw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountView {
    pub key: String,
    pub name: String,
    /// Masked: "+7 ••• •••-••-12".
    pub phone: String,
    /// "@username", when the account has one.
    pub label: Option<String>,
    /// One of the seven avatar colors.
    pub color: u8,
    /// The small profile photo, absolute; none — the initials.
    pub photo: Option<String>,
    /// Sign Out was confirmed: Telegram is ending the session.
    pub signing_out: bool,
}

impl AccountView {
    /// The name's first word.
    pub fn first_name(&self) -> &str {
        self.name.split_whitespace().next().unwrap_or(&self.name)
    }
}

/// What tells the accounts apart where there are several (a chat's header, the Add Chat popup's
/// groups), by key: the first name; the full name where two share it; the masked number after
/// it where two share that too.
pub fn account_labels(accounts: &[AccountView]) -> std::collections::HashMap<String, String> {
    fn first(account: &AccountView) -> &str {
        account.first_name()
    }
    fn full(account: &AccountView) -> &str {
        &account.name
    }
    let shared = |label: fn(&AccountView) -> &str, account: &AccountView| {
        accounts
            .iter()
            .filter(|other| label(other) == label(account))
            .count()
            > 1
    };
    accounts
        .iter()
        .map(|account| {
            let label = if !shared(first, account) {
                account.first_name().to_string()
            } else if !shared(full, account) {
                account.name.clone()
            } else {
                format!("{} · {}", account.name, account.phone)
            };
            (account.key.clone(), label)
        })
        .collect()
}

/// The sign-in dialog: its screen, whether a request is on its way, what went wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignIn {
    pub screen: Screen,
    /// TDLib works on what the user asked (Next): the screen's main button is busy.
    pub busy: bool,
    /// The code is being sent again: the resend button is busy.
    #[serde(default)]
    pub resending: bool,
    pub problem: Option<Problem>,
}

/// The sign-in dialog's screen. While TDLib works between two screens (the client starts, the
/// account is read) the screen on show stays, its button busy: there is no screen of waiting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "screen", rename_all = "kebab-case")]
pub enum Screen {
    /// FluxGram has no app key with Telegram yet (`config::API_ID`).
    NotSetUp,
    /// The QR code of `link` for the phone's camera; an empty link — TDLib is getting one (the
    /// client starts, the code is asked for): a placeholder of the code's size.
    Qr { link: String },
    /// The phone number.
    Phone,
    /// The code: where it went, how else it can come, and when it can be asked for that way —
    /// the wall clock's seconds ([`unix_now`]).
    Code {
        phone: String,
        sent: CodeSent,
        resend: Option<CodeSent>,
        #[serde(default)]
        resend_at: u64,
    },
    /// The account's cloud password (2FA), with its hint.
    Password { hint: String },
    /// Telegram wants an email address for the codes.
    EmailAddress,
    /// The code sent to the email address `pattern` ("e***@mail.com").
    EmailCode { pattern: String },
    /// The number has no Telegram account: FluxGram doesn't sign up.
    NoAccount,
}

/// Where a sign-in code went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodeSent {
    /// A message in Telegram on the user's other device.
    App,
    Sms,
    Call,
    /// A call whose number is the code.
    FlashCall,
    /// A missed call: the code is the last digits of the calling number.
    MissedCall,
    /// An anonymous number bought on Telegram's marketplace.
    Fragment,
    Other,
}

/// What went wrong in the sign-in: a calm line under the field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "problem", content = "value", rename_all = "kebab-case")]
pub enum Problem {
    WrongCode,
    CodeExpired,
    WrongPassword,
    InvalidPhone,
    BannedPhone,
    PhoneFlood,
    InvalidEmail,
    /// Telegram has no other way to send the code now.
    NoOtherWay,
    /// Too many attempts: minutes to wait.
    TryLater(u64),
    /// Telegram refused FluxGram's app key.
    KeyRefused,
    AlreadyAdded,
    ConnectionLost,
    /// Anything else: TDLib's message.
    Failed(String),
}

impl Problem {
    /// The English line: a template for `trf` with the value in `{0}`, and the value.
    pub fn text(&self) -> (&'static str, Option<String>) {
        match self {
            Problem::WrongCode => ("Wrong code. Check it and try again.", None),
            Problem::CodeExpired => ("The code has expired. Ask for a new one.", None),
            Problem::WrongPassword => ("Wrong password.", None),
            Problem::InvalidPhone => (
                "That doesn't look like a phone number. Include the country code.",
                None,
            ),
            Problem::BannedPhone => ("Telegram doesn't let this number sign in.", None),
            Problem::PhoneFlood => (
                "Too many codes were asked for this number. Try again later.",
                None,
            ),
            Problem::InvalidEmail => ("That doesn't look like an email address.", None),
            Problem::NoOtherWay => ("Telegram can't send the code another way now.", None),
            Problem::TryLater(minutes) if *minutes <= 1 => {
                ("Too many attempts. Try again in a minute.", None)
            }
            Problem::TryLater(minutes) => (
                "Too many attempts. Try again in {0} min.",
                Some(minutes.to_string()),
            ),
            Problem::KeyRefused => (
                "Telegram refused FluxGram's app key. Update the plugin.",
                None,
            ),
            Problem::AlreadyAdded => ("This account is already in FluxGram.", None),
            Problem::ConnectionLost => ("FluxGram lost its connection. Try again.", None),
            Problem::Failed(message) => ("Telegram couldn't do that: {0}", Some(message.clone())),
        }
    }
}

/// What a window asks of the service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "kebab-case")]
pub enum Request {
    /// Send me the state (a window whose service has just started).
    Hello,
    /// Add Account: start a sign-in (or show the one in progress).
    AddAccount,
    /// Use Phone Number on the QR screen.
    UsePhone,
    /// Sign In with QR Code: from the phone or the code screen back to the QR code.
    UseQr,
    /// Back: the previous screen.
    Back,
    /// ↵ or Next: the screen's field.
    Submit {
        text: String,
    },
    /// Send the code again (another way).
    Resend,
    /// Cancel, Esc, ×: the sign-in is dropped.
    Cancel,
    /// Sign Out of an account, confirmed.
    SignOut {
        key: String,
    },

    // --- The list (part 3, `chats.rs`) ---
    /// Add a chat to the account's list (the Add Chat popup's choice).
    AddChat {
        key: ChatKey,
    },
    /// Remove a chat from the list: FluxGram stops showing it; it stays in Telegram.
    RemoveChat {
        key: ChatKey,
    },
    /// The Add Chat popup's query (empty — the recent chats): the answer is [`Note::Found`] with
    /// the same `request`, to this window only.
    SearchChats {
        account: String,
        query: String,
        request: u64,
    },
    /// A chat's tab came into sight, or went out of it: no notifications for a chat in sight.
    InSight {
        key: ChatKey,
        in_sight: bool,
    },

    // --- A chat (part 3, `feed.rs`) ---
    /// A chat's tab opened (or came back with the project): the service sends this window the
    /// chat's [`Note::Header`] and its feed ([`Note::Feed`]) from where the user stopped reading,
    /// then every change of them, until [`Request::CloseChat`].
    OpenChat {
        key: ChatKey,
    },
    /// The chat's tab closed in this window.
    CloseChat {
        key: ChatKey,
    },
    /// The feed scrolled near its oldest (`older`) or newest loaded message: a page more.
    LoadOlder {
        key: ChatKey,
    },
    LoadNewer {
        key: ChatKey,
    },
    /// The messages the user sees, the first and the last (the conversation's `visible`): they
    /// are read, and their pictures go first.
    Visible {
        key: ChatKey,
        first: String,
        last: String,
    },
    /// The field was sent: the text, the files, the message answered; `silent` — Send Without
    /// Sound (part 5): the chat gets no notification.
    Send {
        key: ChatKey,
        composed: Composed,
        #[serde(default)]
        silent: bool,
        /// Messages to forward after the text (part 5): their chat and ids, in order.
        #[serde(default)]
        forward: Option<(ChatKey, Vec<String>)>,
    },
    /// The field's word asks for suggestions (part 5): "@" — the members to mention matching
    /// `text`, "/" — the bots' commands; the answer is a [`Note::Feed`] of `suggestions` to this
    /// window only.
    Suggest {
        key: ChatKey,
        trigger: String,
        text: String,
    },
    /// A message's new text (the field in editing).
    Edit {
        key: ChatKey,
        message: String,
        text: String,
    },
    /// Delete a message, confirmed by the user.
    Delete {
        key: ChatKey,
        message: String,
    },
    /// Delete selected messages, confirmed by the user (part 5): for everyone (`revoke`), or
    /// only for the user.
    DeleteMany {
        key: ChatKey,
        messages: Vec<String>,
        revoke: bool,
    },
    /// Forward messages of the chat `from` into the chat `key` (part 5), in this order, with no
    /// text (`Send`'s `forward` goes after one); the account is the same (Telegram forwards only
    /// within one).
    Forward {
        key: ChatKey,
        from: ChatKey,
        messages: Vec<String>,
        silent: bool,
    },
    /// A part the user clicked that isn't there yet: a file, a picture, a video — download it.
    Download {
        key: ChatKey,
        message: String,
        part: u32,
    },
    /// A quote's message isn't loaded: load the feed around it, then reveal it.
    JumpTo {
        key: ChatKey,
        message: String,
    },
    /// The field's text changed: "typing…" to the chat, and the draft kept in Telegram.
    Draft {
        key: ChatKey,
        text: String,
    },

    /// The reply over the field quotes a part of its message (the selected text and where it
    /// starts in the message's text, in UTF-16 units), or the whole message again (none): the
    /// next message sent in reply to `message` carries the quote.
    Quote {
        key: ChatKey,
        message: String,
        quote: Option<(String, u32)>,
    },
    /// A voice or round message (the message's `part`) played for the first time (part 4): it is
    /// listened to in Telegram.
    MediaPlayed {
        key: ChatKey,
        message: String,
        part: u32,
    },
    /// The field's panel of stickers and GIFs (part 4, `panel.rs`).
    Panel(crate::panel::PanelAsk),
    /// Clear Cache in Settings, confirmed: the media files and the rendered animations go.
    ClearCache,

    // --- Profiles and media (part 3+, `profile.rs`) ---
    /// What the profile dialog asks: a person's or a group's profile, the members, the media.
    Profile(crate::profile::ProfileAsk),

    // --- Search in a chat, hidden chats (part 3+, `feed.rs`, `chats.rs`) ---
    /// A chat's search: the matches of a query, the next one.
    Search(crate::feed::SearchAsk),
    /// The hidden chats: the phrase typed in the list's search, hide or show a chat.
    Hidden(crate::chats::HiddenAsk),

    // --- FluxGram's folders (`chats.rs`) ---
    /// Create, rename, delete a folder; put a chat in one, or take it out.
    Folders(crate::chats::FolderAsk),

    // --- The pinned messages and the chat's menus (part 5, `feed.rs`) ---
    /// The pinned message's bar was clicked on `message`: it is revealed (the feed loaded around
    /// it when it isn't), and the bar goes to the next older pinned one.
    PinnedClicked {
        key: ChatKey,
        message: String,
    },
    /// Pin a message, confirmed: `notify` — the group's members are told; `for_both` — pinned
    /// for the other person of a one-to-one chat too.
    Pin {
        key: ChatKey,
        message: String,
        notify: bool,
        for_both: bool,
    },
    /// Unpin a message, confirmed.
    Unpin {
        key: ChatKey,
        message: String,
    },
    /// The bar's × where the user can't unpin: no bar in the chat until a newer message is
    /// pinned; `remember` — after a restart too (never for a hidden chat: no trace of it).
    HidePinned {
        key: ChatKey,
        message: String,
        remember: bool,
    },
    /// Mark the chat as read (every message, mention and reaction), or as unread.
    MarkRead {
        key: ChatKey,
        read: bool,
    },
    /// Copy a message's link: the service tells this window ([`Note::Copied`]).
    CopyLink {
        key: ChatKey,
        message: String,
    },
    /// A sticker message's sticker to the favorites (`keep`) or out of them; a GIF message's GIF
    /// to the saved ones.
    KeepMedia {
        key: ChatKey,
        message: String,
        keep: bool,
    },
    // --- Channels and bots (part 5) ---
    /// Join the channel (the field's button «Join»).
    Join {
        key: ChatKey,
    },
    /// An advertisement clicked or reported (`ads.rs`).
    Ad(crate::ads::AdAsk),
    /// A bot's button (`bots.rs`): a callback — the bot's answer comes to this window
    /// ([`Note::BotAnswer`]); a text of the keyboard over the field, sent.
    BotCallback {
        key: ChatKey,
        message: String,
        data: String,
    },
    BotText {
        key: ChatKey,
        text: String,
    },
    /// Start the bot (the field's «Start»: `sendBotStartMessage`, with the parameter of the link
    /// that opened the chat); `restart` — unblock it first (⋯ → Restart Bot).
    BotStart {
        key: ChatKey,
        restart: bool,
    },
    /// Block the chat's bot or person (⋯ → Stop Bot, confirmed), or unblock it.
    Block {
        key: ChatKey,
        blocked: bool,
    },
    // --- Links (part 5, `links.rs`) ---
    /// A Telegram link, a mention, an invite or a sticker set to open: the answer is a
    /// [`Note::Link`] to this window only.
    Link(crate::links::LinkAsk),
    // --- Polls and reactions (part 5) ---
    /// A vote in the poll of `message`: the options' indices; none — the vote taken back
    /// (Retract Vote).
    Vote {
        key: ChatKey,
        message: String,
        options: Vec<u32>,
    },
    /// Stop the poll of `message`, confirmed: its results are final.
    StopPoll {
        key: ChatKey,
        message: String,
    },
    /// The user's reaction `emoji` on `message`: taken back when it is the user's, else put
    /// (the oldest of the user's own going first when they are at the limit).
    React {
        key: ChatKey,
        message: String,
        emoji: String,
    },
}

// --- The chats (part 3) ---

/// A chat of an account: a tab's key (`<account>:<chat id>`), the list's row, a request's subject.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChatKey {
    pub account: String,
    pub chat: i64,
}

impl ChatKey {
    pub fn new(account: &str, chat: i64) -> Self {
        ChatKey {
            account: account.to_string(),
            chat,
        }
    }

    /// The key of the chat's tab, `<account>:<chat id>`: the window `chat/<key>`.
    pub fn tab(&self) -> String {
        format!("{}:{}", self.account, self.chat)
    }

    /// The chat of a tab's key.
    pub fn parse(tab: &str) -> Option<Self> {
        let (account, chat) = tab.split_once(':')?;
        Some(ChatKey::new(account, chat.parse().ok()?))
    }
}

/// What the service tells the windows: the whole state ([`Snapshot`]), an account's list, a
/// chat's header and feed, the Add Chat popup's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "note", rename_all = "kebab-case")]
pub enum Note {
    /// The accounts and the sign-in: to every window, when it changed.
    State(Snapshot),
    /// The list of an account: the chats the user added, in Telegram's order (pinned first, then
    /// the newest); to every window, when it changed.
    Chats { account: String, rows: Vec<ChatRow> },
    /// A chat's header: to the windows that have the chat open, when it changed.
    Header { key: ChatKey, header: ChatHeader },
    /// Changes of a chat's feed, in order: to the windows that have the chat open. The window
    /// hands them to its conversation as they are (`conversation::update`).
    Feed { key: ChatKey, changes: Vec<Change> },
    /// The chats found for the Add Chat popup's `request`: to the window that asked.
    Found { request: u64, chats: Vec<FoundChat> },
    /// The profile dialog's answers (`profile.rs`): to the window that asked.
    Profile(crate::profile::ProfileTold),
    /// A chat's search's answers (`feed.rs`): to the window that asked.
    Search(crate::feed::SearchTold),
    /// The hidden chats' state (`chats.rs`): to every window.
    Hidden(crate::chats::HiddenTold),
    /// FluxGram's folders, whole (`chats.rs`): to every window when they changed, and to a window
    /// that has just started.
    Folders { folders: Vec<crate::chats::Folder> },
    /// A message's link for the clipboard (part 5): to the window that asked, which copies it and
    /// says so.
    Copied { link: String },
    /// The chat's advertisement (part 5, `ads.rs`): what the window's clicks and About the Ad
    /// need; none — no ad. To the windows that have the chat open.
    Ad {
        key: ChatKey,
        ad: Option<crate::ads::AdInfo>,
    },
    /// A bot's answer to a button (part 5): a notification, or a calm dialog (`alert`) under the
    /// bot's name; a page to open. To the window that asked.
    BotAnswer {
        key: ChatKey,
        bot: String,
        text: String,
        alert: bool,
        url: String,
    },
    /// A bot asks for a reply to `message` (part 5): its quote goes over the field. To the
    /// windows that have the chat open.
    ForceReply { key: ChatKey, message: String },
    /// Report the ad: the options Telegram offers (id, text) under its title — the window asks
    /// the user to choose. To the window that asked.
    AdReport {
        key: ChatKey,
        message: String,
        title: String,
        options: Vec<(String, String)>,
    },
    /// What a link opens (part 5, `links.rs`): to the window that asked.
    Link(crate::links::LinkTold),
}

/// A row of the list on the left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRow {
    pub chat: i64,
    pub title: String,
    pub avatar: AvatarSpec,
    /// The last message in a line, in the interface's language: "Petya: Release on Friday?",
    /// "You: See you at 11?", "Photo"; none — no messages.
    pub last: Option<String>,
    /// The last message's time (seconds since 1970-01-01 UTC): the window shows "14:32",
    /// "Yesterday", "Mon", a date. 0 — none.
    pub date: i64,
    /// The last message is the user's: its mark (✓, ✓✓, a clock, a red one).
    pub delivery: Option<flux_plugin_api::conversation::Delivery>,
    pub unread: u32,
    /// Mentions of the user among the unread ones.
    pub mentions: u32,
    pub muted: bool,
    /// Pinned in Telegram's main list.
    pub pinned: bool,
    /// The user's unsent draft: the row shows "Draft: …" in place of the last message.
    pub draft: Option<String>,
    /// A chat with a bot, and the bot stopped (part 5): the row's ⋯ offers Stop Bot or Restart
    /// Bot, as the tab's does.
    #[serde(default)]
    pub bot: bool,
    #[serde(default)]
    pub blocked: bool,
}

/// A chat's header in its tab: the title, the picture, who it is and what is going on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatHeader {
    pub title: String,
    pub avatar: AvatarSpec,
    /// The line under the title, without the typing: "online", "last seen at 14:32",
    /// "5 members" — in the interface's language.
    pub status: String,
    /// The status line is "online": the accent color.
    pub online: bool,
    /// Who is typing (or sending a photo…), in the interface's language: "Petya is typing…";
    /// none — nobody. It takes the status line's place.
    pub typing: Option<String>,
    /// The user may write here: the field; otherwise a feed only.
    pub can_send: bool,
    pub muted: bool,
    /// Unread messages, or marked as unread (part 5): ⋯ offers Mark as Read, else Mark as Unread.
    #[serde(default)]
    pub unread: bool,
    /// Saved Messages (part 5): nobody to notify — no Send Without Sound.
    #[serde(default)]
    pub saved: bool,
    /// A channel (part 5): where the user can't post, a button in place of the field — Mute or
    /// Unmute, or Join when the user isn't in it (`joined`).
    #[serde(default)]
    pub channel: bool,
    #[serde(default)]
    pub joined: bool,
    /// A one-to-one chat with a bot (part 5): ⋯ offers Stop Bot, or Restart Bot when `blocked`;
    /// with no messages yet, «Start» in place of the field; blocked, «Unblock».
    #[serde(default)]
    pub bot: bool,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub empty: bool,
    /// The bot's menu (part 5): its commands ("/start", the description) — the field's Menu
    /// button; empty — no menu (none, or the bot's menu is an app).
    #[serde(default)]
    pub commands: Vec<(String, String)>,
    /// The reactions the user may put in the chat (part 5), in order: the chat's own list, or
    /// Telegram's when it allows all; empty — none (only shown).
    #[serde(default)]
    pub reactions: Vec<String>,
}

/// A chat the Add Chat popup offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FoundChat {
    pub chat: i64,
    pub title: String,
    pub avatar: AvatarSpec,
    /// Dim after the title: "@anna", "5 members", "group".
    pub detail: Option<String>,
    /// Already in the list.
    pub added: bool,
    /// The user's own chat, Saved Messages (part 5): on top of the Forward picker.
    #[serde(default)]
    pub saved: bool,
}

/// The words of the chats the service makes: the host's translation in the plugin, English in
/// the tests ([`English`]).
pub trait Words {
    /// The text in the interface's language.
    fn tr(&self, text: &str) -> String;
    /// A template with `{0}`, `{1}`… replaced by `values`, translated first.
    fn trf(&self, template: &str, values: &[&str]) -> String;
    /// A count in words, `{0}` the number: "1 member", "5 members". English takes `one` for 1 and
    /// `other` for the rest; Russian has three forms, translated as `"<other>|one"`,
    /// `"<other>|few"` and `"<other>|many"` in `ru.toml` ("{0} members|few" = "{0} участника").
    fn count(&self, n: u64, one: &str, other: &str) -> String;
}

/// The Russian plural form of `n`: "one" (1, 21, 101), "few" (2–4, 22–24), "many" (the rest).
pub fn russian_form(n: u64) -> &'static str {
    match (n % 10, n % 100) {
        (1, rest) if rest != 11 => "one",
        (2..=4, rest) if !(12..=14).contains(&rest) => "few",
        _ => "many",
    }
}

/// The words as they are written: for the tests.
#[cfg(test)]
pub struct English;

#[cfg(test)]
impl Words for English {
    fn tr(&self, text: &str) -> String {
        text.to_string()
    }

    fn trf(&self, template: &str, values: &[&str]) -> String {
        let mut text = template.to_string();
        for (index, value) in values.iter().enumerate() {
            text = text.replace(&format!("{{{index}}}"), value);
        }
        text
    }

    fn count(&self, n: u64, one: &str, other: &str) -> String {
        let template = if n == 1 { one } else { other };
        template.replace("{0}", &n.to_string())
    }
}

// --- TDLib's authorization ---

/// TDLib's authorization state of a client, as far as FluxGram tells them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    WaitParameters,
    WaitPhone,
    WaitQr {
        link: String,
    },
    WaitCode {
        phone: String,
        sent: CodeSent,
        next: Option<CodeSent>,
        /// When `next` can be asked for: the wall clock's seconds.
        resend_at: u64,
    },
    WaitPassword {
        hint: String,
    },
    WaitEmail,
    WaitEmailCode {
        pattern: String,
    },
    /// The number has no account (or Telegram wants it to sign up some other way).
    WaitRegistration,
    Ready,
    LoggingOut,
    Closing,
    Closed,
    Other,
}

impl Auth {
    /// Signed out: a state of a client without an account (or on its way to none).
    pub fn is_signed_out(&self) -> bool {
        !matches!(
            self,
            Auth::WaitParameters | Auth::Ready | Auth::Closing | Auth::Closed | Auth::Other
        )
    }

    /// A state that comes once TDLib has taken the phone number: the phone screen is done.
    pub fn after_phone(&self) -> bool {
        matches!(
            self,
            Auth::WaitCode { .. }
                | Auth::WaitPassword { .. }
                | Auth::WaitEmail
                | Auth::WaitEmailCode { .. }
                | Auth::WaitRegistration
                | Auth::Ready
        )
    }

    /// TDLib takes a new phone number or a QR request in this state (no request pending).
    pub fn takes_phone(&self) -> bool {
        matches!(
            self,
            Auth::WaitPhone
                | Auth::WaitCode { .. }
                | Auth::WaitPassword { .. }
                | Auth::WaitEmail
                | Auth::WaitEmailCode { .. }
                | Auth::WaitRegistration
        )
    }
}

fn text_of(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

/// Reads `authorization_state` of `updateAuthorizationState` that came at `now` (the wall clock's
/// seconds): a code's timeout becomes the moment it ends.
pub fn parse_auth(state: &Value, now: u64) -> Auth {
    match state["@type"].as_str().unwrap_or_default() {
        "authorizationStateWaitTdlibParameters" => Auth::WaitParameters,
        "authorizationStateWaitPhoneNumber" => Auth::WaitPhone,
        "authorizationStateWaitOtherDeviceConfirmation" => Auth::WaitQr {
            link: text_of(state, "link"),
        },
        "authorizationStateWaitCode" => {
            let info = &state["code_info"];
            Auth::WaitCode {
                phone: text_of(info, "phone_number"),
                sent: code_sent(&info["type"]),
                next: info
                    .get("next_type")
                    .filter(|next| next.is_object())
                    .map(code_sent),
                resend_at: now + info["timeout"].as_u64().unwrap_or_default(),
            }
        }
        "authorizationStateWaitPassword" => Auth::WaitPassword {
            hint: text_of(state, "password_hint"),
        },
        "authorizationStateWaitEmailAddress" => Auth::WaitEmail,
        "authorizationStateWaitEmailCode" => Auth::WaitEmailCode {
            pattern: text_of(&state["code_info"], "email_address_pattern"),
        },
        "authorizationStateWaitRegistration" | "authorizationStateWaitPremiumPurchase" => {
            Auth::WaitRegistration
        }
        "authorizationStateReady" => Auth::Ready,
        "authorizationStateLoggingOut" => Auth::LoggingOut,
        "authorizationStateClosing" => Auth::Closing,
        "authorizationStateClosed" => Auth::Closed,
        _ => Auth::Other,
    }
}

/// Where an `authenticationCodeType…` sends the code.
fn code_sent(kind: &Value) -> CodeSent {
    match kind["@type"].as_str().unwrap_or_default() {
        "authenticationCodeTypeTelegramMessage" => CodeSent::App,
        "authenticationCodeTypeSms"
        | "authenticationCodeTypeSmsWord"
        | "authenticationCodeTypeSmsPhrase"
        | "authenticationCodeTypeFirebaseAndroid"
        | "authenticationCodeTypeFirebaseIos" => CodeSent::Sms,
        "authenticationCodeTypeCall" => CodeSent::Call,
        "authenticationCodeTypeFlashCall" => CodeSent::FlashCall,
        "authenticationCodeTypeMissedCall" => CodeSent::MissedCall,
        "authenticationCodeTypeFragment" => CodeSent::Fragment,
        _ => CodeSent::Other,
    }
}

/// The dialog's screen for a sign-in in TDLib's state `auth`; `phone_mode` — the user chose the
/// number over the QR code. None while TDLib works between two screens (the account is read, the
/// client closes): the screen on show stays.
pub fn screen(auth: &Auth, phone_mode: bool) -> Option<Screen> {
    let screen = match auth {
        // The client starts, or waits for the number or for the QR request's answer.
        Auth::WaitParameters | Auth::WaitPhone if phone_mode => Screen::Phone,
        Auth::WaitParameters | Auth::WaitPhone => Screen::Qr {
            link: String::new(),
        },
        // A QR code that came after the user chose the number: its client is being replaced.
        Auth::WaitQr { .. } if phone_mode => Screen::Phone,
        Auth::WaitQr { link } => Screen::Qr { link: link.clone() },
        Auth::WaitCode {
            phone,
            sent,
            next,
            resend_at,
        } => Screen::Code {
            phone: phone.clone(),
            sent: *sent,
            resend: *next,
            resend_at: *resend_at,
        },
        Auth::WaitPassword { hint } => Screen::Password { hint: hint.clone() },
        Auth::WaitEmail => Screen::EmailAddress,
        Auth::WaitEmailCode { pattern } => Screen::EmailCode {
            pattern: pattern.clone(),
        },
        Auth::WaitRegistration => Screen::NoAccount,
        Auth::Ready | Auth::LoggingOut | Auth::Closing | Auth::Closed | Auth::Other => {
            return None;
        }
    };
    Some(screen)
}

/// What Telegram said of a sign-in code, for the log: how it was sent, how it can come next and
/// after how long — never the number (nor the code, which never comes from TDLib).
pub fn code_log(state: &Value) -> Option<String> {
    if state["@type"] != "authorizationStateWaitCode" {
        return None;
    }
    let info = &state["code_info"];
    let kind = |kind: &Value| {
        let name = kind["@type"].as_str().unwrap_or("?").to_string();
        match kind["length"].as_u64().filter(|length| *length > 0) {
            Some(length) => format!("{name} ({length} digits)"),
            None => name,
        }
    };
    let next = match &info["next_type"] {
        next if next.is_object() => kind(next),
        _ => "none".to_string(),
    };
    Some(format!(
        "Sign-in: the code was sent as {}; next: {next}, in {} s",
        kind(&info["type"]),
        info["timeout"].as_u64().unwrap_or_default()
    ))
}

/// The wall clock's seconds: when a code can be sent again, as both instances read it.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A countdown's "0:58", "1:30", "12:00".
pub fn countdown(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// What the service sends by itself when a sign-in's client comes to `auth`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// `setTdlibParameters`.
    Parameters,
    /// `requestQrCodeAuthentication`.
    RequestQr,
    /// `setAuthenticationPhoneNumber` with the number typed before the client was ready.
    SendPhone(String),
    /// `getMe`: signed in.
    GetMe,
    /// A fresh client: a QR code came after the user chose the number, and TDLib doesn't take a
    /// number while its QR code waits.
    Renew,
    Nothing,
}

/// The next request of a sign-in: `phone_mode` — the user chose the number over the QR code;
/// `queued` — a number typed while the client was starting.
pub fn next_step(auth: &Auth, phone_mode: bool, queued: Option<&str>) -> Next {
    match auth {
        Auth::WaitParameters => Next::Parameters,
        Auth::WaitPhone => match queued {
            Some(phone) => Next::SendPhone(phone.to_string()),
            None if !phone_mode => Next::RequestQr,
            None => Next::Nothing,
        },
        Auth::WaitQr { .. } if phone_mode => Next::Renew,
        Auth::Ready => Next::GetMe,
        _ => Next::Nothing,
    }
}

/// TDLib's error (`code`, `message`) as a line for the user.
pub fn problem(code: i64, message: &str) -> Problem {
    if let Some(seconds) = flood_seconds(message) {
        return Problem::TryLater(seconds.div_ceil(60).max(1));
    }
    let upper = message.to_ascii_uppercase();
    let has = |what: &str| upper.contains(what);
    if has("CODE_INVALID") || has("PHONE_CODE_EMPTY") {
        Problem::WrongCode
    } else if has("PHONE_CODE_EXPIRED") || has("CODE_EXPIRED") {
        Problem::CodeExpired
    } else if has("PASSWORD_HASH_INVALID") || has("PASSWORD_INVALID") {
        Problem::WrongPassword
    } else if has("PHONE_NUMBER_INVALID") || has("PHONE_NUMBER_EMPTY") {
        Problem::InvalidPhone
    } else if has("PHONE_NUMBER_BANNED") || has("PHONE_NUMBER_FROZEN") {
        Problem::BannedPhone
    } else if has("PHONE_NUMBER_FLOOD") || has("PHONE_PASSWORD_FLOOD") {
        Problem::PhoneFlood
    } else if has("EMAIL_INVALID") || has("EMAIL_ADDRESS_INVALID") {
        Problem::InvalidEmail
    } else if has("SEND_CODE_UNAVAILABLE") || has("CAN'T BE RESEN") {
        // Telegram's own error, and TDLib's "Authentication code can't be resend".
        Problem::NoOtherWay
    } else if has("API_ID_INVALID") || has("API_ID_PUBLISHED_FLOOD") || has("API_HASH") {
        Problem::KeyRefused
    } else if code == 429 {
        Problem::TryLater(1)
    } else {
        Problem::Failed(message.to_string())
    }
}

/// The seconds of "FLOOD_WAIT_123" or TDLib's "Too Many Requests: retry after 123".
fn flood_seconds(message: &str) -> Option<u64> {
    let digits = ["FLOOD_WAIT_", "retry after "].iter().find_map(|prefix| {
        let at = message.find(prefix)?;
        Some(&message[at + prefix.len()..])
    })?;
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse().ok()
}

/// `setTdlibParameters` of an account: its folders, the database's key, the app key.
pub struct Parameters<'a> {
    /// `<data>/accounts/<key>`.
    pub folder: &'a str,
    /// The database's encryption key, base64.
    pub database_key: &'a str,
    pub language: &'a str,
    pub test_dc: bool,
    pub api_id: i32,
    pub api_hash: &'a str,
}

impl Parameters<'_> {
    pub fn request(&self) -> Value {
        json!({
            "@type": "setTdlibParameters",
            "use_test_dc": self.test_dc,
            "database_directory": format!("{}/db", self.folder),
            "files_directory": format!("{}/files", self.folder),
            "database_encryption_key": self.database_key,
            "use_file_database": true,
            "use_chat_info_database": true,
            "use_message_database": true,
            "use_secret_chats": false,
            "api_id": self.api_id,
            "api_hash": self.api_hash,
            "system_language_code": self.language,
            "device_model": "Flux",
            "system_version": "",
            "application_version": concat!("FluxGram ", env!("CARGO_PKG_VERSION")),
        })
    }
}

// --- Users ---

/// What FluxGram keeps of the signed-in user (TDLib's `user`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Me {
    pub id: i64,
    pub first_name: String,
    pub last_name: String,
    pub username: Option<String>,
    pub phone: String,
    pub color: u8,
    /// The small profile photo: TDLib's file id, and its path once it is downloaded.
    pub photo: Option<(i64, Option<String>)>,
}

/// Reads TDLib's `user`; none when it isn't one.
pub fn parse_user(user: &Value) -> Option<Me> {
    if user["@type"] != "user" {
        return None;
    }
    let id = user["id"].as_i64()?;
    let username = user["usernames"]["active_usernames"]
        .as_array()
        .and_then(|names| names.first())
        .and_then(Value::as_str)
        .map(str::to_string);
    // Telegram's built-in name colors are 0–6, as the avatar colors; the rest are the
    // Premium ones — the classic rule then.
    let accent = user["accent_color_id"].as_i64().unwrap_or(-1);
    let color = if (0..7).contains(&accent) {
        accent as u8
    } else {
        id.rem_euclid(7) as u8
    };
    let small = &user["profile_photo"]["small"];
    let photo = small["id"].as_i64().map(|file| (file, downloaded(small)));
    Some(Me {
        id,
        first_name: text_of(user, "first_name"),
        last_name: text_of(user, "last_name"),
        username,
        phone: text_of(user, "phone_number"),
        color,
        photo,
    })
}

/// The path of TDLib's `file` once it is downloaded.
pub fn downloaded(file: &Value) -> Option<String> {
    let local = &file["local"];
    let path = local["path"].as_str().unwrap_or_default();
    (local["is_downloading_completed"] == true && !path.is_empty()).then(|| path.to_string())
}

/// "Egor Ageev"; the number when there is no name.
pub fn full_name(first: &str, last: &str, phone: &str) -> String {
    let name = format!("{first} {last}").trim().to_string();
    if name.is_empty() {
        mask_phone(phone)
    } else {
        name
    }
}

/// The initials of an avatar: the name's first letter, as Telegram's small avatars show it.
pub fn initials(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

// --- Phone numbers ---

/// The two-digit country codes (the rest are one digit — 1, 7 — or three).
const TWO_DIGIT_CODES: &[&str] = &[
    "20", "27", "30", "31", "32", "33", "34", "36", "39", "40", "41", "43", "44", "45", "46", "47",
    "48", "49", "51", "52", "53", "54", "55", "56", "57", "58", "60", "61", "62", "63", "64", "65",
    "66", "81", "82", "84", "86", "90", "91", "92", "93", "94", "95", "98",
];

/// A phone number with only its country code and last two digits: "+7 ••• •••-••-12".
pub fn mask_phone(phone: &str) -> String {
    let digits: String = phone.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 5 {
        return format!("+{}", "•".repeat(digits.len()));
    }
    let code = match digits.as_bytes()[0] {
        b'1' | b'7' => 1,
        _ if TWO_DIGIT_CODES.iter().any(|two| digits.starts_with(two)) => 2,
        _ => 3,
    };
    let (code, national) = digits.split_at(code);
    let tail = &national[national.len() - 2..];
    let hidden = national.len() - 2;
    let masked = if hidden >= 5 {
        // The last seven digits as "•••-••-12", the rest before them.
        let head = hidden - 5;
        let lead = if head > 0 {
            format!("{} ", "•".repeat(head))
        } else {
            String::new()
        };
        format!("{lead}•••-••-{tail}")
    } else {
        format!("{}{tail}", "•".repeat(hidden))
    };
    format!("+{code} {masked}")
}

/// Looks like a phone number: seven digits or more, nothing but digits, spaces, `+ - ( )`.
pub fn looks_like_phone(text: &str) -> bool {
    text.chars().filter(char::is_ascii_digit).count() >= 7
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || " +-().".contains(c))
}

// --- The helper's output ---

/// The helper's output split into lines: `process-output` comes in chunks that may end in the
/// middle of a line, which waits for its end.
#[derive(Debug, Default)]
pub struct Lines {
    partial: Vec<u8>,
}

impl Lines {
    /// The complete lines of `bytes`, the partial one before them first; empty lines are skipped.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        let mut rest = bytes;
        while let Some(end) = rest.iter().position(|&byte| byte == b'\n') {
            self.partial.extend_from_slice(&rest[..end]);
            let line = String::from_utf8_lossy(&self.partial);
            let line = line.trim_end_matches('\r');
            if !line.is_empty() {
                lines.push(line.to_string());
            }
            self.partial.clear();
            rest = &rest[end + 1..];
        }
        self.partial.extend_from_slice(rest);
        lines
    }

    /// Forgets a partial line: the program was restarted.
    pub fn clear(&mut self) {
        self.partial.clear();
    }
}

/// The delay before the helper's restart after `failures` exits in a row, in seconds.
pub fn restart_delay(failures: u32) -> u64 {
    const DELAYS: [u64; 6] = [1, 2, 5, 10, 30, 60];
    DELAYS[(failures.max(1) as usize - 1).min(DELAYS.len() - 1)]
}

/// TDLib's updates FluxGram uses (`fluxgram.keep`): the helper holds the rest back, so they never
/// wake the plugin — TDLib has about 170 kinds, and new versions add more. The sign-in's states,
/// the users' names and photos (`updateUser`), the files' downloads (`updateFile`), the
/// connection (`updateConnectionState`, to the log), and what [`crate::store::Store`] keeps of the
/// chats. The list (`chats::KEPT`) and the feed (`feed::KEPT`) add theirs; the service sends the
/// union. Updates of chats that aren't in a list or open don't pass at all (`fluxgram.chats`).
pub const KEPT: &[&str] = &[
    "updateAuthorizationState",
    "updateUser",
    "updateFile",
    "updateConnectionState",
    // The store (part 3).
    "updateNewChat",
    "updateChatTitle",
    "updateChatPhoto",
    "updateChatAccentColors",
    "updateChatPermissions",
    "updateChatHasProtectedContent",
    "updateChatLastMessage",
    "updateChatPosition",
    "updateChatReadInbox",
    "updateChatReadOutbox",
    "updateChatUnreadMentionCount",
    "updateChatNotificationSettings",
    "updateChatDraftMessage",
    "updateScopeNotificationSettings",
    "updateBasicGroup",
    "updateSupergroup",
    "updateUserStatus",
    // A forum's topics renamed, recolored, closed (part 5).
    "updateForumTopicInfo",
    // A bot's keyboard over the field, a bot stopped (part 5).
    "updateChatReplyMarkup",
    "updateChatBlockList",
    // The reactions a chat allows, and Telegram's when it allows all (part 5).
    "updateChatAvailableReactions",
    "updateActiveEmojiReactions",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_phones() {
        assert_eq!(mask_phone("79001234512"), "+7 ••• •••-••-12");
        assert_eq!(mask_phone("+7 900 123-45-12"), "+7 ••• •••-••-12");
        assert_eq!(mask_phone("380501234567"), "+380 •• •••-••-67");
        assert_eq!(mask_phone("4915112345678"), "+49 •••• •••-••-78");
        assert_eq!(mask_phone("9996621471"), "+999 •••-••-71");
        assert_eq!(mask_phone("12345"), "+1 ••45");
        assert_eq!(mask_phone("123"), "+•••");
        assert!(!mask_phone("79001234512").contains("900"));
    }

    #[test]
    fn checks_phone_input() {
        assert!(looks_like_phone("+7 (900) 123-45-12"));
        assert!(!looks_like_phone("123"));
        assert!(!looks_like_phone("call me 79001234512"));
    }

    #[test]
    fn splits_lines() {
        let mut lines = Lines::default();
        assert_eq!(lines.push(b"{\"a\":1}\n{\"b\""), ["{\"a\":1}"]);
        assert!(lines.push(b":2").is_empty());
        assert_eq!(
            lines.push(b"}\r\n\n{\"c\":3}\n"),
            ["{\"b\":2}", "{\"c\":3}"]
        );
        assert!(lines.push(b"").is_empty());
        // A multi-byte character split between two chunks stays whole.
        let text = "{\"name\":\"Егор\"}\n".as_bytes();
        assert!(lines.push(&text[..10]).is_empty());
        assert_eq!(lines.push(&text[10..]), ["{\"name\":\"Егор\"}"]);
        lines.push(b"half");
        lines.clear();
        assert_eq!(lines.push(b"new\n"), ["new"]);
    }

    #[test]
    fn reads_auth_states() {
        let qr = json!({"@type": "authorizationStateWaitOtherDeviceConfirmation", "link": "tg://login?token=x"});
        assert_eq!(
            parse_auth(&qr, 0),
            Auth::WaitQr {
                link: "tg://login?token=x".into()
            }
        );
        let code = json!({"@type": "authorizationStateWaitCode", "code_info": {
            "@type": "authenticationCodeInfo", "phone_number": "9996621471",
            "type": {"@type": "authenticationCodeTypeSms", "length": 5},
            "next_type": {"@type": "authenticationCodeTypeCall", "length": 0}, "timeout": 90}});
        assert_eq!(
            parse_auth(&code, 1000),
            Auth::WaitCode {
                phone: "9996621471".into(),
                sent: CodeSent::Sms,
                next: Some(CodeSent::Call),
                resend_at: 1090,
            }
        );
        let app = json!({"@type": "authorizationStateWaitCode", "code_info": {
            "phone_number": "1", "type": {"@type": "authenticationCodeTypeTelegramMessage"}, "next_type": null}});
        assert!(matches!(
            parse_auth(&app, 1000),
            Auth::WaitCode {
                sent: CodeSent::App,
                next: None,
                resend_at: 1000,
                ..
            }
        ));
        let password = json!({"@type": "authorizationStateWaitPassword", "password_hint": "cat",
            "has_recovery_email_address": false});
        assert_eq!(
            parse_auth(&password, 0),
            Auth::WaitPassword { hint: "cat".into() }
        );
        assert_eq!(
            parse_auth(&json!({"@type": "authorizationStateWaitRegistration"}), 0),
            Auth::WaitRegistration
        );
        assert_eq!(
            parse_auth(&json!({"@type": "authorizationStateSomethingNew"}), 0),
            Auth::Other
        );
        assert!(Auth::WaitPhone.is_signed_out() && !Auth::Ready.is_signed_out());
        assert!(Auth::Ready.after_phone() && !Auth::WaitPhone.after_phone());
        assert!(
            !Auth::WaitQr {
                link: String::new()
            }
            .takes_phone()
        );
    }

    #[test]
    fn picks_screens_and_requests() {
        let qr = Auth::WaitQr { link: "l".into() };
        let waiting = Some(Screen::Qr {
            link: String::new(),
        });
        assert_eq!(screen(&qr, false), Some(Screen::Qr { link: "l".into() }));
        assert_eq!(screen(&qr, true), Some(Screen::Phone));
        // No screen of waiting: the QR code's placeholder, or the phone screen at once.
        assert_eq!(screen(&Auth::WaitParameters, false), waiting);
        assert_eq!(screen(&Auth::WaitPhone, false), waiting);
        assert_eq!(screen(&Auth::WaitParameters, true), Some(Screen::Phone));
        assert_eq!(screen(&Auth::WaitPhone, true), Some(Screen::Phone));
        // Between two screens the one on show stays.
        assert_eq!(screen(&Auth::Ready, false), None);
        assert_eq!(screen(&Auth::Closed, true), None);
        assert_eq!(
            screen(&Auth::WaitRegistration, false),
            Some(Screen::NoAccount)
        );
        assert_eq!(
            screen(&Auth::WaitPassword { hint: "h".into() }, false),
            Some(Screen::Password { hint: "h".into() })
        );
        let code = Auth::WaitCode {
            phone: "1".into(),
            sent: CodeSent::App,
            next: Some(CodeSent::Sms),
            resend_at: 60,
        };
        assert_eq!(
            screen(&code, true),
            Some(Screen::Code {
                phone: "1".into(),
                sent: CodeSent::App,
                resend: Some(CodeSent::Sms),
                resend_at: 60,
            })
        );
        assert_eq!(next_step(&qr, true, None), Next::Renew);
        assert_eq!(
            next_step(&Auth::WaitParameters, false, None),
            Next::Parameters
        );
        assert_eq!(next_step(&Auth::WaitPhone, false, None), Next::RequestQr);
        assert_eq!(next_step(&Auth::WaitPhone, true, None), Next::Nothing);
        assert_eq!(
            next_step(&Auth::WaitPhone, true, Some("79001234512")),
            Next::SendPhone("79001234512".into())
        );
        assert_eq!(next_step(&Auth::Ready, true, None), Next::GetMe);
        assert_eq!(next_step(&qr, false, None), Next::Nothing);
    }

    #[test]
    fn maps_errors() {
        assert_eq!(problem(400, "PHONE_CODE_INVALID"), Problem::WrongCode);
        assert_eq!(
            problem(400, "PASSWORD_HASH_INVALID"),
            Problem::WrongPassword
        );
        assert_eq!(problem(400, "PHONE_NUMBER_INVALID"), Problem::InvalidPhone);
        assert_eq!(problem(400, "PHONE_CODE_EXPIRED"), Problem::CodeExpired);
        assert_eq!(problem(400, "API_ID_INVALID"), Problem::KeyRefused);
        assert_eq!(problem(420, "FLOOD_WAIT_3600"), Problem::TryLater(60));
        assert_eq!(
            problem(429, "Too Many Requests: retry after 61"),
            Problem::TryLater(2)
        );
        assert_eq!(problem(429, "FLOOD_WAIT_5"), Problem::TryLater(1));
        assert_eq!(problem(429, "Too Many Requests"), Problem::TryLater(1));
        assert_eq!(
            problem(500, "Something odd"),
            Problem::Failed("Something odd".into())
        );
        assert_eq!(problem(400, "SEND_CODE_UNAVAILABLE"), Problem::NoOtherWay);
        assert_eq!(
            problem(400, "Authentication code can't be resend"),
            Problem::NoOtherWay
        );
        assert_eq!(Problem::TryLater(1).text().1, None);
        assert_eq!(Problem::TryLater(5).text().1.as_deref(), Some("5"));
    }

    #[test]
    fn reads_users() {
        let user = json!({"@type": "user", "id": 777000123, "first_name": "Egor", "last_name": "Ageev",
            "usernames": {"active_usernames": ["egor"]}, "phone_number": "79001234512",
            "accent_color_id": 4, "profile_photo": {"small": {"id": 12,
            "local": {"path": "/data/files/p.jpg", "is_downloading_completed": true}}}});
        let me = parse_user(&user).unwrap();
        assert_eq!(me.color, 4);
        assert_eq!(me.username.as_deref(), Some("egor"));
        assert_eq!(me.photo, Some((12, Some("/data/files/p.jpg".into()))));
        let plain = json!({"@type": "user", "id": 15, "first_name": "", "last_name": "",
            "phone_number": "79001234512", "accent_color_id": 9});
        let plain = parse_user(&plain).unwrap();
        assert_eq!((plain.color, plain.photo), (1, None));
        assert_eq!(full_name("", "", "79001234512"), "+7 ••• •••-••-12");
        assert_eq!(full_name("Egor", "", ""), "Egor");
        assert_eq!(initials("егор"), "Е");
        assert!(parse_user(&json!({"@type": "error"})).is_none());
    }

    #[test]
    fn messages_round_trip() {
        let snapshot = Snapshot {
            accounts: vec![],
            sign_in: Some(SignIn {
                screen: Screen::Code {
                    phone: "1".into(),
                    sent: CodeSent::App,
                    resend: Some(CodeSent::Sms),
                    resend_at: 1_800_000_000,
                },
                busy: false,
                resending: true,
                problem: Some(Problem::TryLater(3)),
            }),
        };
        let text = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(serde_json::from_str::<Snapshot>(&text).unwrap(), snapshot);
        let request = Request::Submit {
            text: "12345".into(),
        };
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), request);
        assert_eq!(
            serde_json::to_string(&Request::UseQr).unwrap(),
            r#"{"do":"use-qr"}"#
        );
    }

    #[test]
    fn logs_the_code_without_the_number() {
        let state = json!({"@type": "authorizationStateWaitCode", "code_info": {
            "@type": "authenticationCodeInfo", "phone_number": "79001234512",
            "type": {"@type": "authenticationCodeTypeTelegramMessage", "length": 5},
            "next_type": {"@type": "authenticationCodeTypeSms", "length": 5}, "timeout": 58}});
        let line = code_log(&state).unwrap();
        assert_eq!(
            line,
            "Sign-in: the code was sent as authenticationCodeTypeTelegramMessage (5 digits); \
             next: authenticationCodeTypeSms (5 digits), in 58 s"
        );
        assert!(!line.contains("4512"));
        let last = json!({"@type": "authorizationStateWaitCode", "code_info": {
            "phone_number": "1", "type": {"@type": "authenticationCodeTypeCall", "length": 0},
            "next_type": null, "timeout": 0}});
        assert_eq!(
            code_log(&last).unwrap(),
            "Sign-in: the code was sent as authenticationCodeTypeCall; next: none, in 0 s"
        );
        assert_eq!(code_log(&json!({"@type": "authorizationStateReady"})), None);
    }

    #[test]
    fn tells_accounts_apart() {
        let account = |key: &str, name: &str, phone: &str| AccountView {
            key: key.into(),
            name: name.into(),
            phone: phone.into(),
            label: None,
            color: 0,
            photo: None,
            signing_out: false,
        };
        let labels = account_labels(&[
            account("a", "Egor Ageev", "+7 ••• •••-••-12"),
            account("b", "Egor Petrov", "+7 ••• •••-••-34"),
            account("c", "Test", "+7 ••• •••-••-56"),
            account("d", "Test", "+7 ••• •••-••-78"),
            account("e", "Anna", "+7 ••• •••-••-90"),
        ]);
        assert_eq!(labels["a"], "Egor Ageev");
        assert_eq!(labels["b"], "Egor Petrov");
        assert_eq!(labels["c"], "Test · +7 ••• •••-••-56");
        assert_eq!(labels["e"], "Anna");
    }

    #[test]
    fn counts_down() {
        assert_eq!(countdown(58), "0:58");
        assert_eq!(countdown(90), "1:30");
        assert_eq!(countdown(720), "12:00");
        assert_eq!(countdown(0), "0:00");
        assert!(unix_now() > 1_700_000_000);
    }

    #[test]
    fn counts_in_russian() {
        let forms: Vec<&str> = [1, 2, 5, 11, 12, 21, 22, 25, 101, 111, 0]
            .into_iter()
            .map(russian_form)
            .collect();
        assert_eq!(
            forms,
            [
                "one", "few", "many", "many", "many", "one", "few", "many", "one", "many", "many"
            ]
        );
        assert_eq!(English.count(1, "{0} member", "{0} members"), "1 member");
        assert_eq!(English.count(5, "{0} member", "{0} members"), "5 members");
    }

    #[test]
    fn backs_off() {
        assert_eq!(restart_delay(0), 1);
        assert_eq!(restart_delay(1), 1);
        assert_eq!(restart_delay(3), 5);
        assert_eq!(restart_delay(50), 60);
        // Every user's "online" would wake the plugin: it passes only for the chats in a list or
        // open (`fluxgram.chats` takes a person's id as their chat's).
        assert!(KEPT.contains(&"updateUserStatus"));
        assert!(!KEPT.contains(&"updateOption"));
        assert!(KEPT.contains(&"updateAuthorizationState"));
        assert!(KEPT.contains(&"updateUser"));
        assert!(KEPT.contains(&"updateFile"));
        assert!(KEPT.contains(&"updateConnectionState"));
    }

    #[test]
    fn builds_parameters() {
        let request = Parameters {
            folder: "/data/accounts/a1",
            database_key: "a2V5",
            language: "ru",
            test_dc: true,
            api_id: 1,
            api_hash: "h",
        }
        .request();
        assert_eq!(request["database_directory"], "/data/accounts/a1/db");
        assert_eq!(request["files_directory"], "/data/accounts/a1/files");
        assert_eq!(request["use_test_dc"], true);
    }
}
