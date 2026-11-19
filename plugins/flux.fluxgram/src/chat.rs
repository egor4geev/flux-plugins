//! A chat's tab (part 3, mockup A): the window `chat` (`tabs = true`), a tab `chat/<account>:<chat>`
//! per chat — the header (the picture, the title, "online" / "5 members" / "Petya is typing…")
//! over the conversation and its field. The service sends the header ([`ChatHeader`]) and the
//! feed's changes, which go to the conversation as they are; this module keeps the feed's
//! messages it was given (for the quote over the field, the text to edit, the user's last
//! message) and passes on what the user does: a page more, the messages seen, the field sent, an
//! edit, a delete (asked first), a download, a quote's message, the draft.
//!
//! Over the conversation, next to the header: 🔍 (and ⌘F) opens the chat's search — a bar under
//! the header: the field, "3 of 17", ↑ ↓ (an older and a newer match; ↵ and ⌘G go older, ⇧⌘G
//! newer) and ×. The query goes to the service once typing stops ([`SearchAsk`]), the matches
//! come newest first ([`SearchTold`]); the feed jumps to each one (`reveal`, or `JumpTo` when it
//! isn't loaded) and the query's words are marked in the messages while the bar is open — here,
//! in this window only: the service's messages stay as they are. ⋯ opens the chat's menu: Search,
//! Profile, Media, Mute, Add to Folder… (the list's picker of FluxGram's folders; not for a hidden
//! chat), Hide Chat (or Unhide), Remove from FluxGram. With two accounts or more, the header's line
//! ends with the chat's account's first name.
//!
//! Part 5: the bar of the pinned message over the feed (the service's, [`crate::pins`]) — a click
//! reveals the message and moves the bar on; its × unpins (asked first) or hides the bar. A
//! message's menu: Pin (asked first, with "Notify all members" in a group, "Also pin for <name>"
//! in a one-to-one chat), Unpin (asked), Copy Link, Add to Favorites (a sticker), Save GIF. ⋯ also
//! has Mark as Read / Unread, and Mute opens a choice: for an hour, 8 hours, 2 days, for good.
//! The field (part 5): "@" and "/" ask the service for suggestions (the members to mention, the
//! bots' commands — [`crate::suggest`]), and Send's menu sends without sound (but in Saved
//! Messages). A channel the user can't post in has a button in place of the field: Mute or
//! Unmute, Join where the user isn't in it (a group's too: one a link opened). Its advertisement
//! ([`crate::ads`]): its button or its picture follows the sponsor's link as a button's and tells
//! Telegram; its menu has About the Ad and Report (the options Telegram offers, as a choice).
//!
//! Selecting and forwarding (part 5): a message's menu has Forward where Telegram lets it go
//! (not a protected chat's), and Flux's Select; the selection's bar has Forward and Delete. Forward
//! asks the list's picker for a chat of the same account ([`TabGo::Forward`]); that chat's tab
//! gets the bar over its field (`forwarding`), kept here per chat until ↵ sends the field's text
//! and then the messages, or × drops them — the ones Telegram won't forward are told in a note.
//! Delete asks once for all ("Delete 3 messages?" with "Also delete for Anna" / "Delete for
//! everyone" when each of them can go for everyone).
//!
//! Polls and reactions (part 5): a vote in a poll goes to the service (`setPollAnswer`) and the
//! poll comes back answered; a message's menu has Retract Vote (not in a quiz) and Stop Poll (its
//! author, a channel's admin; asked first). The chat's reactions (the header's, as the chat allows
//! them) are the conversation's: the ☺ row and the chips under a message put the user's own or
//! take it back — at the user's limit the oldest of theirs goes first
//! ([`crate::convert::toggle_reaction`]).
//!
//! A bot's chat (part 5, [`crate::bots`]): the buttons under its messages do what they say — the
//! bot answers a callback (a notification, or a calm dialog for an alert), a link is followed as a
//! button's ([`Tabs::follow`]), a query goes into the field, a text is
//! copied, a person's profile opens; its keyboard over the field sends a button's text; a bot
//! asking for a reply puts its message's quote over the field. The field's Menu lists the bot's
//! commands (a choice sends one at once). A bot without messages has «Start» in place of the
//! field, a stopped one «Unblock»; ⋯ has Stop Bot (asked first) or Restart Bot.
//!
//! Links (part 5, [`crate::links`]): the conversation owns its links — every one comes here and is
//! followed ([`Tabs::follow`]): Telegram's through the service (a chat's tab, a profile, a message
//! revealed, an invite or a sticker set asked about first), a mention, a hashtag, a command as they
//! say, the web's asked first as Settings → FluxGram → Links say. ⋯ of a chat a link opened that
//! isn't in the list offers Add to FluxGram in place of Remove.
//!
//! Claude (part 6, [`crate::claude`]): ⋯ of a chat in the list (not a hidden one) has Available
//! to Claude, checked while Claude may read the chat; a choice turns it on or off in the service.
//! There too — a message's Reply with Claude (its menu, its hover bar) and the field's button
//! beside Send: the chat goes on for Claude, and the service makes the new session's attachment
//! (the message, or the field's unfinished text), which the window opens. Edit on Claude's card
//! (`send_message`) brings the chat's tab here with the text in the field and the quote over it
//! ([`Tabs::claude_draft`]).
//!
//! The field has its panel (part 4): Flux's emoji, the account's stickers and GIFs — the service
//! fills the tabs ([`PanelAsk`]) and sends what is chosen, answering the quote over the field. A
//! voice or round message played for the first time is told to the service, which marks it
//! listened to in Telegram.
//!
//! A tab Flux brings back with the project comes as `tool-window-shown` of a tab this instance
//! doesn't know: it is filled again from its key. A tab whose account is gone stays, and says so.
//!
//! A scenario reaches a tab through `ui:<plugin>/chat/<key>/<element>`: `open` opens it, `reply/<id>`,
//! `edit/<id>`, `delete/<id>`, `forward/<id>` are a message's actions, `ui-submit:…/send=<text>` sends a text and
//! `…/draft=<text>` types one; `find` opens the search, `ui-submit:…/find-field=<text>` looks for
//! a text at once, `find-older` / `find-newer` / `find-close` are the bar's buttons, `more` the
//! menu and `menu/<item>` its items (`menu/hide`).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use flux_plugin_api::UiEvent;
use flux_plugin_api::conversation::{
    self as conv, Change, Composed, Message, MessageQuote, PanelTab, Part, VideoKind,
};
use flux_plugin_api::dialog::{Ask, Form, Values};
use flux_plugin_api::host::ui::{self, ButtonSpec, ElementKind, View};
use flux_plugin_api::view::*;
use flux_plugin_api::{notify, picker, service, system, timers, tr, trf};

use crate::ads::{AdAsk, AdInfo};
use crate::claude::{Card, ClaudeAsk};
use crate::convert::{
    ABOUT_AD, COPY_LINK, DELETE, DELETE_FOR_ME, EDIT, FAVE_STICKER, FORWARD, PIN, REPLY,
    REPLY_CLAUDE, REPORT_AD, RETRACT_VOTE, SAVE_GIF, STOP_POLL, UNFAVE_STICKER, UNPIN, highlighted,
    search_words,
};
use crate::feed::{SearchAsk, SearchTold};
use crate::links::{LinkAsk, LinkTold, Shown, Target, Web};
use crate::model::{AccountView, ChatHeader, ChatKey, Request, Words, account_labels, initials};
use crate::panel::PanelAsk;

/// The window of the chats' tabs (`tabs = true`).
pub const CHAT: &str = "chat";
/// The conversation element of a chat's view.
pub const FEED: &str = "feed";
/// The header element of a chat's view: a click opens the profile.
pub const HEADER: &str = "header";
/// The header's buttons: the search (🔍) and the chat's menu (⋯; the folders' picker opens under
/// it too).
const FIND: &str = "find";
pub const MORE: &str = "more";
/// The search bar: the field, "3 of 17", an older and a newer match, ×.
const FIND_FIELD: &str = "find-field";
const FIND_COUNT: &str = "find-count";
const FIND_OLDER: &str = "find-older";
const FIND_NEWER: &str = "find-newer";
const FIND_CLOSE: &str = "find-close";
/// The search asks once typing stops this long.
const FIND_AFTER: Duration = Duration::from_millis(300);
/// Send's menu (part 5): Send Without Sound.
const SILENT: &str = "silent";
/// The button in place of the field in a channel (part 5): Join, Mute, Unmute.
const JOIN: &str = "join";
/// A bot's chat (part 5): Start and Unblock in place of the field, the field's Menu.
const START_BOT: &str = "start-bot";
const UNBLOCK_BOT: &str = "unblock-bot";
const BOT_MENU: &str = "bot-menu";
const MUTE_CHANNEL: &str = "mute-channel";
const UNMUTE_CHANNEL: &str = "unmute-channel";

/// What a chat's tab asks the window for (part 3+): the profile dialog's parts.
pub enum TabGo {
    /// The chat's profile (the header's click, ⋯ → Profile).
    Profile(ChatKey),
    /// The chat's media (⋯ → Media).
    Media(ChatKey),
    /// Mute or unmute the chat (⋯ → Mute).
    Mute(ChatKey, bool),
    /// Mute the chat for this many seconds (⋯ → Mute → a choice; part 5).
    MuteFor(ChatKey, i64),
    /// Hide the chat (⋯ → Hide Chat): the list knows how (the phrase).
    Hide(ChatKey),
    /// Show a hidden chat in the list again (⋯ → Unhide).
    Unhide(ChatKey),
    /// The folders' picker for the chat (⋯ → Add to Folder…): the list's.
    Folder(ChatKey),
    /// The picker of a chat to forward messages to (part 5): the list's.
    Forward(Forward),
    /// A link opened the chat's tab (part 5): the list marks it.
    Opened(ChatKey),
}

/// Messages to forward (part 5): from a chat, in the feed's order, with the bar's quote over the
/// target's field ("Forwarding 3 messages", "Anna, Boris: Hi…").
#[derive(Debug, Clone, PartialEq)]
pub struct Forward {
    pub from: ChatKey,
    pub messages: Vec<String>,
    pub quote: MessageQuote,
}

/// The selection's Delete (part 5); Forward is the message action's id.
const DELETE_SELECTED: &str = "delete-selected";

/// The window `chat/<key>` is a chat's tab.
pub fn is_tab(window: &str) -> bool {
    window
        .strip_prefix(CHAT)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// The chat of a tab's window name.
pub fn key_of(window: &str) -> Option<ChatKey> {
    ChatKey::parse(window.strip_prefix(CHAT)?.strip_prefix('/')?)
}

/// A chat's tab in this window.
struct Tab {
    /// `chat/<key>`.
    window: String,
    /// The title on the tab: the list's, then the header's.
    title: String,
    header: Option<ChatHeader>,
    /// The feed's messages, as the conversation has them.
    messages: HashMap<String, Message>,
    /// The account was signed out: the tab says so.
    gone: bool,
    in_sight: bool,
    /// A hidden chat's tab: it doesn't come back after a restart, and goes when the phrase leaves
    /// the list's search.
    hidden: bool,
    /// The search bar, while it is open.
    find: Option<Find>,
    /// The message the quote over the field answers: a sticker or a GIF of the panel answers it
    /// too.
    reply: Option<String>,
    /// The view set last: an unchanged one isn't set again.
    view: Option<View>,
    /// The chat's advertisement (part 5): what its clicks and About the Ad need.
    ad: Option<AdInfo>,
}

/// A chat's search in its tab: the bar's field, the matches, the words marked in the messages.
#[derive(Default)]
struct Find {
    /// The field's text.
    text: String,
    /// The query asked (the field's text once typing stopped) and its request; empty — none.
    query: String,
    request: u64,
    /// The query's words, marked in the messages.
    words: Vec<Vec<char>>,
    /// The matches so far, newest first; how many Telegram counts (-1: it doesn't know); where
    /// the next page starts (0: no more).
    found: Vec<String>,
    total: i64,
    next: i64,
    /// The answer to the query came; Telegram couldn't search.
    answered: bool,
    failed: bool,
    /// The match on show: an index into `found`.
    current: Option<usize>,
    /// The next page is on its way; `step` — the user went past the last match: the page's
    /// first one is next.
    paging: bool,
    step: bool,
    /// The typing's timer: the query goes when it fires.
    timer: Option<u64>,
    /// The messages shown marked: they get their unmarked selves back when the words change or the
    /// bar closes.
    lit: HashSet<String>,
}

impl Find {
    /// "3 of 17", "No results", "…" while the answer is on its way; empty — nothing asked.
    fn count(&self) -> String {
        if self.query.is_empty() {
            return String::new();
        }
        if !self.answered {
            return "…".to_string();
        }
        if self.failed {
            return tr("Couldn't search");
        }
        if self.found.is_empty() {
            return tr("No results");
        }
        let total = if self.total >= self.found.len() as i64 {
            self.total.to_string()
        } else if self.next != 0 {
            format!("{}+", self.found.len())
        } else {
            self.found.len().to_string()
        };
        let current = self.current.map_or(0, |index| index + 1);
        trf("{0} of {1}", &[&current, &total])
    }
}

pub struct Tabs {
    tabs: HashMap<ChatKey, Tab>,
    /// What the tabs ask the window for, taken after each event ([`Tabs::take_go`]).
    go: Vec<TabGo>,
    /// The accounts signed in, once the service told them.
    accounts: Option<Vec<String>>,
    /// What tells the accounts apart (their first names), by key, with two accounts or more: the
    /// header says whose chat it is. Empty with one.
    names: HashMap<String, String>,
    /// The Delete question on show: its id, the chat, the message.
    asked: Option<(u64, ChatKey, String)>,
    /// The ⋯ menu on show: the picker's id, the chat; the Mute choice's (part 5).
    menu: Option<(u64, ChatKey)>,
    mute_menu: Option<(u64, ChatKey)>,
    /// The Pin and Unpin questions on show (part 5): the id, the chat, the message.
    pin_asked: Option<(u64, ChatKey, String)>,
    unpin_asked: Option<(u64, ChatKey, String)>,
    /// Stop Poll asked (part 5): the id, the chat, the message.
    stop_poll_asked: Option<(u64, ChatKey, String)>,
    /// A page asked about before it opens in the browser (part 5): a link FluxGram doesn't
    /// open, a bot's game — the question, the address; the report's choice on show: the picker,
    /// the chat, the ad.
    link_asked: Option<(u64, String)>,
    report_picker: Option<(u64, ChatKey, String)>,
    /// A bot's menu of commands on show (part 5): the picker, the chat; Stop Bot asked.
    commands_picker: Option<(u64, ChatKey)>,
    stop_asked: Option<(u64, ChatKey)>,
    /// Messages to forward into a chat, by the chat (part 5): the bar over its field until they
    /// are sent or cancelled, wherever the user goes meanwhile.
    forwards: HashMap<ChatKey, Forward>,
    /// The Delete question of selected messages (part 5): its id, the chat, the messages.
    delete_many: Option<(u64, ChatKey, Vec<String>)>,
    /// Links (part 5, [`crate::links`]): a web page asked about (Open, Copy), an invite (Join),
    /// a sticker set (Add) — the question, the chat, the link or the set.
    web_asked: Option<(u64, String)>,
    invite_asked: Option<(u64, ChatKey, String)>,
    stickers_asked: Option<(u64, ChatKey, String)>,
    /// The chats in each account's list: ⋯ of another chat's tab (opened by a link) offers Add
    /// to FluxGram.
    listed: HashMap<String, HashSet<i64>>,
    /// The chats available to Claude in each account's list (part 6): ⋯'s item is checked.
    claude: HashMap<String, HashSet<i64>>,
    /// A message a link reveals in a chat whose feed hasn't come yet: asked for once it has.
    jumps: HashMap<ChatKey, String>,
    /// Claude's text Edit put into a tab whose feed hasn't come yet (part 6): it wins over the
    /// saved draft the feed's first fill brings.
    claude_drafts: HashMap<ChatKey, String>,
    /// The last search request's id.
    requests: u64,
}

impl Tabs {
    pub fn new() -> Self {
        Tabs {
            tabs: HashMap::new(),
            go: Vec::new(),
            accounts: None,
            names: HashMap::new(),
            asked: None,
            menu: None,
            mute_menu: None,
            pin_asked: None,
            unpin_asked: None,
            stop_poll_asked: None,
            link_asked: None,
            report_picker: None,
            commands_picker: None,
            stop_asked: None,
            forwards: HashMap::new(),
            delete_many: None,
            web_asked: None,
            invite_asked: None,
            stickers_asked: None,
            listed: HashMap::new(),
            claude: HashMap::new(),
            jumps: HashMap::new(),
            claude_drafts: HashMap::new(),
            requests: 0,
        }
    }

    /// A page of a chat's search's matches: the first jumps to the newest match; a page more, to
    /// its first when the user went past the last one.
    pub fn search_told(&mut self, told: SearchTold) {
        let SearchTold::Found {
            key,
            request,
            from,
            ids,
            total,
            next,
            failed,
        } = told;
        let Some(find) = self
            .tabs
            .get_mut(&key)
            .and_then(|tab| tab.find.as_mut())
            .filter(|find| find.request == request)
        else {
            return;
        };
        let mut jump = false;
        if from == 0 {
            find.found = ids;
            find.current = (!find.found.is_empty()).then_some(0);
            jump = find.current.is_some();
        } else {
            let before = find.found.len();
            let more: Vec<String> = ids
                .into_iter()
                .filter(|id| !find.found.contains(id))
                .collect();
            find.found.extend(more);
            find.paging = false;
            if std::mem::take(&mut find.step) && find.found.len() > before {
                find.current = Some(before);
                jump = true;
            }
        }
        find.total = total;
        find.next = next;
        find.answered = true;
        find.failed = failed;
        let current = find
            .current
            .and_then(|index| find.found.get(index).cloned())
            .filter(|_| jump);
        if let Some(message) = current {
            self.reveal(&key, &message);
        }
        self.render(&key);
    }

    /// The tab's menu (⋯), or the picker of another: whose choice it is.
    pub fn owns_picker(&self, picker: u64) -> bool {
        self.menu.as_ref().is_some_and(|(id, _)| *id == picker)
            || self.mute_menu.as_ref().is_some_and(|(id, _)| *id == picker)
            || self
                .report_picker
                .as_ref()
                .is_some_and(|(id, _, _)| *id == picker)
            || self
                .commands_picker
                .as_ref()
                .is_some_and(|(id, _)| *id == picker)
    }

    /// The ⋯ menu (or Mute's choice) closed: its choice, or none.
    pub fn picker_chosen(&mut self, picker: u64, choice: Option<String>) {
        if let Some((_, key, message)) = self.report_picker.take_if(|(id, _, _)| *id == picker) {
            if let Some(option) = choice {
                ask(&Request::Ad(AdAsk::Report {
                    key,
                    message,
                    option,
                }));
            }
            return;
        }
        // A bot's command from its menu (part 5): sent at once, as Telegram's menu does.
        if let Some((_, key)) = self.commands_picker.take_if(|(id, _)| *id == picker) {
            if let Some(command) = choice {
                ask(&Request::BotText { key, text: command });
            }
            return;
        }
        if let Some((_, key)) = self.mute_menu.take_if(|(id, _)| *id == picker) {
            if let Some(seconds) = choice.and_then(|choice| choice.parse::<i64>().ok()) {
                self.go.push(TabGo::MuteFor(key, seconds));
            }
            return;
        }
        let Some((_, key)) = self.menu.take_if(|(id, _)| *id == picker) else {
            return;
        };
        if let Some(choice) = choice {
            self.choose(key, &choice);
        }
    }

    /// Mute's choice under ⋯ (part 5): for an hour, 8 hours, 2 days, for good.
    fn open_mute_menu(&mut self, key: &ChatKey) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let items: Vec<_> = mute_choices()
            .into_iter()
            .map(|(seconds, label)| picker::item(&seconds.to_string(), &tr(label)))
            .collect();
        let id = picker::Picker::menu()
            .items(items)
            .under(&tab.window, MORE)
            .open();
        self.mute_menu = Some((id, key.clone()));
    }

    /// A message's link came from the service (Copy Link): to the clipboard.
    pub fn copied(&self, link: &str) {
        system::copy_text(link);
        notify::info(&tr("Link copied"));
    }

    /// The chat's advertisement came, or went (part 5).
    pub fn ad_told(&mut self, key: &ChatKey, ad: Option<AdInfo>) {
        if let Some(tab) = self.tabs.get_mut(key) {
            tab.ad = ad;
        }
    }

    /// Reporting the ad: Telegram's options, as a choice under its title (part 5).
    pub fn ad_report(
        &mut self,
        key: ChatKey,
        message: String,
        title: &str,
        options: Vec<(String, String)>,
    ) {
        let Some(tab) = self.tabs.get(&key) else {
            return;
        };
        let items: Vec<_> = options
            .iter()
            .map(|(id, text)| picker::item(id, text))
            .collect();
        // Telegram's question as the field's gray text, the options under it.
        let id = picker::Picker::new(title)
            .items(items)
            .under(&tab.window, FEED)
            .open();
        self.report_picker = Some((id, key, message));
    }

    /// The message is the chat's advertisement (part 5).
    fn is_ad(&self, key: &ChatKey, message: &str) -> bool {
        self.tabs
            .get(key)
            .and_then(|tab| tab.ad.as_ref())
            .is_some_and(|ad| ad.message == message)
    }

    /// The advertisement's button or picture (`media`): Telegram is told, the sponsor's page
    /// opens as a button's link does (part 5): Telegram's in FluxGram, the web's asked first.
    fn open_ad(&mut self, key: &ChatKey, message: &str, media: bool) {
        let Some(ad) = self
            .tabs
            .get(key)
            .and_then(|tab| tab.ad.clone())
            .filter(|ad| ad.message == message)
        else {
            return;
        };
        ask(&Request::Ad(AdAsk::Clicked {
            key: key.clone(),
            message: message.to_string(),
            media,
        }));
        self.follow(key, &ad.url, Shown::Button);
    }

    /// A bot's button under a message, or of its keyboard over the field (`message` empty): what
    /// its id says (part 5).
    fn bot_pressed(&mut self, key: &ChatKey, message: String, button: &str) {
        use crate::bots::Press;
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let window = tab.window.clone();
        match Press::of(button) {
            Some(Press::Callback(data)) => ask(&Request::BotCallback {
                key: key.clone(),
                message,
                data,
            }),
            Some(Press::Link(url)) => self.follow(key, &url, Shown::Button),
            Some(Press::Field(query)) => {
                conv::update(&window, FEED, vec![conv::draft(&format!("{query} "))])
            }
            Some(Press::Copy(text)) => {
                system::copy_text(&text);
                notify::info(&tr("Copied"));
            }
            Some(Press::User(user)) => self
                .go
                .push(TabGo::Profile(ChatKey::new(&key.account, user))),
            Some(Press::Text(text)) => ask(&Request::BotText {
                key: key.clone(),
                text,
            }),
            Some(Press::Unsupported(what, url)) => {
                let question = Ask::info(&tr("Not Available Here")).message(&tr(what.sentence()));
                match url {
                    Some(url) => {
                        let id = question
                            .primary(&tr("Open in Browser"))
                            .cancel(&tr("Cancel"))
                            .send();
                        self.link_asked = Some((id, url));
                    }
                    None => {
                        question.primary(&tr("OK")).send();
                    }
                }
            }
            None => {}
        }
    }

    /// A bot answered a button (part 5): its text as a notification, or as a calm dialog under
    /// its name when it asks for one; its page opened as a button's link.
    pub fn bot_answer(&mut self, key: &ChatKey, bot: &str, text: &str, alert: bool, url: &str) {
        if !text.is_empty() {
            if alert {
                Ask::info(bot).message(text).primary(&tr("OK")).send();
            } else {
                notify::info(text);
            }
        }
        if !url.is_empty() {
            self.follow(key, url, Shown::Button);
        }
    }

    /// A bot asks for a reply to `message` (part 5): its quote over the field.
    pub fn force_reply(&mut self, key: &ChatKey, message: &str) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let Some(found) = tab.messages.get(message) else {
            return;
        };
        let quote = quote_of(found, tab, &|text| tr(text));
        let window = tab.window.clone();
        self.set_reply(key, Some(message.to_string()));
        conv::update(&window, FEED, vec![conv::reply_to(Some(quote))]);
    }

    /// The bot's menu (part 5): its commands, each with its description, under the field.
    fn open_commands(&mut self, key: &ChatKey) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let Some(header) = tab.header.as_ref() else {
            return;
        };
        let items: Vec<_> = header
            .commands
            .iter()
            .map(|(command, description)| picker::item(command, command).detail(description))
            .collect();
        // Above the field (part 5), as its suggestions are.
        let id = picker::Picker::menu()
            .items(items)
            .under(&tab.window, &format!("{FEED}/field"))
            .open();
        self.commands_picker = Some((id, key.clone()));
    }

    /// About the Ad (part 5): the sponsor's info and the ad's, in a calm dialog.
    fn about_ad(&self, key: &ChatKey) {
        let Some(ad) = self.tabs.get(key).and_then(|tab| tab.ad.as_ref()) else {
            return;
        };
        let about = ad.about(&tr("Telegram shows ads in public channels."));
        Ask::info(&tr("About the Ad"))
            .message(&about)
            .primary(&tr("OK"))
            .send();
    }

    /// An item of the ⋯ menu (a scenario's `menu/<item>` too).
    fn choose(&mut self, key: ChatKey, choice: &str) {
        match choice {
            "search" => self.open_find(&key),
            "profile" => self.go.push(TabGo::Profile(key)),
            "media" => self.go.push(TabGo::Media(key)),
            "mute" => self.open_mute_menu(&key),
            "read" => ask(&Request::MarkRead { key, read: true }),
            "unread" => ask(&Request::MarkRead { key, read: false }),
            "unmute" => self.go.push(TabGo::Mute(key, false)),
            "hide" => self.go.push(TabGo::Hide(key)),
            "unhide" => self.go.push(TabGo::Unhide(key)),
            "folder" => self.go.push(TabGo::Folder(key)),
            "remove" => ask(&Request::RemoveChat { key }),
            "add" => ask(&Request::AddChat { key }),
            // Claude (part 6): on or off; the rows bring the change back.
            "claude" => {
                let on = !self.claude_on(&key);
                let chats = self.claude.entry(key.account.clone()).or_default();
                if on {
                    chats.insert(key.chat);
                } else {
                    chats.remove(&key.chat);
                }
                ask(&Request::Claude(ClaudeAsk::Set { key, on }))
            }
            // A bot (part 5): stopped after a question, restarted at once.
            "stop-bot" => self.stop_bot(key),
            "restart-bot" => ask(&Request::BotStart { key, restart: true }),
            _ => {}
        }
    }

    /// Stop Bot of the tab's ⋯ or the list row's (part 5): asked first.
    pub fn stop_bot(&mut self, key: ChatKey) {
        let id = Ask::warning(&tr("Stop the bot?"))
            .message(&tr("It won't be able to write to you."))
            .primary(&tr("Stop"))
            .cancel(&tr("Cancel"))
            .send();
        self.stop_asked = Some((id, key));
    }

    /// Opens a hidden chat's tab: it never comes back after a restart.
    pub fn open_hidden(&mut self, key: &ChatKey, title: Option<&str>) {
        self.open(key, title);
        if let Some(tab) = self.tabs.get_mut(key) {
            tab.hidden = true;
            ui::set_restorable(&tab.window, false);
        }
    }

    /// The chat was hidden (true) or shown in the list again: its tab, if open here, comes back
    /// after a restart only when it isn't hidden.
    pub fn set_hidden(&mut self, key: &ChatKey, hidden: bool) {
        let Some(tab) = self.tabs.get_mut(key) else {
            return;
        };
        if tab.hidden != hidden {
            tab.hidden = hidden;
            ui::set_restorable(&tab.window, !hidden);
        }
    }

    /// Closes the chat's tab, if it is open here.
    pub fn close(&mut self, key: &ChatKey) {
        if let Some(window) = self.tabs.get(key).map(|tab| tab.window.clone()) {
            ui::hide(&window);
            self.closed(&window);
        }
    }

    /// Closes the hidden chats' tabs: the phrase left the list's search.
    pub fn close_hidden(&mut self) {
        let hidden: Vec<ChatKey> = self
            .tabs
            .iter()
            .filter(|(_, tab)| tab.hidden)
            .map(|(key, _)| key.clone())
            .collect();
        for key in hidden {
            self.close(&key);
        }
    }

    /// What the tabs asked the window for since the last call.
    pub fn take_go(&mut self) -> Vec<TabGo> {
        std::mem::take(&mut self.go)
    }

    /// Opens the chat's tab (or brings it forward) with the list's title for it, and asks the
    /// service for the chat ([`Request::OpenChat`]).
    pub fn open(&mut self, key: &ChatKey, title: Option<&str>) {
        let known = self.tabs.get(key).map(|tab| tab.title.clone());
        let title = title
            .map(str::to_string)
            .or(known)
            .unwrap_or_else(|| tr("Chat"));
        let window = open_tab(CHAT, &key.tab(), &title, true);
        if self.tabs.contains_key(key) {
            return;
        }
        self.start(key, window, title);
    }

    /// A tab this instance didn't know: its view, and the chat asked for.
    fn start(&mut self, key: &ChatKey, window: String, title: String) {
        let gone = self
            .accounts
            .as_ref()
            .is_some_and(|accounts| !accounts.contains(&key.account));
        self.tabs.insert(
            key.clone(),
            Tab {
                window,
                title,
                header: None,
                messages: HashMap::new(),
                gone,
                in_sight: false,
                hidden: false,
                find: None,
                reply: None,
                view: None,
                ad: None,
            },
        );
        self.render(key);
        if !gone {
            ask(&Request::OpenChat { key: key.clone() });
        }
        // Messages waiting to be forwarded here (part 5): their bar again.
        if let (Some(forward), Some(tab)) = (self.forwards.get(key), self.tabs.get(key)) {
            let quote = Some(forward.quote.clone());
            conv::update(&tab.window, FEED, vec![conv::forwarding(quote)]);
        }
    }

    /// Forward (part 5), of a message or of selected ones: those Telegram lets go — the others
    /// told calmly — go to the list's picker of a chat.
    fn forward(&mut self, key: &ChatKey, messages: &[String]) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let (can, cannot) = forwardable(messages, &tab.messages);
        let words = crate::service::HostWords;
        if cannot > 0 {
            notify::info(&words.count(
                cannot as u64,
                "{0} message can't be forwarded.",
                "{0} messages can't be forwarded.",
            ));
        }
        if can.is_empty() {
            return;
        }
        let found: Vec<&Message> = can.iter().filter_map(|id| tab.messages.get(id)).collect();
        let quote = forward_quote(&found, tab, &words);
        self.go.push(TabGo::Forward(Forward {
            from: key.clone(),
            messages: can,
            quote,
        }));
    }

    /// The Forward picker's chat (part 5): the bar over its field (the tab opens after), the quote
    /// being answered there gone.
    pub fn set_forward(&mut self, key: &ChatKey, forward: Forward) {
        if let Some(tab) = self.tabs.get(key) {
            let quote = Some(forward.quote.clone());
            conv::update(&tab.window, FEED, vec![conv::forwarding(quote)]);
        }
        self.set_reply(key, None);
        self.forwards.insert(key.clone(), forward);
    }

    /// The selection's Delete (part 5): one question for all — "Also delete for Anna" (for
    /// everyone in a group) when each of them can go for everyone, as a message's Delete decides.
    fn ask_delete_many(&mut self, key: &ChatKey, messages: &[String]) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let (can, for_all) = deletable(messages, &tab.messages);
        if can.is_empty() {
            notify::info(&tr("These messages can't be deleted."));
            return;
        }
        let words = crate::service::HostWords;
        let title = if can.len() == 1 {
            tr("Delete the message?")
        } else {
            words.count(
                can.len() as u64,
                "Delete {0} message?",
                "Delete {0} messages?",
            )
        };
        let id = if for_all {
            let name = tab
                .header
                .as_ref()
                .map_or_else(|| tab.title.clone(), |header| header.title.clone());
            let label = if key.chat > 0 {
                trf("Also delete for {0}", &[&name])
            } else {
                tr("Delete for everyone")
            };
            Form::new(&title, &tr("Delete"))
                .toggle("revoke", &label, true)
                .send()
        } else {
            Ask::warning(&title)
                .message(&tr("They will be deleted only for you."))
                .danger(&tr("Delete"))
                .cancel(&tr("Cancel"))
                .send()
        };
        self.delete_many = Some((id, key.clone(), can));
    }

    /// The chat's header changed.
    pub fn header(&mut self, key: &ChatKey, header: ChatHeader) {
        let Some(tab) = self.tabs.get_mut(key) else {
            return;
        };
        if !header.title.is_empty() && header.title != tab.title {
            tab.title = header.title.clone();
            ui::set_title(&tab.window, &tab.title);
        }
        tab.header = Some(header);
        self.render(key);
    }

    /// Changes of the chat's feed: kept as they come, and shown with the search's words marked
    /// while the search is open.
    pub fn feed(&mut self, key: &ChatKey, mut changes: Vec<Change>) {
        let Some(tab) = self.tabs.get_mut(key).filter(|tab| !tab.gone) else {
            return;
        };
        let claude = self.claude_drafts.get(key);
        for change in &mut changes {
            match change {
                // Edit on Claude's card opened the tab (part 6): its text, not the saved draft.
                Change::Draft(text) if claude.is_some() => {
                    *text = claude.cloned().unwrap_or_default();
                }
                Change::Put(messages) => {
                    for message in messages.iter_mut() {
                        tab.messages.insert(message.id.clone(), message.clone());
                        let Some(find) = tab.find.as_mut() else {
                            continue;
                        };
                        match highlighted(message, &find.words) {
                            Some(marked) => {
                                find.lit.insert(message.id.clone());
                                *message = marked;
                            }
                            None => {
                                find.lit.remove(&message.id);
                            }
                        }
                    }
                }
                Change::Remove(ids) => {
                    for id in ids.iter() {
                        tab.messages.remove(id);
                        if let Some(find) = tab.find.as_mut() {
                            find.lit.remove(id);
                        }
                    }
                }
                Change::Clear => {
                    tab.messages.clear();
                    if let Some(find) = tab.find.as_mut() {
                        find.lit.clear();
                    }
                }
                _ => {}
            }
        }
        conv::update(&tab.window, FEED, changes);
        if !tab.messages.is_empty() {
            self.claude_drafts.remove(key);
        }
        // A link's message (part 5): revealed once the feed is there.
        if !tab.messages.is_empty()
            && let Some(message) = self.jumps.remove(key)
        {
            ask(&Request::JumpTo {
                key: key.clone(),
                message,
            });
        }
    }

    /// The user did something in a chat's tab.
    pub fn ui(&mut self, window: &str, element: &str, event: UiEvent) {
        let Some(key) = key_of(window) else {
            return;
        };
        // A scenario's `ui:<plugin>/chat/<key>/open`.
        if element == "open" && matches!(event, UiEvent::Clicked) {
            self.open(&key, None);
            return;
        }
        let Some(tab) = self.tabs.get(&key).filter(|tab| !tab.gone) else {
            return;
        };
        let window = tab.window.clone();
        match (element, event) {
            (HEADER, UiEvent::Clicked) => self.go.push(TabGo::Profile(key)),
            (FIND, UiEvent::Clicked) | (_, UiEvent::Find) => self.open_find(&key),
            (MORE, UiEvent::Clicked) => self.open_menu(&key),
            (FIND_FIELD, UiEvent::Changed(text)) => self.find_typed(&key, text),
            // ↵: the query at once when it changed, else the next older match.
            (FIND_FIELD, UiEvent::Submitted(text)) => {
                let changed = tab
                    .find
                    .as_ref()
                    .is_some_and(|find| find.query != text.trim());
                self.find_typed(&key, text);
                if changed {
                    self.find_now(&key);
                } else {
                    self.find_step(&key, true);
                }
            }
            (FIND_OLDER, UiEvent::Clicked) | (_, UiEvent::FindNext) => self.find_step(&key, true),
            (FIND_NEWER, UiEvent::Clicked) | (_, UiEvent::FindPrevious) => {
                self.find_step(&key, false)
            }
            // × of the bar, or Esc in its field.
            (FIND_CLOSE, UiEvent::Clicked) | (FIND_FIELD, UiEvent::Cancelled) => {
                self.close_find(&key)
            }
            (FEED, UiEvent::LoadOlder) => ask(&Request::LoadOlder { key }),
            (FEED, UiEvent::LoadNewer) => ask(&Request::LoadNewer { key }),
            (FEED, UiEvent::Visible((first, last))) => ask(&Request::Visible { key, first, last }),
            // ↵, or Send Without Sound from Send's menu (part 5).
            (FEED, UiEvent::Composed(composed)) => self.composed(&key, composed, false),
            (FEED, UiEvent::ComposedWith((option, composed))) => {
                self.composed(&key, composed, option == SILENT)
            }
            // "@" and "/" in the field (part 5): the service answers this window.
            (FEED, UiEvent::Suggest((trigger, text))) => {
                ask(&Request::Suggest { key, trigger, text })
            }
            (FEED, UiEvent::DraftChanged(text)) => ask(&Request::Draft { key, text }),
            (FEED, UiEvent::MessageAction((message, action))) => {
                self.message_action(&key, &message, &action, None)
            }
            // An action over a selected part of the message: Reply quotes just that part.
            (FEED, UiEvent::MessageActionQuoted((message, action, text, offset))) => {
                self.message_action(&key, &message, &action, Some((text, offset)))
            }
            (FEED, UiEvent::Edited((message, text))) => {
                if !text.trim().is_empty() {
                    ask(&Request::Edit { key, message, text });
                }
                conv::update(&window, FEED, vec![conv::editing(None)]);
            }
            (FEED, UiEvent::EditCancelled) => {
                conv::update(&window, FEED, vec![conv::editing(None)])
            }
            (FEED, UiEvent::EditLast) => {
                let last = tab
                    .messages
                    .values()
                    .filter(|message| offers(message, EDIT))
                    .max_by_key(|message| message.order)
                    .map(|message| message.id.clone());
                if let Some(message) = last {
                    self.message_action(&key, &message, EDIT, None);
                }
            }
            (FEED, UiEvent::ReplyCancelled) => {
                self.set_reply(&key, None);
                conv::update(&window, FEED, vec![conv::reply_to(None)])
            }
            // Part 5: the selection's Forward and Delete; the forwarding bar's ×.
            (FEED, UiEvent::SelectionAction((action, messages))) => match action.as_str() {
                FORWARD => self.forward(&key, &messages),
                DELETE_SELECTED => self.ask_delete_many(&key, &messages),
                _ => {}
            },
            // A vote, a reaction (part 5): the service sends them; the message comes again.
            (FEED, UiEvent::PollVoted((message, _, options))) => ask(&Request::Vote {
                key: key.clone(),
                message,
                options,
            }),
            (FEED, UiEvent::ReactionToggled((message, emoji))) => ask(&Request::React {
                key: key.clone(),
                message,
                emoji,
            }),
            (FEED, UiEvent::ForwardCancelled) => {
                self.forwards.remove(&key);
            }
            // A voice or round message played for the first time: listened to in Telegram.
            (FEED, UiEvent::MediaPlayed((message, part))) => {
                ask(&Request::MediaPlayed { key, message, part })
            }
            // The field's panel: its tabs' content comes from the service.
            (FEED, UiEvent::PanelOpened(tab)) => {
                ask(&Request::Panel(PanelAsk::Opened { key, tab }))
            }
            (FEED, UiEvent::PanelSearched((tab, text))) => {
                ask(&Request::Panel(PanelAsk::Searched { key, tab, text }))
            }
            (FEED, UiEvent::PanelVisible((tab, first, last))) => {
                ask(&Request::Panel(PanelAsk::Visible {
                    key,
                    tab,
                    first,
                    last,
                }))
            }
            (FEED, UiEvent::PanelLoadMore(tab)) => {
                ask(&Request::Panel(PanelAsk::LoadMore { key, tab }))
            }
            // A sticker or a GIF: sent, answering the quote over the field (which goes then).
            (FEED, UiEvent::PanelChosen((tab, item))) => {
                let reply_to = self.set_reply(&key, None);
                if reply_to.is_some() {
                    conv::update(&window, FEED, vec![conv::reply_to(None)]);
                }
                ask(&Request::Panel(PanelAsk::Chosen {
                    key,
                    tab,
                    item,
                    reply_to,
                }));
            }
            // The pinned message's bar (part 5): the service reveals it and moves the bar on.
            // A bot's advertisement in the bar (part 5): its page.
            (FEED, UiEvent::PinnedClicked(message)) if self.is_ad(&key, &message) => {
                self.open_ad(&key, &message, false)
            }
            (FEED, UiEvent::PinnedClicked(message)) => {
                ask(&Request::PinnedClicked { key, message })
            }
            (FEED, UiEvent::PinnedAction((message, action))) => {
                self.pinned_action(&key, &message, &action)
            }
            (FEED, UiEvent::PartClicked((message, _))) if self.is_ad(&key, &message) => {
                self.open_ad(&key, &message, true)
            }
            (FEED, UiEvent::PartClicked((message, part))) => {
                ask(&Request::Download { key, message, part })
            }
            (FEED, UiEvent::QuoteClicked((_, message))) => ask(&Request::JumpTo { key, message }),
            // Part 5: the button in place of the field; an advertisement's button.
            (FEED, UiEvent::ComposerAction((action, _))) if action == JOIN => {
                ask(&Request::Join { key })
            }
            (FEED, UiEvent::ComposerAction((action, _)))
                if action == MUTE_CHANNEL || action == UNMUTE_CHANNEL =>
            {
                self.go.push(TabGo::Mute(key, action == MUTE_CHANNEL))
            }
            (FEED, UiEvent::KeyboardPressed((message, _))) if self.is_ad(&key, &message) => {
                self.open_ad(&key, &message, false)
            }
            // A bot's button (part 5).
            (FEED, UiEvent::KeyboardPressed((message, button))) => {
                self.bot_pressed(&key, message, &button)
            }
            (FEED, UiEvent::ComposerAction((action, _))) if action == START_BOT => {
                ask(&Request::BotStart {
                    key,
                    restart: false,
                })
            }
            (FEED, UiEvent::ComposerAction((action, _))) if action == UNBLOCK_BOT => {
                ask(&Request::Block {
                    key,
                    blocked: false,
                })
            }
            (FEED, UiEvent::ComposerAction((action, _))) if action == BOT_MENU => {
                self.open_commands(&key)
            }
            // Reply with Claude (part 6): the field's text goes along; it stays in the field.
            (FEED, UiEvent::ComposerAction((action, composed))) if action == REPLY_CLAUDE => {
                self.reply_with_claude(&key, None, Some(composed.text))
            }
            (FEED, UiEvent::LinkClicked((message, link))) => self.link(&key, &message, &link),
            // A scenario's menu items (`menu/hide`) and message actions: `reply/<id>`,
            // `edit/<id>`, `delete/<id>`.
            (element, UiEvent::Clicked) if element.starts_with("menu/") => {
                self.choose(key, &element["menu/".len()..])
            }
            (element, UiEvent::Clicked) => {
                if let Some((action, message)) = element.split_once('/') {
                    let action = match action {
                        "delete" => tab
                            .messages
                            .get(message)
                            .filter(|found| offers(found, DELETE_FOR_ME))
                            .map_or(DELETE, |_| DELETE_FOR_ME),
                        action => action,
                    };
                    self.message_action(&key, message, action, None);
                }
            }
            // A scenario's field: `send=<text>`, `draft=<text>`.
            ("send", UiEvent::Submitted(text)) => ask(&Request::Send {
                key,
                composed: Composed {
                    text,
                    files: Vec::new(),
                    reply_to: None,
                },
                silent: false,
                forward: None,
            }),
            ("draft", UiEvent::Submitted(text)) => ask(&Request::Draft { key, text }),
            _ => {}
        }
    }

    /// The field was sent (`silent`: without sound): the quote over it goes.
    fn composed(&mut self, key: &ChatKey, composed: Composed, silent: bool) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let window = tab.window.clone();
        // The messages to forward go after the text (part 5); Flux has taken the bar away.
        let forward = self
            .forwards
            .remove(key)
            .map(|forward| (forward.from, forward.messages));
        if !composed.text.trim().is_empty() || !composed.files.is_empty() {
            ask(&Request::Send {
                key: key.clone(),
                composed,
                silent,
                forward,
            });
        } else if let Some((from, messages)) = forward {
            ask(&Request::Forward {
                key: key.clone(),
                from,
                messages,
                silent,
            });
        }
        self.set_reply(key, None);
        conv::update(&window, FEED, vec![conv::reply_to(None)]);
    }

    /// A message's action: Reply puts its quote over the field, Edit puts its text in the field,
    /// Delete asks first.
    fn message_action(
        &mut self,
        key: &ChatKey,
        message: &str,
        action: &str,
        selected: Option<(String, u32)>,
    ) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let Some(found) = tab
            .messages
            .get(message)
            .filter(|found| offers(found, action))
        else {
            return;
        };
        match action {
            REPLY => {
                let mut quote = quote_of(found, tab, &|text: &str| tr(text));
                if let Some((text, _)) = &selected {
                    quote.text = crate::summary::first_line(text);
                }
                // The service sends the reply with the quoted part (or forgets an older one).
                ask(&Request::Quote {
                    key: key.clone(),
                    message: message.to_string(),
                    quote: selected,
                });
                conv::update(&tab.window, FEED, vec![conv::reply_to(Some(quote))]);
                self.set_reply(key, Some(message.to_string()));
            }
            REPLY_CLAUDE => self.reply_with_claude(key, Some(message.to_string()), None),
            EDIT => {
                // As Markdown: saving parses it back, so the bold and the links stay.
                let text = crate::convert::markdown(found);
                conv::update(
                    &tab.window,
                    FEED,
                    vec![conv::editing(Some((message, &text)))],
                );
            }
            DELETE | DELETE_FOR_ME => {
                let line = if action == DELETE {
                    tr("It will be deleted for everyone in the chat.")
                } else {
                    tr("It will be deleted only for you.")
                };
                let id = Ask::warning(&tr("Delete the message?"))
                    .message(&line)
                    .danger(&tr("Delete"))
                    .cancel(&tr("Cancel"))
                    .send();
                self.asked = Some((id, key.clone(), message.to_string()));
            }
            PIN => self.ask_pin(key, message),
            UNPIN => self.ask_unpin(key, message),
            RETRACT_VOTE => ask(&Request::Vote {
                key: key.clone(),
                message: message.to_string(),
                options: Vec::new(),
            }),
            STOP_POLL => {
                let id = Ask::info(&tr("Stop the poll?"))
                    .message(&tr("Its results will be final: nobody will vote any more."))
                    .primary(&tr("Stop"))
                    .cancel(&tr("Cancel"))
                    .send();
                self.stop_poll_asked = Some((id, key.clone(), message.to_string()));
            }
            FORWARD => self.forward(key, &[message.to_string()]),
            COPY_LINK => ask(&Request::CopyLink {
                key: key.clone(),
                message: message.to_string(),
            }),
            FAVE_STICKER | UNFAVE_STICKER | SAVE_GIF => ask(&Request::KeepMedia {
                key: key.clone(),
                message: message.to_string(),
                keep: action != UNFAVE_STICKER,
            }),
            ABOUT_AD => self.about_ad(key),
            REPORT_AD => ask(&Request::Ad(AdAsk::Report {
                key: key.clone(),
                message: message.to_string(),
                option: String::new(),
            })),
            _ => {}
        }
    }

    /// The pinned bar's ×: Unpin, asked first; or, where the user can't unpin, the bar hidden
    /// until a newer message is pinned (remembered, except for a hidden chat).
    fn pinned_action(&mut self, key: &ChatKey, message: &str, action: &str) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        match action {
            ABOUT_AD => self.about_ad(key),
            crate::pins::UNPIN => self.ask_unpin(key, message),
            crate::pins::HIDE => ask(&Request::HidePinned {
                key: key.clone(),
                message: message.to_string(),
                remember: !tab.hidden,
            }),
            _ => {}
        }
    }

    /// «Unpin the message?»: a title and one sentence.
    fn ask_unpin(&mut self, key: &ChatKey, message: &str) {
        let id = Ask::info(&tr("Unpin the message?"))
            .message(&tr("It will be unpinned for everyone in the chat."))
            .primary(&tr("Unpin"))
            .cancel(&tr("Cancel"))
            .send();
        self.unpin_asked = Some((id, key.clone(), message.to_string()));
    }

    /// «Pin the message?» with one checkbox: in a group, tell every member; in a one-to-one
    /// chat, pin it for the other person too.
    fn ask_pin(&mut self, key: &ChatKey, message: &str) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let name = tab
            .header
            .as_ref()
            .map_or_else(|| tab.title.clone(), |header| header.title.clone());
        let mut form = Form::new(&tr("Pin the message?"), &tr("Pin"));
        form = match key.chat > 0 {
            true => form.toggle("both", &trf("Also pin for {0}", &[&name]), true),
            false => form.toggle("notify", &tr("Notify all members"), true),
        };
        let id = form.send();
        self.pin_asked = Some((id, key.clone(), message.to_string()));
    }

    /// The Pin question's answer (part 5); false — not the tabs'.
    pub fn form_answered(&mut self, id: u64, values: Option<Vec<(String, String)>>) -> bool {
        // Selected messages' Delete, with "Also delete for…" (part 5).
        if let Some((_, key, messages)) = self.delete_many.take_if(|(asked, _, _)| *asked == id) {
            if let Some(values) = values.map(Values::from) {
                ask(&Request::DeleteMany {
                    key,
                    messages,
                    revoke: values.get("revoke") == Some("true"),
                });
            }
            return true;
        }
        let Some((_, key, message)) = self.pin_asked.take_if(|(asked, _, _)| *asked == id) else {
            return false;
        };
        if let Some(values) = values.map(Values::from) {
            ask(&Request::Pin {
                key,
                message,
                notify: values.get("notify") == Some("true"),
                for_both: values.get("both") == Some("true"),
            });
        }
        true
    }

    /// The message the quote over the field answers (none — no quote); the one before.
    fn set_reply(&mut self, key: &ChatKey, reply: Option<String>) -> Option<String> {
        let tab = self.tabs.get_mut(key)?;
        std::mem::replace(&mut tab.reply, reply)
    }

    // --- Links (part 5, `links.rs`) ---

    /// A link of the feed — every one, the conversation owns its links: a text's, a link
    /// preview's, a quote's. The message tells what the link reads as.
    fn link(&mut self, key: &ChatKey, message: &str, link: &str) {
        let text = self
            .tabs
            .get(key)
            .and_then(|tab| tab.messages.get(message))
            .and_then(|found| crate::links::link_text(found, link));
        let shown = text.as_deref().map_or(Shown::Address, Shown::Text);
        self.follow(key, link, shown);
    }

    /// Follows a link clicked in the chat `key`, whose account opens it ([`crate::links`]):
    /// Telegram's in FluxGram, the text's entities as they say, the web's asked first as the
    /// settings say.
    pub fn follow(&mut self, key: &ChatKey, link: &str, shown: Shown) {
        match Target::of(link) {
            Target::Telegram(url) => ask(&Request::Link(LinkAsk::Open {
                key: key.clone(),
                url,
            })),
            Target::Mention(username) => ask(&Request::Link(LinkAsk::Mention {
                key: key.clone(),
                username,
            })),
            Target::User(user) => self
                .go
                .push(TabGo::Profile(ChatKey::new(&key.account, user))),
            Target::Command(command) => ask(&Request::Send {
                key: key.clone(),
                composed: Composed {
                    text: command,
                    files: Vec::new(),
                    reply_to: None,
                },
                silent: false,
                forward: None,
            }),
            Target::Search(tag) => self.find_text(key, &tag),
            Target::Phone(number) => {
                system::copy_text(&number);
                notify::info(&tr("Phone number copied"));
            }
            Target::Card(number) => {
                system::copy_text(&number);
                notify::info(&tr("Card number copied"));
            }
            Target::Mail(url) => {
                let _ = system::open_url(&url);
            }
            Target::Web(url) => {
                let policy = crate::links::Policy::of(
                    flux_plugin_api::setting::<String>("ask-links").as_deref(),
                );
                match crate::links::web(&url, shown, policy) {
                    Web::Open => {
                        let _ = system::open_url(&url);
                    }
                    Web::Ask { shown, spoofed } => {
                        let message = match spoofed {
                            true => format!(
                                "{shown}\n\n{}",
                                tr(
                                    "The address mixes letters of different alphabets — check it is the site you expect."
                                )
                            ),
                            false => shown,
                        };
                        let id = Ask::info(&tr("Open the link?"))
                            .message(&message)
                            .primary(&tr("Open"))
                            .normal(&tr("Copy"))
                            .cancel(&tr("Cancel"))
                            .send();
                        self.web_asked = Some((id, url));
                    }
                }
            }
        }
    }

    /// What a link opens, from the service (part 5).
    pub fn link_told(&mut self, told: LinkTold) {
        match told {
            LinkTold::Open {
                key,
                title,
                message,
                draft,
            } => {
                let title = (!title.is_empty()).then_some(title);
                self.open(&key, title.as_deref());
                self.go.push(TabGo::Opened(key.clone()));
                // Its message revealed: at once in a feed that is there, else once it comes (a
                // jump before the first page would start another load).
                if let Some(message) = message {
                    match self
                        .tabs
                        .get(&key)
                        .is_some_and(|tab| !tab.messages.is_empty())
                    {
                        true => ask(&Request::JumpTo {
                            key: key.clone(),
                            message,
                        }),
                        false => {
                            self.jumps.insert(key.clone(), message);
                        }
                    }
                }
                if let (Some(draft), Some(tab)) = (draft, self.tabs.get(&key)) {
                    conv::update(&tab.window, FEED, vec![conv::draft(&draft)]);
                }
            }
            LinkTold::Profile { key } => self.go.push(TabGo::Profile(key)),
            LinkTold::Invite {
                key,
                link,
                title,
                members,
                channel,
                request,
            } => {
                let words = crate::service::HostWords;
                let mut line = match channel {
                    true => words.count(members, "{0} subscriber", "{0} subscribers"),
                    false => words.count(members, "{0} member", "{0} members"),
                };
                // The digits grouped as Telegram writes them: "1 234 members".
                line = line.replace(&members.to_string(), &grouped(members));
                if request {
                    line = format!("{line}. {}", tr("The admins will review your request."));
                }
                let id = Ask::info(&trf("Join «{0}»?", &[&title]))
                    .message(&line)
                    .primary(&tr(if request { "Request to Join" } else { "Join" }))
                    .cancel(&tr("Cancel"))
                    .send();
                self.invite_asked = Some((id, key, link));
            }
            LinkTold::Stickers {
                key,
                set,
                title,
                count,
                installed,
            } => {
                if installed {
                    notify::info(&trf("The «{0}» stickers are already added", &[&title]));
                    return;
                }
                let words = crate::service::HostWords;
                let id = Ask::info(&trf("Add the «{0}» stickers?", &[&title]))
                    .message(&words.count(count, "{0} sticker", "{0} stickers"))
                    .primary(&tr("Add"))
                    .cancel(&tr("Cancel"))
                    .send();
                self.stickers_asked = Some((id, key, set));
            }
            LinkTold::Said { text } => {
                notify::info(&tr(&text));
            }
            LinkTold::Unsupported { url } => {
                let question = Ask::info(&tr("FluxGram doesn't open such links"));
                if crate::links::is_telegram(&url) && url.to_ascii_lowercase().starts_with("tg:") {
                    question.primary(&tr("OK")).send();
                    return;
                }
                let id = question
                    .message(&url)
                    .primary(&tr("Open in Browser"))
                    .cancel(&tr("Cancel"))
                    .send();
                self.link_asked = Some((id, url));
            }
        }
    }

    /// A #hashtag or a $cashtag: the chat's search with it (part 5).
    fn find_text(&mut self, key: &ChatKey, text: &str) {
        let Some(tab) = self.tabs.get_mut(key) else {
            return;
        };
        let find = tab.find.get_or_insert_with(Find::default);
        find.text = text.to_string();
        if let Some(timer) = find.timer.take() {
            timers::cancel(timer);
        }
        // The field shows the text: a bar on show is drawn anew with it.
        tab.view = None;
        self.render(key);
        self.find_now(key);
    }

    /// The chats in an account's list (part 5): ⋯ of another chat's tab offers Add to FluxGram.
    /// Reply with Claude follows the list (part 6): the tabs of a chat that joined or left it
    /// are drawn again — the first rows may come after the tab was drawn.
    pub fn listed(&mut self, account: &str, chats: impl Iterator<Item = i64>) {
        let old = self.listed.insert(account.to_string(), chats.collect());
        let new = self.listed.get(account);
        let changed: Vec<ChatKey> = self
            .tabs
            .iter()
            .filter(|(key, tab)| {
                key.account == account
                    && for_claude(tab.hidden, old.as_ref(), key.chat)
                        != for_claude(tab.hidden, new, key.chat)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in changed {
            self.render(&key);
        }
    }

    /// The chats of an account's list available to Claude (part 6): ⋯'s item is checked.
    pub fn claude(&mut self, account: &str, chats: impl Iterator<Item = i64>) {
        self.claude.insert(account.to_string(), chats.collect());
    }

    /// The chat is available to Claude.
    fn claude_on(&self, key: &ChatKey) -> bool {
        self.claude
            .get(&key.account)
            .is_some_and(|chats| chats.contains(&key.chat))
    }

    /// Reply with Claude (part 6): the chat on for Claude (⋯'s item at once; the service's rows
    /// bring the list's mark), and the service makes the session's attachment — of a message, or
    /// of the field's unfinished text — which comes back to open ([`crate::model::Note::Session`]).
    fn reply_with_claude(&mut self, key: &ChatKey, message: Option<String>, draft: Option<String>) {
        self.claude
            .entry(key.account.clone())
            .or_default()
            .insert(key.chat);
        ask(&Request::Claude(ClaudeAsk::Reply {
            key: key.clone(),
            message,
            draft,
        }));
    }

    /// Edit on Claude's card (part 6, `send_message`): the chat's tab here, its field with the
    /// text and the quote of the message answered, taking the keyboard. The user sends it.
    pub fn claude_draft(&mut self, card: &Card) {
        let key = &card.chat;
        self.open(key, Some(&card.title));
        self.go.push(TabGo::Opened(key.clone()));
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        // The feed not here yet: its first fill may bring the saved draft — this text wins.
        if tab.messages.is_empty() {
            self.claude_drafts.insert(key.clone(), card.text.clone());
        }
        let mut changes = vec![conv::draft(&card.text)];
        if let Some(reply) = &card.reply {
            // A part of another message quoted before is forgotten: the whole one is answered.
            ask(&Request::Quote {
                key: key.clone(),
                message: reply.message.clone(),
                quote: None,
            });
            changes.push(conv::reply_to(Some(MessageQuote {
                message: Some(reply.message.clone()),
                author: reply.author.clone(),
                color: reply.color,
                text: reply.text.clone(),
            })));
        }
        conv::update(&tab.window, FEED, changes);
        self.set_reply(key, card.reply.as_ref().map(|reply| reply.message.clone()));
    }

    /// A chat's tab came into sight, or went out of it (a tab Flux brought back comes here first).
    pub fn in_sight(&mut self, window: &str, in_sight: bool) {
        let Some(key) = key_of(window) else {
            return;
        };
        if !self.tabs.contains_key(&key) {
            if !in_sight {
                return;
            }
            // Brought back with the project: filled again from its key.
            self.start(&key, window.to_string(), tr("Chat"));
        }
        let Some(tab) = self.tabs.get_mut(&key) else {
            return;
        };
        if tab.in_sight != in_sight {
            tab.in_sight = in_sight;
            if !tab.gone {
                ask(&Request::InSight { key, in_sight });
            }
        }
    }

    /// A chat's tab was closed.
    pub fn closed(&mut self, window: &str) {
        let Some(key) = key_of(window) else {
            return;
        };
        let Some(tab) = self.tabs.remove(&key) else {
            return;
        };
        self.claude_drafts.remove(&key);
        if tab.gone {
            return;
        }
        if tab.in_sight {
            ask(&Request::InSight {
                key: key.clone(),
                in_sight: false,
            });
        }
        ask(&Request::CloseChat { key });
    }

    /// The answer to a question of the tabs (Delete?); false — not theirs.
    pub fn dialog_answered(&mut self, id: u64, answer: Option<u32>) -> bool {
        if let Some((_, url)) = self.link_asked.take_if(|(asked, _)| *asked == id) {
            if answer == Some(0) {
                let _ = system::open_url(&url);
            }
            return true;
        }
        // Links (part 5): Open or Copy; Join; Add.
        if let Some((_, url)) = self.web_asked.take_if(|(asked, _)| *asked == id) {
            match answer {
                Some(0) => {
                    let _ = system::open_url(&url);
                }
                Some(1) => {
                    system::copy_text(&url);
                    notify::info(&tr("Link copied"));
                }
                _ => {}
            }
            return true;
        }
        if let Some((_, key, link)) = self.invite_asked.take_if(|(asked, _, _)| *asked == id) {
            if answer == Some(0) {
                ask(&Request::Link(LinkAsk::Join { key, link }));
            }
            return true;
        }
        if let Some((_, key, set)) = self.stickers_asked.take_if(|(asked, _, _)| *asked == id) {
            if answer == Some(0) {
                ask(&Request::Link(LinkAsk::AddStickers { key, set }));
            }
            return true;
        }
        if let Some((_, key)) = self.stop_asked.take_if(|(asked, _)| *asked == id) {
            if answer == Some(0) {
                ask(&Request::Block { key, blocked: true });
            }
            return true;
        }
        if let Some((_, key, message)) = self.unpin_asked.take_if(|(asked, _, _)| *asked == id) {
            if answer == Some(0) {
                ask(&Request::Unpin { key, message });
            }
            return true;
        }
        if let Some((_, key, message)) = self.stop_poll_asked.take_if(|(asked, _, _)| *asked == id)
        {
            if answer == Some(0) {
                ask(&Request::StopPoll { key, message });
            }
            return true;
        }
        // Selected messages' Delete, only for the user (part 5).
        if let Some((_, key, messages)) = self.delete_many.take_if(|(asked, _, _)| *asked == id) {
            if answer == Some(0) {
                ask(&Request::DeleteMany {
                    key,
                    messages,
                    revoke: false,
                });
            }
            return true;
        }
        if self.asked.as_ref().is_none_or(|(asked, _, _)| *asked != id) {
            return false;
        }
        let Some((_, key, message)) = self.asked.take() else {
            return false;
        };
        if answer == Some(0) {
            ask(&Request::Delete { key, message });
        }
        true
    }

    /// A timer of the tabs fired (one they started); false — not theirs. The tabs' timers are
    /// the searches' typing: the query goes.
    pub fn timer(&mut self, timer: u64) -> bool {
        let key = self.tabs.iter_mut().find_map(|(key, tab)| {
            let find = tab.find.as_mut()?;
            find.timer.take_if(|mine| *mine == timer)?;
            Some(key.clone())
        });
        let Some(key) = key else {
            return false;
        };
        self.find_now(&key);
        true
    }

    // --- The search and the menu ---

    /// 🔍, ⌘F, ⋯ → Search: the bar under the header, the keyboard in its field (again on ⌘F
    /// when it is open).
    fn open_find(&mut self, key: &ChatKey) {
        let Some(tab) = self.tabs.get_mut(key) else {
            return;
        };
        let window = tab.window.clone();
        if tab.find.is_none() {
            tab.find = Some(Find::default());
            self.render(key);
        }
        flux_plugin_api::view::focus(&window, FIND_FIELD);
    }

    /// × of the bar: the marks go from the messages.
    fn close_find(&mut self, key: &ChatKey) {
        let Some(tab) = self.tabs.get_mut(key) else {
            return;
        };
        let Some(find) = tab.find.take() else {
            return;
        };
        if let Some(timer) = find.timer {
            timers::cancel(timer);
        }
        let mut back: Vec<Message> = find
            .lit
            .iter()
            .filter_map(|id| tab.messages.get(id).cloned())
            .collect();
        if !back.is_empty() {
            back.sort_by_key(|message| message.order);
            conv::update(&tab.window, FEED, vec![conv::put(back)]);
        }
        self.render(key);
    }

    /// The bar's field changed: the query goes once typing stops (an empty one — at once).
    fn find_typed(&mut self, key: &ChatKey, text: String) {
        let Some(find) = self.tabs.get_mut(key).and_then(|tab| tab.find.as_mut()) else {
            return;
        };
        if find.text == text {
            return;
        }
        find.text = text;
        if let Some(timer) = find.timer.take() {
            timers::cancel(timer);
        }
        if find.text.trim().is_empty() {
            self.find_now(key);
        } else {
            find.timer = Some(timers::after(FIND_AFTER));
        }
    }

    /// The field's text is the query: asked of the service, its words marked.
    fn find_now(&mut self, key: &ChatKey) {
        let request = self.requests + 1;
        let Some(find) = self.tabs.get_mut(key).and_then(|tab| tab.find.as_mut()) else {
            return;
        };
        if let Some(timer) = find.timer.take() {
            timers::cancel(timer);
        }
        let query = find.text.trim().to_string();
        if query == find.query {
            return;
        }
        *find = Find {
            text: std::mem::take(&mut find.text),
            query: query.clone(),
            request,
            words: search_words(&query),
            lit: std::mem::take(&mut find.lit),
            ..Find::default()
        };
        self.requests = request;
        if !query.is_empty() {
            ask(&Request::Search(SearchAsk::Find {
                key: key.clone(),
                query,
                request,
                from: 0,
            }));
        }
        self.relight(key);
        self.render(key);
    }

    /// ↑ (older) or ↓ (newer): the next match, and the page after the last one loaded once it is
    /// reached.
    fn find_step(&mut self, key: &ChatKey, older: bool) {
        let Some(find) = self.tabs.get_mut(key).and_then(|tab| tab.find.as_mut()) else {
            return;
        };
        if find.found.is_empty() {
            return;
        }
        let index = match (older, find.current) {
            (true, None) => 0,
            (true, Some(index)) if index + 1 < find.found.len() => index + 1,
            (true, Some(_)) => {
                // Past the last loaded: its page's first, once it comes.
                find.step = find.paging;
                return;
            }
            (false, Some(index)) if index > 0 => index - 1,
            (false, _) => return,
        };
        find.current = Some(index);
        let message = find.found[index].clone();
        if index + 1 == find.found.len() && find.next != 0 && !find.paging {
            find.paging = true;
            ask(&Request::Search(SearchAsk::Find {
                key: key.clone(),
                query: find.query.clone(),
                request: find.request,
                from: find.next,
            }));
        }
        self.reveal(key, &message);
        self.render(key);
    }

    /// Scrolls to a match: here when it is loaded, else the service loads the feed around it.
    fn reveal(&self, key: &ChatKey, message: &str) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        if tab.messages.contains_key(message) {
            conv::update(&tab.window, FEED, vec![conv::reveal(message)]);
        } else {
            ask(&Request::JumpTo {
                key: key.clone(),
                message: message.to_string(),
            });
        }
    }

    /// The search's words changed: the messages whose marks changed are put again.
    fn relight(&mut self, key: &ChatKey) {
        let Some(tab) = self.tabs.get_mut(key) else {
            return;
        };
        let Some(find) = tab.find.as_mut() else {
            return;
        };
        let mut put = Vec::new();
        for (id, message) in &tab.messages {
            match highlighted(message, &find.words) {
                Some(marked) => {
                    find.lit.insert(id.clone());
                    put.push(marked);
                }
                None if find.lit.remove(id) => put.push(message.clone()),
                None => {}
            }
        }
        if !put.is_empty() {
            put.sort_by_key(|message| message.order);
            conv::update(&tab.window, FEED, vec![conv::put(put)]);
        }
    }

    /// ⋯: the chat's menu under the button.
    fn open_menu(&mut self, key: &ChatKey) {
        let Some(tab) = self.tabs.get(key) else {
            return;
        };
        let muted = tab.header.as_ref().is_some_and(|header| header.muted);
        let mute = if muted {
            picker::item("unmute", &tr("Unmute")).icon("bell")
        } else {
            picker::item("mute", &tr("Mute")).icon("icons/muted.svg")
        };
        let hide = if tab.hidden {
            picker::item("unhide", &tr("Unhide Chat")).icon("icons/eye.svg")
        } else {
            picker::item("hide", &tr("Hide Chat")).icon("icons/hidden.svg")
        };
        let read = if tab.header.as_ref().is_some_and(|header| header.unread) {
            picker::item("read", &tr("Mark as Read")).icon("check")
        } else {
            picker::item("unread", &tr("Mark as Unread")).icon("icons/unread.svg")
        };
        // A chat a link opened that isn't in the list (part 5): Add to FluxGram.
        let listed = tab.hidden
            || self
                .listed
                .get(&key.account)
                .is_none_or(|chats| chats.contains(&key.chat));
        let mut items = vec![
            picker::item("search", &tr("Search")).icon("search"),
            picker::item("profile", &tr("Profile")).icon("info"),
            picker::item("media", &tr("Media")).icon("icons/gallery.svg"),
            read,
            mute,
        ];
        // Claude (part 6): a chat of the list, never a hidden one; ✓ while it is on.
        if for_claude(tab.hidden, self.listed.get(&key.account), key.chat) {
            let icon = if self.claude_on(key) {
                "check"
            } else {
                "claude"
            };
            items.push(picker::item("claude", &tr("Available to Claude")).icon(icon));
        }
        // The folders are plaintext: a hidden chat never goes into one.
        if !tab.hidden {
            items.push(picker::item("folder", &tr("Add to Folder…")).icon("folder-plus"));
        }
        // A bot (part 5): Stop Bot, or Restart Bot once stopped.
        match tab.header.as_ref().filter(|header| header.bot) {
            Some(header) if header.blocked => {
                items.push(picker::item("restart-bot", &tr("Restart Bot")).icon("update"))
            }
            Some(_) => items.push(picker::item("stop-bot", &tr("Stop Bot")).icon("close")),
            None => {}
        }
        items.push(hide);
        items.push(match listed {
            true => picker::item("remove", &tr("Remove from FluxGram")).icon("close"),
            false => picker::item("add", &tr("Add to FluxGram")).icon("plus"),
        });
        let id = picker::Picker::menu()
            .items(items)
            .under(&tab.window, MORE)
            .open();
        self.menu = Some((id, key.clone()));
    }

    /// The service started again: it knows no open chats — they are asked for again.
    pub fn service_started(&mut self) {
        for (key, tab) in &self.tabs {
            if tab.gone {
                continue;
            }
            ask(&Request::OpenChat { key: key.clone() });
            if tab.in_sight {
                ask(&Request::InSight {
                    key: key.clone(),
                    in_sight: true,
                });
            }
        }
    }

    /// The accounts changed: the tabs of one that is gone say so (and stay, for the user to
    /// close).
    pub fn accounts(&mut self, accounts: &[AccountView]) {
        let keys: Vec<String> = accounts.iter().map(|account| account.key.clone()).collect();
        let names = if accounts.len() > 1 {
            account_labels(accounts)
        } else {
            HashMap::new()
        };
        let renamed = names != self.names;
        self.names = names;
        let mut changed = Vec::new();
        for (key, tab) in self.tabs.iter_mut() {
            let gone = !keys.contains(&key.account);
            if gone != tab.gone {
                tab.gone = gone;
                if gone {
                    tab.messages.clear();
                }
                changed.push(key.clone());
            } else if renamed {
                changed.push(key.clone());
            }
        }
        self.accounts = Some(keys);
        for key in changed {
            self.render(&key);
            let back = self.tabs.get(&key).is_some_and(|tab| !tab.gone);
            if back {
                ask(&Request::OpenChat { key });
            }
        }
    }

    /// The tab's view: the header over the conversation (its field when the user may write).
    fn render(&mut self, key: &ChatKey) {
        let Some(tab) = self.tabs.get_mut(key) else {
            return;
        };
        let header = tab.header.as_ref();
        let title = header.map_or(tab.title.as_str(), |header| header.title.as_str());
        let avatar_spec = header.map_or_else(
            || AvatarSpec::initials(&initials(title), 0),
            |header| header.avatar.clone(),
        );
        let status = match header {
            Some(header) if header.typing.is_some() => {
                Some(span(header.typing.as_deref().unwrap_or_default()).tone(Tone::Accent))
            }
            Some(header) if !header.status.is_empty() => {
                let tone = if header.online {
                    Tone::Accent
                } else {
                    Tone::Dim
                };
                Some(span(&header.status).tone(tone))
            }
            _ => None,
        };
        // With two accounts or more: whose chat it is, after the line.
        let account = self.names.get(&key.account).map(|name| {
            let name = match status {
                Some(_) => format!(" · {name}"),
                None => name.clone(),
            };
            span(&name).tone(Tone::Dim)
        });
        // A click on the header opens the chat's profile (`TabGo::Profile`); 🔍 and ⋯ beside it.
        let top: Element = flux_plugin_api::view::header(HEADER, title)
            .avatar(avatar_spec)
            .subtitle(status.into_iter().chain(account))
            .into();
        let mut children = if tab.gone {
            vec![top]
        } else {
            vec![row(
                "top",
                [
                    top,
                    spacer("top-gap"),
                    icon_button(FIND, "search", &tr("Search")),
                    icon_button(MORE, "more", &tr("More")),
                ],
            )]
        };
        if let Some(find) = tab.find.as_ref().filter(|_| !tab.gone) {
            let older = find
                .current
                .is_none_or(|index| index + 1 < find.found.len() || find.next != 0);
            let newer = find.current.is_some_and(|index| index > 0);
            children.push(row(
                "find-bar",
                [
                    text_field(FIND_FIELD, &find.text, Some(&tr("Search in chat"))),
                    text(FIND_COUNT, [span(&find.count()).tone(Tone::Dim)]),
                    step_button(
                        FIND_OLDER,
                        "arrow-up",
                        &tr("Older"),
                        !find.found.is_empty() && older,
                    ),
                    step_button(FIND_NEWER, "arrow-down", &tr("Newer"), newer),
                    icon_button(FIND_CLOSE, "close", &tr("Close the search")),
                ],
            ));
        }
        if tab.gone {
            children.push(text(
                "gone",
                [span(&tr("Signed out of this account.")).tone(Tone::Dim)],
            ));
        } else {
            // A group or a channel the user isn't in (opened by a link, part 5): Join in place of
            // the field.
            let outside = header.filter(|header| !header.joined);
            let can_send = header.is_some_and(|header| header.can_send) && outside.is_none();
            let saved = header.is_some_and(|header| header.saved);
            // Reply with Claude (part 6): where ⋯ has Available to Claude.
            let claude = for_claude(tab.hidden, self.listed.get(&key.account), key.chat);
            let mut spec = conv::spec()
                .empty_text(&tr("No messages here yet"))
                .message_action(REPLY, &tr("Reply"), Some("arrow-left"));
            // Its icon on the hover bar after Reply's; every message offers it but the chat's
            // own lines.
            if claude {
                spec = spec.message_action(REPLY_CLAUDE, &tr("Reply with Claude"), Some("claude"));
            }
            spec = spec
                .message_action(EDIT, &tr("Edit"), Some("pencil"))
                .message_action(DELETE, &tr("Delete"), Some("trash"))
                .message_action(DELETE_FOR_ME, &tr("Delete"), Some("trash"))
                // Part 5: in the menu only — the hover bar keeps its three.
                .message_action(PIN, &tr("Pin"), None)
                .message_action(UNPIN, &tr("Unpin"), None)
                .message_action(COPY_LINK, &tr("Copy Link"), None)
                .message_action(FORWARD, &tr("Forward"), None)
                .message_action(FAVE_STICKER, &tr("Add to Favorites"), None)
                .message_action(UNFAVE_STICKER, &tr("Remove from Favorites"), None)
                .message_action(SAVE_GIF, &tr("Save GIF"), None)
                // A poll's (part 5).
                .message_action(RETRACT_VOTE, &tr("Retract Vote"), None)
                .message_action(STOP_POLL, &tr("Stop Poll"), None)
                // An advertisement's (part 5).
                .message_action(ABOUT_AD, &tr("About the Ad"), None)
                .message_action(REPORT_AD, &tr("Report"), None)
                // Selecting messages (part 5): the menu's Select, the bar's actions.
                .selection_action(FORWARD, &tr("Forward"), true)
                .selection_action(DELETE_SELECTED, &tr("Delete"), false)
                // Every link comes here: Telegram's open in FluxGram, the web's are asked first
                // as the settings say (part 5, `links.rs`).
                .own_links()
                // The reactions the user may put here (part 5): ☺ and the chips.
                .reactions(
                    header
                        .map(|header| header.reactions.clone())
                        .unwrap_or_default(),
                );
            if can_send {
                // Formatting wraps the selection in Telegram's Markdown (shown as it is typed);
                // sending parses it. The panel: Flux's emoji, the account's stickers and GIFs.
                // "@" suggests the members, "/" the bots' commands; Send's menu sends without
                // sound, but in Saved Messages (part 5).
                let mut composer = conv::composer(&tr("Message…"))
                    .attachments()
                    .formatting()
                    .panel([PanelTab::Emoji, PanelTab::Stickers, PanelTab::Gifs])
                    .triggers(["@", "/"]);
                if !saved {
                    composer = composer.send_option(SILENT, &tr("Send Without Sound"));
                }
                // A bot's menu of commands (part 5).
                if header.is_some_and(|header| !header.commands.is_empty()) {
                    composer = composer.action(BOT_MENU, &tr("Menu"), Some("slash"));
                }
                if claude {
                    composer =
                        composer.action(REPLY_CLAUDE, &tr("Reply with Claude"), Some("claude"));
                }
                spec = spec.composer(composer);
            }
            // A bot (part 5): stopped — Unblock; no messages yet — Start, in place of the field.
            match header.filter(|header| header.bot) {
                Some(header) if header.blocked => {
                    spec = spec.field_button(UNBLOCK_BOT, &tr("Unblock"))
                }
                Some(header) if header.empty => spec = spec.field_button(START_BOT, &tr("Start")),
                _ => {}
            }
            // A channel the user can't post in (part 5): Join, or Mute / Unmute, in place of the
            // field.
            let channel = header.filter(|header| (header.channel || !header.joined) && !can_send);
            if let Some(header) = channel {
                spec = match (header.joined, header.muted) {
                    (false, _) => spec.field_button(JOIN, &tr("Join")),
                    (true, false) => spec.field_button(MUTE_CHANNEL, &tr("Mute")),
                    (true, true) => spec.field_button(UNMUTE_CHANNEL, &tr("Unmute")),
                };
            }
            // In place of the field where the user can't write — not before the header comes:
            // the chat may well take messages.
            if header.is_some() && !can_send && channel.is_none() {
                spec = spec.field_note(&tr("You can't write in this chat"));
            }
            children.push(conversation(FEED, spec));
        }
        let view = column("root", children).into_view();
        if tab.view.as_ref() == Some(&view) {
            return;
        }
        ui::set_view(&tab.window, &view);
        tab.view = Some(view);
    }
}

fn ask(request: &Request) {
    let _ = service::send_json(request);
}

/// A number with its digits in groups of three, a no-break space between: "12 345".
fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('\u{a0}');
        }
        grouped.push(digit);
    }
    grouped
}

/// Mute's choice: the seconds (Telegram's "for good" is its largest), the label.
pub fn mute_choices() -> [(i64, &'static str); 4] {
    [
        (3600, "For 1 Hour"),
        (8 * 3600, "For 8 Hours"),
        (2 * 86_400, "For 2 Days"),
        (crate::profile::FOREVER, "Forever"),
    ]
}

/// An icon button of the search bar, grayed out when there is no match that way.
fn step_button(id: &str, icon: &str, tooltip: &str, enabled: bool) -> Element {
    Element::raw(
        id,
        ElementKind::Button(ButtonSpec {
            label: None,
            icon: Some(icon.to_string()),
            tooltip: Some(tooltip.to_string()),
            primary: false,
            enabled,
            busy: false,
            badge: None,
        }),
    )
}

/// The selected messages Telegram lets forward, in their order, and how many it doesn't (part
/// 5).
fn forwardable(messages: &[String], found: &HashMap<String, Message>) -> (Vec<String>, usize) {
    let can: Vec<String> = messages
        .iter()
        .filter(|id| found.get(*id).is_some_and(|m| offers(m, FORWARD)))
        .cloned()
        .collect();
    let cannot = messages.len() - can.len();
    (can, cannot)
}

/// The selected messages that can be deleted, in their order, and whether every one of them can
/// go for everyone (part 5) — a message's own Delete decides as much.
fn deletable(messages: &[String], found: &HashMap<String, Message>) -> (Vec<String>, bool) {
    let mut for_all = true;
    let can: Vec<String> = messages
        .iter()
        .filter(|id| {
            let Some(message) = found.get(*id) else {
                return false;
            };
            let everyone = offers(message, DELETE) && message.actions.is_some();
            for_all &= everyone;
            everyone || offers(message, DELETE_FOR_ME) && message.actions.is_some()
        })
        .cloned()
        .collect();
    (can, for_all)
}

/// The forwarding bar's quote (part 5): "Forwarding 3 messages" over the authors, in their order,
/// and the first message's line — "Anna, Boris: Hi…".
fn forward_quote(messages: &[&Message], tab: &Tab, words: &dyn Words) -> MessageQuote {
    let tr = |text: &str| words.tr(text);
    let mut authors: Vec<String> = Vec::new();
    for message in messages {
        let author = quote_of(message, tab, &tr).author;
        if !authors.contains(&author) {
            authors.push(author);
        }
    }
    let line = messages
        .first()
        .map(|first| quote_of(first, tab, &tr).text)
        .unwrap_or_default();
    let title = if messages.len() == 1 {
        words.tr("Forwarding a message")
    } else {
        words.count(
            messages.len() as u64,
            "Forwarding {0} message",
            "Forwarding {0} messages",
        )
    };
    MessageQuote {
        message: None,
        author: title,
        color: 0,
        text: format!("{}: {line}", authors.join(", ")),
    }
}

/// The message offers the action (no list — every one).
fn offers(message: &Message, action: &str) -> bool {
    message
        .actions
        .as_ref()
        .is_none_or(|actions| actions.iter().any(|offered| offered == action))
}

/// The quote over the field for a reply: the author ("You" for the user's own, the person in a
/// one-to-one chat) and a line of the message.
/// Claude may be given the chat (part 6: Available to Claude, Reply with Claude): one of the
/// account's list (`listed`; not known yet — it is), never a hidden one nor one a link opened.
fn for_claude(hidden: bool, listed: Option<&HashSet<i64>>, chat: i64) -> bool {
    !hidden && listed.is_none_or(|chats| chats.contains(&chat))
}

fn quote_of(message: &Message, tab: &Tab, tr: &dyn Fn(&str) -> String) -> MessageQuote {
    let (author, color) = match (&message.author, message.outgoing) {
        (_, true) => (tr("You"), 0),
        (Some(author), false) => (author.name.clone(), author.color),
        (None, false) => (
            tab.header
                .as_ref()
                .map_or_else(|| tab.title.clone(), |header| header.title.clone()),
            tab.header.as_ref().map_or(0, |header| header.avatar.color),
        ),
    };
    let line = message
        .parts
        .iter()
        .find_map(|part| match part {
            Part::Text(spans) | Part::Quote(spans) | Part::Notice(spans) => {
                let text: String = spans.iter().map(|span| span.text.as_str()).collect();
                (!text.trim().is_empty()).then_some(text)
            }
            Part::Code(code) => Some(code.text.clone()),
            Part::Placeholder(placeholder) => Some(placeholder.text.clone()),
            // "📊 Poll: …", as the list's line says it (part 5).
            Part::Poll(poll) => {
                let question: String = poll
                    .question
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect();
                let quiz = poll.options.iter().any(|option| option.correct)
                    || [tr("Quiz"), tr("Anonymous quiz")].contains(&poll.kind);
                let template = tr(if quiz { "Quiz: {0}" } else { "Poll: {0}" });
                Some(format!("📊 {}", template.replace("{0}", &question)))
            }
            _ => None,
        })
        .or_else(|| {
            message.parts.first().map(|part| match part {
                Part::Image(spec) if spec.play && spec.label.as_deref() == Some("GIF") => tr("GIF"),
                Part::Image(spec) if spec.play => tr("Video"),
                Part::Image(_) => tr("Photo"),
                Part::Sticker(_) => tr("Sticker"),
                Part::Voice(_) => tr("Voice message"),
                Part::Video(video) => match video.kind {
                    VideoKind::Video => tr("Video"),
                    VideoKind::Gif => tr("GIF"),
                    VideoKind::Round => tr("Video message"),
                },
                Part::File(file) => file.name.clone(),
                Part::Link(link) => link.url.clone(),
                _ => String::new(),
            })
        })
        .unwrap_or_default();
    MessageQuote {
        message: Some(message.id.clone()),
        author,
        color,
        text: crate::summary::first_line(&line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_plugin_api::conversation::{AvatarSpec, Delivery, code, rich};

    #[test]
    fn names_tabs() {
        assert!(is_tab("chat/a1:-100"));
        assert!(!is_tab("chats"));
        assert!(!is_tab("fluxgram"));
        assert_eq!(key_of("chat/a1:-100"), Some(ChatKey::new("a1", -100)));
        assert_eq!(key_of("chat/a1"), None);
    }

    #[test]
    fn counts_group_their_digits() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(412), "412");
        assert_eq!(grouped(1234), "1\u{a0}234");
        assert_eq!(grouped(1_234_567), "1\u{a0}234\u{a0}567");
    }

    fn tab() -> Tab {
        Tab {
            window: "chat/a1:101".into(),
            title: "Anna Smirnova".into(),
            header: None,
            messages: HashMap::new(),
            gone: false,
            in_sight: true,
            hidden: false,
            find: None,
            reply: None,
            view: None,
            ad: None,
        }
    }

    #[test]
    fn quotes_and_edits_messages() {
        let tab = tab();
        let quote_of = |message: &Message, tab: &Tab| quote_of(message, tab, &|t: &str| t.into());
        let theirs = Message::new("5", 5, 0).text("Hi there\nsecond line");
        let quote = quote_of(&theirs, &tab);
        assert_eq!(
            (quote.author.as_str(), quote.text.as_str()),
            ("Anna Smirnova", "Hi there")
        );
        let photo = Message::new("6", 6, 0)
            .author("Petya", 0, Some(AvatarSpec::initials("P", 0)))
            .part(conv::image(conv::pending_picture(10, 10)));
        assert_eq!(quote_of(&photo, &tab).text, "Photo");
        // A poll, a closed quiz (its right answer marked): what they are and the question.
        let poll = conv::poll_spec(vec![rich("Lunch?")], "Anonymous poll", Vec::new(), "");
        let asked = Message::new("8", 8, 0).part(conv::poll(poll.clone()));
        assert_eq!(quote_of(&asked, &tab).text, "📊 Poll: Lunch?");
        let mut quiz = poll.closed();
        quiz.options = vec![conv::poll_option(vec![rich("Pho")], 100).correct()];
        let asked = Message::new("9", 9, 0).part(conv::poll(quiz));
        assert_eq!(quote_of(&asked, &tab).text, "📊 Quiz: Lunch?");
        let mine = Message::new("7", 7, 0)
            .outgoing(Delivery::Read)
            .part(conv::text(vec![rich("Here: "), rich("fix").bold()]))
            .part(code("fn main() {}", Some("rust")))
            .actions([REPLY, EDIT]);
        assert_eq!(quote_of(&mine, &tab).author, "You");
        // Editing takes the message as Markdown: the bold stays bold once saved.
        assert!(crate::convert::markdown(&mine).starts_with("Here: **fix**"));
        assert!(offers(&mine, EDIT) && !offers(&mine, DELETE));
        assert!(offers(&theirs, DELETE), "no list: every action");
    }

    #[test]
    fn plans_forwarding_and_deleting_selected_messages() {
        let mut tab = tab();
        let put = |tab: &mut Tab, message: Message| {
            tab.messages.insert(message.id.clone(), message);
        };
        put(
            &mut tab,
            Message::new("1", 1, 0)
                .text("Hi there")
                .actions([REPLY, FORWARD]),
        );
        put(
            &mut tab,
            Message::new("2", 2, 0)
                .author("Boris", 2, None)
                .text("Protected")
                .actions([REPLY]),
        );
        put(
            &mut tab,
            Message::new("3", 3, 0)
                .outgoing(Delivery::Read)
                .text("Mine")
                .actions([REPLY, FORWARD, DELETE]),
        );
        put(
            &mut tab,
            Message::new("4", 4, 0)
                .outgoing(Delivery::Read)
                .text("Old")
                .actions([FORWARD, DELETE_FOR_ME]),
        );
        let ids = |list: &[&str]| list.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        // The one Telegram won't let go is counted, the rest keep their order.
        let (can, cannot) = forwardable(&ids(&["1", "2", "3"]), &tab.messages);
        assert_eq!((can, cannot), (ids(&["1", "3"]), 1));
        // Deleting: for everyone only when each one can; the others' messages stay.
        assert_eq!(deletable(&ids(&["3"]), &tab.messages), (ids(&["3"]), true));
        assert_eq!(
            deletable(&ids(&["1", "3", "4"]), &tab.messages),
            (ids(&["3", "4"]), false)
        );
        // The bar: the count over the authors and the first message's line.
        let found: Vec<&Message> = ["1", "2", "3"]
            .iter()
            .map(|id| &tab.messages[*id])
            .collect();
        let quote = forward_quote(&found, &tab, &crate::model::English);
        assert_eq!(quote.author, "Forwarding 3 messages");
        assert_eq!(quote.text, "Anna Smirnova, Boris, You: Hi there");
        let quote = forward_quote(&found[..1], &tab, &crate::model::English);
        assert_eq!(
            (quote.author.as_str(), quote.text.as_str()),
            ("Forwarding a message", "Anna Smirnova: Hi there")
        );
    }

    #[test]
    fn claude_is_offered_for_the_list_s_chats_only() {
        let listed: HashSet<i64> = [101, -5].into();
        assert!(for_claude(false, Some(&listed), 101));
        // The list not known yet: offered.
        assert!(for_claude(false, None, 101));
        // A hidden chat, one a link opened: never.
        assert!(!for_claude(true, Some(&listed), 101));
        assert!(!for_claude(true, None, 101));
        assert!(!for_claude(false, Some(&listed), 202));
    }
}
