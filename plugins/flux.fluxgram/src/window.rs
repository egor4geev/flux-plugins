//! A window's instance: draws what the service tells ([`Note`]s) and passes on what the user does
//! ([`Request`]). Its places: the FluxGram window on the left ([`List`]: the accounts and their
//! chats), a chat's tab ([`Tabs`]), the accounts' block in Settings → FluxGram, and the Add Account
//! dialog (a QR code, or the number, the code and the password). This module draws the last two
//! and routes the rest.
//!
//! A view is set only when it differs from the one set before, and the countdowns (the QR code's
//! refresh, the code's other way) tick only while their screen is on show: an idle window costs
//! nothing.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use flux_plugin_api::dialog::Ask;
use flux_plugin_api::host::ui::{self, View};
use flux_plugin_api::view::*;
use flux_plugin_api::{Event, UiEvent, service, timers, tr, trf};

use crate::chat::{self, TabGo, Tabs};
use crate::list::{Go, HIDDEN, HIDE, List};
use crate::model::{
    AccountView, ChatKey, CodeSent, Note, Request, Screen, SignIn, Snapshot, countdown, initials,
    unix_now,
};
use crate::profile_view::{PROFILE, ProfileGo, ProfileView, Section};

/// The window on the left.
pub const MAIN: &str = "fluxgram";
/// The block of Settings → FluxGram.
pub const ACCOUNTS: &str = "accounts";
/// The Add Account dialog.
pub const SIGN_IN: &str = "sign-in";
/// How long a QR code lives before TDLib replaces it (Telegram's login tokens: 30 s).
const QR_LIFETIME: u64 = 30;

pub struct Window {
    state: Snapshot,
    /// The window on the left.
    list: List,
    /// The chats' tabs.
    tabs: Tabs,
    /// The profile dialog.
    profile: ProfileView,
    /// The dialog is open in this window.
    dialog: bool,
    /// What is typed into the dialog's fields, by field.
    typed: HashMap<&'static str, String>,
    /// The QR code on show, since when.
    link: Option<(String, Instant)>,
    /// The code screen counts down to its other way.
    counting: bool,
    /// The countdowns' timer.
    tick: Option<u64>,
    /// The Sign Out question, by its id: the account's key.
    asked: Option<(u64, String)>,
    /// The Clear Cache question (Settings → FluxGram → Cache), by its id.
    cache_asked: Option<u64>,
    /// The views set, by window: an unchanged one isn't set again.
    views: HashMap<&'static str, View>,
}

impl Window {
    pub fn new() -> Self {
        Window {
            state: Snapshot::default(),
            list: List::new(),
            tabs: Tabs::new(),
            profile: ProfileView::new(),
            dialog: false,
            typed: HashMap::new(),
            link: None,
            counting: false,
            tick: None,
            asked: None,
            cache_asked: None,
            views: HashMap::new(),
        }
    }

    pub fn activate(&mut self) {
        // The Settings block has its view before the page opens; the service's state follows.
        self.render();
        let _ = service::send_json(&Request::Hello);
    }

    /// A command: Add Telegram Account…, Add Chat…, `open-chat` (a notification's Open: the
    /// argument is the chat's tab key), and `clear-cache` (Settings' button, asked first).
    pub fn run_command(&mut self, command: &str, argument: Option<&str>) {
        match command {
            "add-account" => self.add_account(),
            "add-chat" => self.list.add_chat(),
            "clear-cache" => {
                let id = Ask::warning(&tr("Clear the cache?"))
                    .message(&tr("Media you've seen downloads again when you open it."))
                    .danger(&tr("Clear"))
                    .cancel(&tr("Cancel"))
                    .send();
                self.cache_asked = Some(id);
            }
            "open-chat" => {
                if let Some(key) = argument.and_then(ChatKey::parse) {
                    let title = self.list.title(&key);
                    self.tabs.open(&key, title.as_deref());
                    self.list.tab_opened(&key);
                }
            }
            _ => {}
        }
    }

    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::ServiceMessage((None, text)) => match service::parse::<Note>(&text) {
                Some(Note::State(state)) => self.update(state),
                Some(Note::Chats { account, rows }) => {
                    // The profile dialog offers Add to FluxGram or Remove by the list.
                    self.profile
                        .listed(&account, rows.iter().map(|row| row.chat));
                    // ⋯ of a tab a link opened offers Add to FluxGram by it too (part 5).
                    self.tabs.listed(&account, rows.iter().map(|row| row.chat));
                    self.list.chats(&account, rows)
                }
                Some(Note::Found { request, chats }) => self.list.found(request, chats),
                Some(Note::Header { key, header }) => self.tabs.header(&key, header),
                Some(Note::Feed { key, changes }) => self.tabs.feed(&key, changes),
                Some(Note::Profile(told)) => self.profile.told(told),
                Some(Note::Search(told)) => self.tabs.search_told(told),
                Some(Note::Hidden(told)) => {
                    let go = self.list.hidden_told(told);
                    self.go(go);
                    // The profile offers Show in List for the hidden chats open here (part 5).
                    self.profile.set_hidden(self.list.hidden_chats());
                }
                Some(Note::Folders { folders }) => self.list.folders(folders),
                Some(Note::Copied { link }) => self.tabs.copied(&link),
                Some(Note::Ad { key, ad }) => self.tabs.ad_told(&key, ad),
                Some(Note::AdReport {
                    key,
                    message,
                    title,
                    options,
                }) => self.tabs.ad_report(key, message, &title, options),
                // A bot's answer, its ask for a reply (part 5).
                Some(Note::BotAnswer {
                    key,
                    bot,
                    text,
                    alert,
                    url,
                }) => {
                    self.tabs.bot_answer(&key, &bot, &text, alert, &url);
                    self.tabs_go();
                }
                Some(Note::ForceReply { key, message }) => self.tabs.force_reply(&key, &message),
                // What a link opens (part 5).
                Some(Note::Link(told)) => {
                    self.tabs.link_told(told);
                    self.tabs_go();
                }
                None => {}
            },
            Event::ServiceStarted => {
                let _ = service::send_json(&Request::Hello);
                self.tabs.service_started();
                self.list.service_started();
            }
            Event::ToolWindowShown(window) if window == SIGN_IN => {
                self.dialog = true;
                self.render();
            }
            Event::ToolWindowHidden(window) if window == SIGN_IN => {
                self.dialog = false;
                self.sync_tick();
                // Esc or ×: the sign-in is dropped.
                if self.state.sign_in.is_some() {
                    self.ask(Request::Cancel);
                }
            }
            Event::ToolWindowShown(window) if chat::is_tab(&window) => {
                self.tabs.in_sight(&window, true)
            }
            Event::ToolWindowHidden(window) if chat::is_tab(&window) => {
                self.tabs.in_sight(&window, false)
            }
            Event::ToolWindowHidden(window) if window == HIDE => self.list.hide_dialog_closed(),
            Event::ToolWindowShown(window) if window == PROFILE => self.profile.shown(true),
            Event::ToolWindowHidden(window) if window == PROFILE => self.profile.shown(false),
            Event::ToolWindowShown(window) if window == MAIN => self.list.shown(true),
            Event::ToolWindowHidden(window) if window == MAIN => self.list.shown(false),
            Event::TabClosed(window) => self.tabs.closed(&window),
            Event::Timer(id) if Some(id) == self.tick => self.render_dialog(),
            Event::Timer(id) => {
                if !self.list.timer(id) && !self.profile.timer(id) {
                    self.tabs.timer(id);
                }
                self.tabs_go();
            }
            Event::DialogAnswered((id, answer)) if self.cache_asked == Some(id) => {
                self.cache_asked = None;
                if answer == Some(0) {
                    self.ask(Request::ClearCache);
                }
            }
            Event::DialogAnswered((id, answer))
                if self.asked.as_ref().is_some_and(|a| a.0 == id) =>
            {
                let key = self.asked.take().map(|(_, key)| key).unwrap_or_default();
                if answer == Some(0) {
                    self.ask(Request::SignOut { key });
                }
            }
            Event::DialogAnswered((id, answer)) => {
                if !self.tabs.dialog_answered(id, answer) {
                    self.list.dialog_answered(id, answer);
                }
            }
            // A folder's name (New Folder, Rename…): the list's.
            Event::TextAnswered((id, text)) => self.list.text_answered(id, text),
            // Pin's question (part 5): the tabs'.
            Event::FormAnswered((id, values)) => {
                self.tabs.form_answered(id, values);
            }
            // A tab's menu (⋯) is the tabs'; the Add Chat popup is the list's.
            Event::PickerQuery((picker, query)) if !self.tabs.owns_picker(picker) => {
                self.list.picker_query(picker, &query)
            }
            Event::PickerChosen((picker, key)) if self.tabs.owns_picker(picker) => {
                self.tabs.picker_chosen(picker, key);
                self.tabs_go();
            }
            Event::PickerChosen((picker, key)) => {
                let go = self.list.picker_chosen(picker, key);
                self.go(go);
            }
            Event::Ui(input) => match (input.window.as_str(), input.event) {
                (ACCOUNTS, UiEvent::Clicked) if input.element == "accounts-add" => {
                    self.add_account()
                }
                (ACCOUNTS, UiEvent::RowAction((key, action))) if action == "sign-out" => {
                    self.confirm_sign_out(&key)
                }
                (MAIN, event) => {
                    let go = self.list.ui(&input.element, event);
                    self.go(go);
                }
                (SIGN_IN, event) => self.dialog_input(&input.element, event),
                (PROFILE, event) => {
                    let go = self.profile.ui(&input.element, event);
                    self.profile_go(go);
                }
                (HIDDEN, event) => self.list.hidden_ui(&input.element, event),
                (HIDE, event) => self.list.hide_dialog_ui(&input.element, event),
                (window, event) if chat::is_tab(window) => {
                    self.tabs.ui(window, &input.element, event);
                    self.tabs_go();
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// What the chats' tabs asked the window for: the profile dialog's parts, hiding the chat.
    fn tabs_go(&mut self) {
        for go in self.tabs.take_go() {
            match go {
                TabGo::Profile(key) => self.profile.open(&key, Section::Info),
                TabGo::Media(key) => self.profile.open(&key, Section::Media),
                TabGo::Mute(key, muted) => self.profile.mute(&key, muted),
                TabGo::MuteFor(key, seconds) => self.profile.mute_for(&key, seconds),
                TabGo::Hide(key) => self.list.hide(&key),
                TabGo::Unhide(key) => self.list.unhide(&key),
                TabGo::Folder(key) => self.list.folder_menu(&key, true),
                TabGo::Forward(forward) => self.list.forward(forward),
                TabGo::Opened(key) => self.list.tab_opened(&key),
            }
        }
    }

    /// What the profile dialog asks the window for.
    fn profile_go(&mut self, go: Vec<ProfileGo>) {
        for go in go {
            match go {
                ProfileGo::Open(key) => {
                    let title = self.list.title(&key);
                    self.tabs.open(&key, title.as_deref());
                    self.list.tab_opened(&key);
                }
                ProfileGo::Unhide(key) => self.list.unhide(&key),
                // A link of the chat's media (part 5): Telegram's opens in FluxGram, over which
                // the dialog would stay.
                ProfileGo::Link(key, url) => {
                    if crate::links::is_telegram(&url) {
                        ui::hide(PROFILE);
                    }
                    self.tabs.follow(&key, &url, crate::links::Shown::Address);
                    self.tabs_go();
                }
            }
        }
    }

    /// What the list asks the window for.
    fn go(&mut self, go: Vec<Go>) {
        for go in go {
            match go {
                Go::Open(key) => {
                    let title = self.list.title(&key);
                    self.tabs.open(&key, title.as_deref());
                    self.list.tab_opened(&key);
                }
                // Part 5: the Forward picker's chat — its bar first, then the tab.
                Go::Forward(key, forward) => {
                    self.tabs.set_forward(&key, forward);
                    let title = self.list.title(&key);
                    self.tabs.open(&key, title.as_deref());
                    self.list.tab_opened(&key);
                }
                Go::OpenHidden(key) => {
                    let title = self.list.title(&key);
                    self.tabs.open_hidden(&key, title.as_deref());
                }
                Go::Close(key) => self.tabs.close(&key),
                Go::CloseHidden => self.tabs.close_hidden(),
                Go::MarkHidden(key, hidden) => self.tabs.set_hidden(&key, hidden),
                Go::AddAccount => self.add_account(),
                Go::Mute(key, Some(seconds)) => self.profile.mute_for(&key, seconds),
                Go::Mute(key, None) => self.profile.mute(&key, false),
                Go::StopBot(key) => self.tabs.stop_bot(key),
            }
        }
        // The hidden chats may have closed (the phrase left the search): the profile forgets them.
        self.profile.set_hidden(self.list.hidden_chats());
    }

    fn ask(&self, request: Request) {
        let _ = service::send_json(&request);
    }

    /// The service's new state: the views that changed are set again.
    fn update(&mut self, state: Snapshot) {
        let ended = self.state.sign_in.is_some() && state.sign_in.is_none();
        self.state = state;
        if ended && self.dialog {
            // Signed in (or cancelled elsewhere): the dialog closes.
            self.dialog = false;
            ui::hide(SIGN_IN);
        }
        self.list.state(&self.state);
        self.tabs.accounts(&self.state.accounts);
        self.render();
    }

    fn add_account(&mut self) {
        self.typed.clear();
        self.ask(Request::AddAccount);
        self.render_dialog();
        ui::show(SIGN_IN);
    }

    fn confirm_sign_out(&mut self, key: &str) {
        let Some(account) = self.state.accounts.iter().find(|a| a.key == key) else {
            return;
        };
        let id = Ask::warning(&trf("Sign out of {0}?", &[&account.name]))
            .message(&tr(
                "FluxGram forgets this account on this computer; it stays in Telegram.",
            ))
            .danger(&tr("Sign Out"))
            .cancel(&tr("Cancel"))
            .send();
        self.asked = Some((id, key.to_string()));
    }

    fn dialog_input(&mut self, element: &str, event: UiEvent) {
        match (element, event) {
            ("sign-in-phone", UiEvent::Clicked) => self.ask(Request::UsePhone),
            ("sign-in-use-qr", UiEvent::Clicked) => self.ask(Request::UseQr),
            ("sign-in-back", UiEvent::Clicked) => self.ask(Request::Back),
            ("sign-in-resend", UiEvent::Clicked) => self.ask(Request::Resend),
            ("sign-in-cancel", UiEvent::Clicked) => {
                self.ask(Request::Cancel);
                self.dialog = false;
                ui::hide(SIGN_IN);
            }
            ("sign-in-next", UiEvent::Clicked) => self.submit(),
            (field, UiEvent::Changed(text)) => self.type_into(field, text),
            // ↵ in a field: what it has, and on.
            (field, UiEvent::Submitted(text)) => {
                self.type_into(field, text);
                self.submit();
            }
            _ => {}
        }
    }

    fn type_into(&mut self, field: &str, text: String) {
        if let Some(field) = FIELDS.iter().find(|known| **known == field) {
            self.typed.insert(field, text);
        }
    }

    /// Next, or ↵: the field of the screen on show goes to the service.
    fn submit(&mut self) {
        let Some(sign_in) = &self.state.sign_in else {
            return;
        };
        let field = match sign_in.screen {
            Screen::Phone => PHONE,
            Screen::Code { .. } => CODE,
            Screen::Password { .. } => PASSWORD,
            Screen::EmailAddress => EMAIL,
            Screen::EmailCode { .. } => EMAIL_CODE,
            _ => return,
        };
        let text = self.typed.get(field).cloned().unwrap_or_default();
        self.ask(Request::Submit { text });
    }

    fn render(&mut self) {
        self.render_accounts();
        self.list.render();
        self.render_dialog();
    }

    /// Sets the window's view, unless it is the one already there.
    fn put(&mut self, window: &'static str, root: Element) {
        let view = root.into_view();
        if self.views.get(window) == Some(&view) {
            return;
        }
        ui::set_view(window, &view);
        self.views.insert(window, view);
    }

    // --- Settings → FluxGram ---

    fn render_accounts(&mut self) {
        let mut children = Vec::new();
        if self.state.accounts.is_empty() {
            children.push(text(
                "accounts-none",
                [span(&tr("No accounts yet.")).tone(Tone::Dim)],
            ));
        } else {
            let mut tree = Tree::new().two_line();
            for account in &self.state.accounts {
                let subtitle = if account.signing_out {
                    tr("Signing out…")
                } else {
                    match &account.label {
                        Some(label) => format!("{} · {label}", account.phone),
                        None => account.phone.clone(),
                    }
                };
                let mut row = RowSpec::new(&account.key, &account.name)
                    .avatar(avatar_of(account))
                    .subtitle(vec![span(&subtitle)]);
                if !account.signing_out {
                    row = row.action(
                        "sign-out",
                        Some(&tr("Sign Out")),
                        Some("icons/sign-out.svg"),
                        false,
                    );
                }
                tree.add(row);
            }
            children.push(tree.into_element("accounts-list"));
        }
        children.push(row(
            "accounts-buttons",
            [button("accounts-add", &tr("Add Account"))
                .icon("plus")
                .primary()
                .into()],
        ));
        self.put(ACCOUNTS, column("accounts-root", children));
    }

    // --- The Add Account dialog ---

    fn render_dialog(&mut self) {
        // Before the service's first word: the QR code's placeholder, as a sign-in starts.
        let sign_in = self.state.sign_in.clone().unwrap_or(SignIn {
            screen: Screen::Qr {
                link: String::new(),
            },
            busy: false,
            resending: false,
            problem: None,
        });
        let mut children = Vec::new();
        let mut buttons: Vec<Element> = Vec::new();
        // Back waits for TDLib's answer; Cancel always works.
        let back = || {
            button("sign-in-back", &tr("Back"))
                .enabled(!sign_in.busy)
                .into()
        };
        let cancel = || button("sign-in-cancel", &tr("Cancel")).into();
        let next = |label: &str| {
            button("sign-in-next", label)
                .primary()
                .busy(sign_in.busy && !sign_in.resending)
                .enabled(!sign_in.resending)
                .into()
        };
        // The QR code's age: a new link starts the countdown again.
        match &sign_in.screen {
            Screen::Qr { link } if link.is_empty() => self.link = None,
            Screen::Qr { link } if self.link.as_ref().is_none_or(|(shown, _)| shown != link) => {
                self.link = Some((link.clone(), Instant::now()));
            }
            Screen::Qr { .. } => {}
            _ => self.link = None,
        }
        self.counting = false;
        match &sign_in.screen {
            Screen::NotSetUp => {
                children.push(text(
                    "sign-in-about",
                    [span(&tr(
                        "FluxGram isn't set up to sign in to Telegram yet. An update of the plugin will bring it.",
                    ))],
                ));
                buttons.extend([spacer("sign-in-gap"), cancel()]);
            }
            Screen::Qr { link } => {
                // An empty link: the placeholder with a spinner, until TDLib gets the code — but
                // not under a problem, which no wait mends.
                if !link.is_empty() || sign_in.problem.is_none() {
                    children.push(qr("sign-in-qr", link));
                }
                children.push(markdown(
                    "sign-in-steps",
                    &tr("1. Open Telegram on your phone\n2. Go to Settings → Devices → Link Desktop Device\n3. Point the camera at the code"),
                ));
                let age = self
                    .link
                    .as_ref()
                    .map_or(0, |(_, at)| at.elapsed().as_secs());
                let countdown = if link.is_empty() {
                    tr("Getting the code…")
                } else {
                    match QR_LIFETIME.checked_sub(age).filter(|left| *left > 0) {
                        Some(left) => trf("The code refreshes in {0} s", &[&left]),
                        None => tr("Refreshing the code…"),
                    }
                };
                if sign_in.problem.is_none() {
                    children.push(text(
                        "sign-in-countdown",
                        [span(&countdown).tone(Tone::Dim)],
                    ));
                }
                buttons.extend([
                    button("sign-in-phone", &tr("Use Phone Number"))
                        .icon("icons/phone.svg")
                        .into(),
                    spacer("sign-in-gap"),
                    cancel(),
                ]);
            }
            Screen::Phone => {
                children.push(about(&tr("Your phone number with the country code.")));
                children.push(text_field(
                    PHONE,
                    self.field(PHONE),
                    Some("+7 900 000-00-00"),
                ));
                buttons.extend([back(), spacer("sign-in-gap"), cancel(), next(&tr("Next"))]);
            }
            Screen::Code {
                phone,
                sent,
                resend,
                resend_at,
            } => {
                children.push(about(&code_sent_text(*sent, phone)));
                children.push(text_field(CODE, self.field(CODE), Some(&tr("Code"))));
                // Didn't get the code: another way when Telegram has one (after its countdown),
                // and the QR code, which needs no code.
                children.push(text(
                    "sign-in-no-code",
                    [span(&tr(
                        "Didn't get the code? You can also sign in with the QR code from your phone.",
                    ))
                    .tone(Tone::Dim)],
                ));
                // The QR code first: it's the main way.
                let mut other: Vec<Element> = vec![
                    button("sign-in-use-qr", &tr("Sign In with QR Code"))
                        .icon("icons/qr.svg")
                        .enabled(!sign_in.busy)
                        .into(),
                ];
                if let Some(next) = resend {
                    let left = resend_at.saturating_sub(unix_now());
                    self.counting = left > 0;
                    let label = if left > 0 {
                        trf(resend_wait(*next), &[&countdown(left)])
                    } else {
                        resend_label(*next)
                    };
                    other.push(
                        button("sign-in-resend", &label)
                            .enabled(left == 0 && (!sign_in.busy || sign_in.resending))
                            .busy(sign_in.resending)
                            .into(),
                    );
                }
                children.push(row("sign-in-other", other));
                buttons.extend([back(), spacer("sign-in-gap"), cancel(), next(&tr("Next"))]);
            }
            Screen::Password { hint } => {
                children.push(about(&tr("The account is protected with a password.")));
                children.push(password_field(
                    PASSWORD,
                    self.field(PASSWORD),
                    Some(&tr("Password")),
                ));
                if !hint.is_empty() {
                    children.push(text(
                        "sign-in-hint",
                        [span(&trf("Hint: {0}", &[hint])).tone(Tone::Dim)],
                    ));
                }
                buttons.extend([
                    back(),
                    spacer("sign-in-gap"),
                    cancel(),
                    next(&tr("Sign In")),
                ]);
            }
            Screen::EmailAddress => {
                children.push(about(&tr(
                    "Telegram asks for an email address to send the codes to.",
                )));
                children.push(text_field(
                    EMAIL,
                    self.field(EMAIL),
                    Some("name@example.com"),
                ));
                buttons.extend([back(), spacer("sign-in-gap"), cancel(), next(&tr("Next"))]);
            }
            Screen::EmailCode { pattern } => {
                children.push(about(&trf("The code sent to {0}.", &[pattern])));
                children.push(text_field(
                    EMAIL_CODE,
                    self.field(EMAIL_CODE),
                    Some(&tr("Code")),
                ));
                buttons.extend([back(), spacer("sign-in-gap"), cancel(), next(&tr("Next"))]);
            }
            Screen::NoAccount => {
                children.push(text(
                    "sign-in-about",
                    [span(&tr(
                        "This number has no Telegram account. Sign up in the Telegram app first, then add it here.",
                    ))],
                ));
                buttons.extend([back(), spacer("sign-in-gap"), cancel()]);
            }
        }
        if let Some(problem) = &sign_in.problem {
            let (template, value) = problem.text();
            let line = match value {
                Some(value) => trf(template, &[&value]),
                None => tr(template),
            };
            children.push(text("sign-in-error", [span(&line).tone(Tone::Error)]));
        }
        children.push(row("sign-in-buttons", buttons));
        self.put(SIGN_IN, column("sign-in-root", children));
        self.sync_tick();
    }

    fn field(&self, field: &str) -> &str {
        self.typed.get(field).map_or("", String::as_str)
    }

    /// The countdowns' timer runs only while the QR code, or the code screen counting down to its
    /// other way, is on show in this window.
    fn sync_tick(&mut self) {
        let wanted = self.dialog && (self.link.is_some() || self.counting);
        match (wanted, self.tick) {
            (true, None) => self.tick = Some(timers::every(Duration::from_secs(1))),
            (false, Some(tick)) => {
                timers::cancel(tick);
                self.tick = None;
            }
            _ => {}
        }
    }
}

/// The dialog's fields.
const PHONE: &str = "sign-in-phone-field";
const CODE: &str = "sign-in-code-field";
const PASSWORD: &str = "sign-in-password-field";
const EMAIL: &str = "sign-in-email-field";
const EMAIL_CODE: &str = "sign-in-email-code-field";
const FIELDS: [&str; 5] = [PHONE, CODE, PASSWORD, EMAIL, EMAIL_CODE];

fn about(line: &str) -> Element {
    text("sign-in-about", [span(line).tone(Tone::Dim)])
}

/// An account's picture: its photo, or its initials on its color.
pub fn avatar_of(account: &AccountView) -> AvatarSpec {
    AvatarSpec {
        image: account.photo.clone(),
        initials: initials(&account.name),
        color: account.color,
    }
}

/// Where the code went, in a line.
fn code_sent_text(sent: CodeSent, phone: &str) -> String {
    let phone = if phone.starts_with('+') {
        phone.to_string()
    } else {
        format!("+{phone}")
    };
    match sent {
        CodeSent::App => tr("We've sent the code to Telegram on your other device."),
        CodeSent::Sms => trf("We've sent the code by SMS to {0}.", &[&phone]),
        CodeSent::Call => trf("We're calling {0} to dictate the code.", &[&phone]),
        CodeSent::FlashCall | CodeSent::MissedCall => trf(
            "We're calling {0}: the code is the last digits of the calling number.",
            &[&phone],
        ),
        CodeSent::Fragment => tr("We've sent the code to your anonymous number."),
        CodeSent::Other => trf("Type the code sent to {0}.", &[&phone]),
    }
}

/// The button that asks for the code another way.
fn resend_label(next: CodeSent) -> String {
    match next {
        CodeSent::Sms => tr("Send by SMS"),
        CodeSent::Call | CodeSent::FlashCall | CodeSent::MissedCall => tr("Call Me"),
        _ => tr("Send Again"),
    }
}

/// The same button counting down: a template for `trf` with "0:58" in `{0}`.
fn resend_wait(next: CodeSent) -> &'static str {
    match next {
        CodeSent::Sms => "Send by SMS in {0}",
        CodeSent::Call | CodeSent::FlashCall | CodeSent::MissedCall => "Call Me in {0}",
        _ => "Send Again in {0}",
    }
}
