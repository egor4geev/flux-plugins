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
//! A tab Flux brings back with the project comes as `tool-window-shown` of a tab this instance
//! doesn't know: it is filled again from its key. A tab whose account is gone stays, and says so.
//!
//! A scenario reaches a tab through `ui:<plugin>/chat/<key>/<element>`: `open` opens it, `reply/<id>`,
//! `edit/<id>`, `delete/<id>` are a message's actions, `ui-submit:…/send=<text>` sends a text and
//! `…/draft=<text>` types one; `find` opens the search, `ui-submit:…/find-field=<text>` looks for
//! a text at once, `find-older` / `find-newer` / `find-close` are the bar's buttons, `more` the
//! menu and `menu/<item>` its items (`menu/hide`).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use flux_plugin_api::UiEvent;
use flux_plugin_api::conversation::{self as conv, Change, Composed, Message, MessageQuote, Part};
use flux_plugin_api::dialog::Ask;
use flux_plugin_api::host::ui::{self, ButtonSpec, ElementKind, View};
use flux_plugin_api::view::*;
use flux_plugin_api::{picker, service, system, timers, tr, trf};

use crate::convert::{
    COMMAND_LINK, DELETE, DELETE_FOR_ME, EDIT, REPLY, USER_LINK, highlighted, search_words,
};
use crate::feed::{SearchAsk, SearchTold};
use crate::model::{AccountView, ChatHeader, ChatKey, Request, account_labels, initials};

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

/// What a chat's tab asks the window for (part 3+): the profile dialog's parts.
pub enum TabGo {
    /// The chat's profile (the header's click, ⋯ → Profile).
    Profile(ChatKey),
    /// The chat's media (⋯ → Media).
    Media(ChatKey),
    /// Mute or unmute the chat (⋯ → Mute).
    Mute(ChatKey, bool),
    /// Hide the chat (⋯ → Hide Chat): the list knows how (the phrase).
    Hide(ChatKey),
    /// Show a hidden chat in the list again (⋯ → Unhide).
    Unhide(ChatKey),
    /// The folders' picker for the chat (⋯ → Add to Folder…): the list's.
    Folder(ChatKey),
}

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
    /// The view set last: an unchanged one isn't set again.
    view: Option<View>,
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
    /// The ⋯ menu on show: the picker's id, the chat.
    menu: Option<(u64, ChatKey)>,
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
    }

    /// The ⋯ menu closed: its choice, or none.
    pub fn picker_chosen(&mut self, picker: u64, choice: Option<String>) {
        let Some((_, key)) = self.menu.take_if(|(id, _)| *id == picker) else {
            return;
        };
        if let Some(choice) = choice {
            self.choose(key, &choice);
        }
    }

    /// An item of the ⋯ menu (a scenario's `menu/<item>` too).
    fn choose(&mut self, key: ChatKey, choice: &str) {
        match choice {
            "search" => self.open_find(&key),
            "profile" => self.go.push(TabGo::Profile(key)),
            "media" => self.go.push(TabGo::Media(key)),
            "mute" => self.go.push(TabGo::Mute(key, true)),
            "unmute" => self.go.push(TabGo::Mute(key, false)),
            "hide" => self.go.push(TabGo::Hide(key)),
            "unhide" => self.go.push(TabGo::Unhide(key)),
            "folder" => self.go.push(TabGo::Folder(key)),
            "remove" => ask(&Request::RemoveChat { key }),
            _ => {}
        }
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
                view: None,
            },
        );
        self.render(key);
        if !gone {
            ask(&Request::OpenChat { key: key.clone() });
        }
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
        for change in &mut changes {
            match change {
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
            (FEED, UiEvent::Composed(composed)) => {
                if !composed.text.trim().is_empty() || !composed.files.is_empty() {
                    ask(&Request::Send { key, composed });
                }
                conv::update(&window, FEED, vec![conv::reply_to(None)]);
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
                conv::update(&window, FEED, vec![conv::reply_to(None)])
            }
            (FEED, UiEvent::PartClicked((message, part))) => {
                ask(&Request::Download { key, message, part })
            }
            (FEED, UiEvent::QuoteClicked((_, message))) => ask(&Request::JumpTo { key, message }),
            (FEED, UiEvent::LinkClicked((_, link))) => self.link(&key, &link),
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
            }),
            ("draft", UiEvent::Submitted(text)) => ask(&Request::Draft { key, text }),
            _ => {}
        }
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
            }
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
            _ => {}
        }
    }

    /// A link that isn't a web one: a person's mention opens their chat, a bot's command goes
    /// into the field, a phone number to the system.
    fn link(&mut self, key: &ChatKey, link: &str) {
        if let Some(user) = link
            .strip_prefix(USER_LINK)
            .and_then(|id| id.parse::<i64>().ok())
        {
            self.open(&ChatKey::new(&key.account, user), None);
        } else if let Some(command) = link.strip_prefix(COMMAND_LINK) {
            if let Some(tab) = self.tabs.get(key) {
                conv::update(&tab.window, FEED, vec![conv::draft(&format!("{command} "))]);
            }
        } else {
            let _ = system::open_url(link);
        }
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
        let mut items = vec![
            picker::item("search", &tr("Search")).icon("search"),
            picker::item("profile", &tr("Profile")).icon("info"),
            picker::item("media", &tr("Media")).icon("icons/gallery.svg"),
            mute,
        ];
        // The folders are plaintext: a hidden chat never goes into one.
        if !tab.hidden {
            items.push(picker::item("folder", &tr("Add to Folder…")).icon("folder-plus"));
        }
        items.push(hide);
        items.push(picker::item("remove", &tr("Remove from FluxGram")).icon("close"));
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
            let can_send = header.is_some_and(|header| header.can_send);
            let mut spec = conv::spec()
                .empty_text(&tr("No messages here yet"))
                .message_action(REPLY, &tr("Reply"), Some("arrow-left"))
                .message_action(EDIT, &tr("Edit"), Some("pencil"))
                .message_action(DELETE, &tr("Delete"), Some("trash"))
                .message_action(DELETE_FOR_ME, &tr("Delete"), Some("trash"));
            if can_send {
                // Formatting wraps the selection in Telegram's Markdown; sending parses it.
                spec = spec.composer(conv::composer(&tr("Message…")).attachments().formatting());
            }
            children.push(conversation(FEED, spec));
            // Before the header comes nothing is said: the chat may well take messages.
            if header.is_some() && !can_send {
                children.push(text(
                    "read-only",
                    [span(&tr("You can't write in this chat")).tone(Tone::Dim)],
                ));
            }
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

/// The message offers the action (no list — every one).
fn offers(message: &Message, action: &str) -> bool {
    message
        .actions
        .as_ref()
        .is_none_or(|actions| actions.iter().any(|offered| offered == action))
}

/// The quote over the field for a reply: the author ("You" for the user's own, the person in a
/// one-to-one chat) and a line of the message.
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
            _ => None,
        })
        .or_else(|| {
            message.parts.first().map(|part| match part {
                Part::Image(spec) if spec.play && spec.label.as_deref() == Some("GIF") => tr("GIF"),
                Part::Image(spec) if spec.play => tr("Video"),
                Part::Image(_) => tr("Photo"),
                Part::Sticker(_) => tr("Sticker"),
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
            view: None,
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
}
