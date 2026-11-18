//! The service: one instance for the whole app (`service = true`). It runs the helper (TDLib behind
//! JSON lines, `helpers/fluxgram-helper/README.md`), keeps a TDLib client per account and the
//! sign-in in progress, and tells the windows' instances what changed ([`Note`]s: the accounts'
//! [`Snapshot`] only when it differs). It has no window: the windows draw, ask the user and pass
//! on what the user does ([`Request`]).
//!
//! Accounts: the list in the plugin's storage (`accounts`), each database's encryption key in
//! Flux's secrets (`database/<key>`), the files in `<data>/accounts/<key>/{db,files}`.
//!
//! The chats (part 3): every line of an account's client goes to its [`Store`] first, then to the
//! list ([`chats`]: the chats the user added, the Add Chat popup, the notifications) and to the
//! feed ([`feed`]: the open chats' messages, sending); answers go to whoever asked. At the end of
//! a batch both send what changed, and the helper's filter (`fluxgram.chats`) follows the chats in
//! a list or open, so the rest of Telegram never wakes the plugin.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use base64::Engine;
use flux_plugin_api::process::{self, OutputChannel, ProcessOutput};
use flux_plugin_api::{Event, host, log, notify, secrets, service, storage, timers, tr, trf};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::convert::MediaOptions;
use crate::model::{
    AccountView, Auth, KEPT, Lines, Me, Next, Note, Parameters, Problem, Request, Screen, SignIn,
    Snapshot, Words, code_log, full_name, looks_like_phone, mask_phone, next_step, parse_auth,
    parse_user, problem, restart_delay, russian_form, screen, unix_now,
};
use crate::store::Store;
use crate::{chats, config, feed, panel, profile};

/// The helper's name in the manifest's `[[helpers]]`.
const HELPER: &str = "fluxgram-helper";
/// The storage's key of the accounts' list.
const ACCOUNTS: &str = "accounts";
/// The helper ran this long: its next exit isn't one of a series.
const HEALTHY: Duration = Duration::from_secs(60);
/// After this many exits in a row the user is told (once).
const WARN_AFTER: u32 = 3;
/// The cache's size is looked at when a download ends, at most this often.
const CACHE_CHECK_EVERY: Duration = Duration::from_secs(600);
/// The cache's limit when the setting has none, in GB.
const CACHE_LIMIT_GB: u64 = 2;

/// An account signed in, as kept in the plugin's storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Account {
    key: String,
    user_id: i64,
    first_name: String,
    last_name: String,
    phone: String,
    #[serde(default)]
    username: Option<String>,
    color: u8,
    /// The small profile photo, absolute.
    #[serde(default)]
    photo: Option<String>,
}

impl Account {
    fn name(&self) -> String {
        full_name(&self.first_name, &self.last_name, &self.phone)
    }
}

/// What the service knows of an account's client while the helper runs.
#[derive(Default)]
struct Live {
    /// The profile photo being downloaded: TDLib's file id.
    photo_file: Option<i64>,
    /// Sign Out was confirmed: `logOut` is on its way.
    signing_out: bool,
}

/// The sign-in in progress: a client of its own until it is an account.
struct Pending {
    key: String,
    /// The database's encryption key, base64: into the secrets once signed in.
    database_key: String,
    auth: Auth,
    /// The user chose the number over the QR code.
    phone_mode: bool,
    /// The phone screen is shown (chosen, gone back to, or its number on its way to TDLib).
    phone_screen: bool,
    /// A number typed while the client was starting: sent once it can take one.
    queued_phone: Option<String>,
    /// The number is on its way: the phone screen stays until TDLib takes it.
    phone_sent: bool,
    /// A QR code is asked for and hasn't come: its placeholder on the QR screen; a number typed
    /// meanwhile waits for the fresh client the code's arrival brings (`Next::Renew`).
    qr_asked: bool,
    /// TDLib works on what the user asked: Next is busy.
    busy: bool,
    /// The code is being sent again.
    resending: bool,
    problem: Option<Problem>,
    /// The screen on show: it stays while TDLib works between two screens.
    shown: Screen,
}

impl Pending {
    fn new(key: String) -> Self {
        Pending {
            key,
            database_key: database_key(),
            auth: Auth::WaitParameters,
            phone_mode: false,
            phone_screen: false,
            queued_phone: None,
            phone_sent: false,
            qr_asked: false,
            busy: false,
            resending: false,
            problem: None,
            shown: Screen::Qr {
                link: String::new(),
            },
        }
    }

    /// The dialog's screen: the phone screen when it is on show, the QR code's placeholder when
    /// one is asked for, else TDLib's state's — or the one on show while TDLib works.
    fn screen(&self) -> Screen {
        if self.phone_screen {
            Screen::Phone
        } else if self.qr_asked {
            Screen::Qr {
                link: String::new(),
            }
        } else {
            screen(&self.auth, self.phone_mode).unwrap_or_else(|| self.shown.clone())
        }
    }

    /// The requests on their way are gone (a new client, a new helper): only a number typed
    /// before keeps Next busy.
    fn forget_requests(&mut self) {
        self.phone_sent = false;
        self.qr_asked = false;
        self.resending = false;
        self.busy = self.queued_phone.is_some();
    }
}

/// What an `@extra` asked.
pub enum Asked {
    /// `getMe` of a client.
    Me(String),
    /// A sign-in request of the pending client: its error goes under the dialog's field.
    SignIn(String),
    /// The pending client's phone number: its error goes back to the phone screen.
    Phone(String),
    /// A request of the list: its answer goes there.
    Chats(chats::Ask),
    /// A request of the feed: its answer goes there.
    Feed(feed::Ask),
    /// A request of the profiles: its answer goes there.
    Profile(profile::Ask),
    /// A request of the field's panel: its answer goes there.
    Panel(panel::Ask),
    /// The helper's render of an animation (`fluxgram.render`): its output.
    Render(String),
    /// The size of an account's cache (`getStorageStatisticsFast`): over the limit — optimized.
    Cache(String),
    /// Anything else: an error goes to the log.
    Other(&'static str),
}

/// The helper as the service and its parts talk to it: the process, and what each `@extra` asked.
#[derive(Default)]
pub struct Td {
    /// The helper's process.
    helper: Option<u64>,
    asked: HashMap<u64, Asked>,
    next_extra: u64,
}

impl Td {
    /// The helper runs: requests go somewhere.
    pub fn running(&self) -> bool {
        self.helper.is_some()
    }

    /// A line to the helper as it is (`fluxgram.*`).
    pub fn write(&mut self, value: &Value) {
        let Some(helper) = self.helper else {
            return;
        };
        let mut line = value.to_string();
        line.push('\n');
        if let Err(err) = process::write(helper, line.as_bytes()) {
            log::warn(&format!("Couldn't write to the helper: {err}"));
        }
    }

    /// A request of the helper itself (`fluxgram.render`): its answer comes with the `@extra`,
    /// without a client.
    pub fn ask(&mut self, mut request: Value, asked: Asked) {
        if self.helper.is_none() {
            return;
        }
        self.next_extra += 1;
        request["@extra"] = json!(self.next_extra);
        self.asked.insert(self.next_extra, asked);
        self.write(&request);
    }

    /// A TDLib request of the client `key`; `asked` — what its answer is for.
    pub fn send(&mut self, key: &str, mut request: Value, asked: Asked) {
        if self.helper.is_none() {
            return;
        }
        self.next_extra += 1;
        request["@client"] = json!(key);
        request["@extra"] = json!(self.next_extra);
        self.asked.insert(self.next_extra, asked);
        self.write(&request);
    }
}

/// What the list and the feed work with: the helper and the accounts' stores.
pub struct Cx<'a> {
    pub td: &'a mut Td,
    pub stores: &'a mut HashMap<String, Store>,
}

/// The words of the chats through the host's translations ([`Words`]).
pub struct HostWords;

impl Words for HostWords {
    fn tr(&self, text: &str) -> String {
        tr(text)
    }

    fn trf(&self, template: &str, values: &[&str]) -> String {
        let values: Vec<&dyn std::fmt::Display> = values
            .iter()
            .map(|value| value as &dyn std::fmt::Display)
            .collect();
        trf(template, &values)
    }

    fn count(&self, n: u64, one: &str, other: &str) -> String {
        let template = if host::i18n::language() == "ru" {
            let key = format!("{other}|{}", russian_form(n));
            let form = tr(&key);
            if form == key { tr(other) } else { form }
        } else if n == 1 {
            tr(one)
        } else {
            tr(other)
        };
        template.replace("{0}", &n.to_string())
    }
}

pub struct Service {
    td: Td,
    lines: Lines,
    started: Option<Instant>,
    /// Exits of the helper in a row.
    failures: u32,
    restart: Option<u64>,
    warned: bool,
    /// The plugin is stopping: the helper's exit is expected.
    stopping: bool,
    accounts: Vec<Account>,
    live: HashMap<String, Live>,
    pending: Option<Pending>,
    /// Add Account without an app key: the dialog says so.
    not_set_up: bool,
    /// Clients being closed whose folders go once they are (cancelled sign-ins, duplicates,
    /// signed out elsewhere).
    discarded: HashSet<String>,
    /// The last snapshot sent, as JSON.
    sent: Option<String>,
    /// What the service knows of each signed-in account's Telegram, while the helper runs.
    stores: HashMap<String, Store>,
    /// The list: the chats the user added, the Add Chat popup, the notifications.
    chats: chats::Chats,
    /// The open chats' feeds and what the user sends.
    feed: feed::Feed,
    /// The profile dialog's people, groups, members and media.
    profiles: profile::Profiles,
    /// The field's panel: stickers and GIFs (part 4).
    panels: panel::Panels,
    /// When each account's cache was last looked at.
    cache_checked: HashMap<String, Instant>,
    /// The helper's chat filter of each account, as last sent (`fluxgram.chats`).
    filters: HashMap<String, Vec<i64>>,
}

impl Service {
    pub fn new() -> Self {
        Service {
            td: Td::default(),
            lines: Lines::default(),
            started: None,
            failures: 0,
            restart: None,
            warned: false,
            stopping: false,
            accounts: Vec::new(),
            live: HashMap::new(),
            pending: None,
            not_set_up: false,
            discarded: HashSet::new(),
            sent: None,
            stores: HashMap::new(),
            chats: chats::Chats::new(),
            feed: feed::Feed::new(),
            profiles: profile::Profiles::new(),
            panels: panel::Panels::new(),
            cache_checked: HashMap::new(),
            filters: HashMap::new(),
        }
    }

    pub fn activate(&mut self) {
        self.feed.set_media(media_options());
        self.accounts = storage::get(ACCOUNTS).unwrap_or_default();
        let keys: Vec<String> = self.accounts.iter().map(|a| a.key.clone()).collect();
        self.chats.activate(&keys);
        self.start_helper();
        // Windows that started before the service learn the state now.
        self.publish();
    }

    /// The plugin stops (Flux quits, the plugin is turned off): the end of the helper's input
    /// closes every client, so the databases are left whole.
    pub fn deactivate(&mut self) {
        self.stopping = true;
        if let Some(helper) = self.td.helper.take() {
            process::close_stdin(helper);
        }
    }

    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::ProcessOutput(output) => self.output(output),
            Event::ProcessExited((id, code)) if Some(id) == self.td.helper => self.exited(code),
            Event::Timer(id) if Some(id) == self.restart => {
                self.restart = None;
                self.start_helper();
                self.publish();
            }
            Event::Timer(id) => {
                let mut cx = Cx {
                    td: &mut self.td,
                    stores: &mut self.stores,
                };
                if !self.chats.timer(&mut cx, id) {
                    self.feed.timer(&mut cx, id);
                }
                self.flush();
            }
            Event::WindowOpened(window) => self.hello(window),
            Event::SettingsChanged => {
                self.feed.set_media(media_options());
                self.panels.set_media(&self.feed.media);
                // A lower limit applies at once.
                self.cache_checked.clear();
                let keys: Vec<String> = self.stores.keys().cloned().collect();
                for key in keys {
                    self.check_cache(&key);
                }
                self.flush();
            }
            Event::WindowClosed(window) => {
                let mut cx = Cx {
                    td: &mut self.td,
                    stores: &mut self.stores,
                };
                self.chats.window_closed(&mut cx, window);
                self.feed.window_closed(&mut cx, window);
                self.profiles.window_closed(window);
                self.panels.window_closed(window);
                self.flush();
            }
            Event::ServiceMessage((Some(window), text)) => match service::parse(&text) {
                Some(Request::Hello) => self.hello(window),
                Some(request) => {
                    self.request(window, request);
                    self.publish();
                    self.flush();
                }
                None => log::warn(&format!("An unknown message from a window: {text}")),
            },
            _ => {}
        }
    }

    /// A window that has just started (or whose service has): the state, then the lists.
    fn hello(&mut self, window: u64) {
        let _ = service::send_json_to(window, &Note::State(self.snapshot()));
        let mut cx = Cx {
            td: &mut self.td,
            stores: &mut self.stores,
        };
        self.chats.window_opened(&mut cx, window);
    }

    /// The end of a batch (of the helper's output, a window's request, a timer): the list and the
    /// feed send what changed, and the helper's filter follows the chats in a list or open.
    fn flush(&mut self) {
        let mut cx = Cx {
            td: &mut self.td,
            stores: &mut self.stores,
        };
        self.chats.flush(&mut cx);
        self.feed.flush(&mut cx);
        self.profiles.flush(&mut cx);
        self.panels.flush(&mut cx, &mut self.feed);
        self.sync_filters();
    }

    /// `fluxgram.chats` of each account whose chats in a list or open changed: the helper passes
    /// only their updates (and those of no chat).
    fn sync_filters(&mut self) {
        if !self.td.running() {
            return;
        }
        for account in &self.accounts {
            let mut wanted = self.chats.listed(&account.key);
            wanted.extend(self.feed.open(&account.key));
            // A profile on show: its person's status, its muting.
            wanted.extend(self.profiles.chats(&account.key));
            wanted.sort_unstable();
            wanted.dedup();
            if self.filters.get(&account.key) == Some(&wanted) {
                continue;
            }
            self.td
                .write(&json!({"@type": "fluxgram.chats", "@client": account.key,
                "chats": wanted}));
            self.filters.insert(account.key.clone(), wanted);
        }
    }

    /// The account's client is ready (signed in now, or again after the helper's start): the
    /// list and the feed load what they show.
    fn account_ready(&mut self, key: &str, me: i64) {
        self.stores.entry(key.to_string()).or_default().me = Some(me);
        self.sync_filters();
        let mut cx = Cx {
            td: &mut self.td,
            stores: &mut self.stores,
        };
        self.chats.account_ready(&mut cx, key);
        self.feed.account_ready(&mut cx, key);
        self.panels.account_ready(key);
        self.check_cache(key);
    }

    /// The account is gone (signed out): its store, list and feeds go.
    fn account_gone(&mut self, key: &str) {
        self.stores.remove(key);
        self.filters.remove(key);
        self.chats.account_gone(key);
        self.feed.account_gone(key);
        self.profiles.account_gone(key);
        self.panels.account_gone(key);
        self.cache_checked.remove(key);
    }

    // --- The helper ---

    fn start_helper(&mut self) {
        if self.td.helper.is_some() || self.stopping {
            return;
        }
        let dir = storage::dir();
        let started = process::helper(HELPER).and_then(|path| {
            process::command(&path)
                .args(["--log", &format!("{dir}/tdlib.log"), "--verbosity", "1"])
                .cwd(&dir)
                .spawn()
        });
        let id = match started {
            Ok(id) => id,
            Err(err) => {
                log::error(&format!("The helper didn't start: {err}"));
                self.failed();
                return;
            }
        };
        self.td.helper = Some(id);
        self.started = Some(Instant::now());
        self.lines.clear();
        self.live.clear();
        self.td.asked.clear();
        // A new TDLib: what it told the stores is told again as the clients start (before their
        // `getMe` is answered: the stores take the start's burst).
        self.stores = self
            .accounts
            .iter()
            .map(|account| (account.key.clone(), Store::default()))
            .collect();
        self.filters.clear();
        let kept: Vec<&str> = KEPT
            .iter()
            .chain(chats::KEPT)
            .chain(feed::KEPT)
            .chain(profile::KEPT)
            .copied()
            .collect();
        self.td
            .write(&json!({"@type": "fluxgram.keep", "types": kept}));
        // The filters before the clients: their start's burst is filtered already.
        self.sync_filters();
        let keys: Vec<String> = self.accounts.iter().map(|a| a.key.clone()).collect();
        for key in keys {
            self.live.insert(key.clone(), Live::default());
            self.open_client(&key);
        }
        if let Some(pending) = &mut self.pending {
            pending.auth = Auth::WaitParameters;
            pending.forget_requests();
            let key = pending.key.clone();
            self.open_client(&key);
        }
    }

    /// The helper ended: everything it held is gone; it starts again after a pause that grows
    /// with exits in a row.
    fn exited(&mut self, code: Option<i32>) {
        self.td.helper = None;
        self.live.clear();
        self.td.asked.clear();
        // The closing clients' folders are left: no TDLib holds them now.
        for key in std::mem::take(&mut self.discarded) {
            remove_folder(&key);
        }
        if self.stopping {
            return;
        }
        log::warn(&format!("The helper ended: {code:?}"));
        if let Some(pending) = &mut self.pending {
            pending.auth = Auth::WaitParameters;
            pending.forget_requests();
            pending.problem = Some(Problem::ConnectionLost);
        }
        self.failed();
        self.publish();
    }

    fn failed(&mut self) {
        if self
            .started
            .is_some_and(|started| started.elapsed() > HEALTHY)
        {
            self.failures = 0;
        }
        self.started = None;
        self.failures += 1;
        if self.failures >= WARN_AFTER && !self.warned {
            self.warned = true;
            notify::warning(
                &tr("FluxGram keeps losing its connection"),
                Some(&tr(
                    "It reconnects by itself; the plugin's log has the details.",
                )),
            );
        }
        let delay = restart_delay(self.failures);
        self.restart = Some(timers::after(Duration::from_secs(delay)));
    }

    /// The first request of a key makes its client; TDLib then asks for the parameters.
    fn open_client(&mut self, key: &str) {
        self.td.send(
            key,
            json!({"@type": "getOption", "name": "version"}),
            Asked::Other("getOption"),
        );
    }

    fn output(&mut self, output: ProcessOutput) {
        if Some(output.process) != self.td.helper {
            return;
        }
        if output.channel == OutputChannel::Stderr {
            let text = String::from_utf8_lossy(&output.bytes);
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                log::info(line);
            }
            return;
        }
        for line in self.lines.push(&output.bytes) {
            match serde_json::from_str::<Value>(&line) {
                Ok(value) => self.line(value),
                Err(err) => log::warn(&format!("Not JSON from the helper ({err}): {line}")),
            }
        }
        // One snapshot for the whole batch, if it changed; the list's and the feed's changes too.
        self.publish();
        self.flush();
    }

    fn line(&mut self, value: Value) {
        let client = value["@client"].as_str().unwrap_or_default().to_string();
        // A signed-in account's line: its store takes it, then the list and the feed (answers go
        // to whoever asked).
        if self.stores.contains_key(&client) {
            self.account_line(&client, &value);
        }
        match value["@type"].as_str().unwrap_or_default() {
            "fluxgram.ready" => log::info(&format!(
                "The helper {} is ready: TDLib {}",
                value["version"].as_str().unwrap_or("?"),
                value["tdlib"].as_str().unwrap_or("?")
            )),
            "fluxgram.closed" => self.closed(&client),
            "fluxgram.error" if value["@extra"].is_null() => {
                log::warn(&format!("The helper: {}", value["message"]))
            }
            "updateAuthorizationState" => {
                let state = &value["authorization_state"];
                if self.pending.as_ref().is_some_and(|p| p.key == client)
                    && let Some(line) = code_log(state)
                {
                    log::info(&line);
                }
                self.auth(&client, parse_auth(state, unix_now()));
            }
            "updateConnectionState" => log::info(&format!(
                "{client}: {}",
                value["state"]["@type"].as_str().unwrap_or("?")
            )),
            "updateUser" => {
                if let Some(me) = parse_user(&value["user"]) {
                    self.user(&client, me, false);
                }
            }
            "updateFile" => {
                self.file(&client, &value["file"]);
                if value["file"]["local"]["is_downloading_completed"] == true {
                    self.check_cache(&client);
                }
            }
            _ => {
                if let Some(extra) = value["@extra"].as_u64() {
                    self.answer(&client, extra, &value);
                }
            }
        }
    }

    /// A line of a signed-in account's client, for the chats.
    fn account_line(&mut self, key: &str, value: &Value) {
        let Some(store) = self.stores.get_mut(key) else {
            return;
        };
        let touched = store.apply(value);
        let mut cx = Cx {
            td: &mut self.td,
            stores: &mut self.stores,
        };
        if let Some(extra) = value["@extra"].as_u64() {
            match cx.td.asked.remove(&extra) {
                Some(Asked::Chats(ask)) => self.chats.answer(&mut cx, key, ask, value),
                Some(Asked::Feed(ask)) => self.feed.answer(&mut cx, key, ask, value),
                Some(Asked::Profile(ask)) => self.profiles.answer(&mut cx, key, ask, value),
                Some(Asked::Panel(ask)) => {
                    self.panels.answer(&mut cx, &mut self.feed, key, ask, value)
                }
                // The sign-in's and the account's own: `answer` takes them.
                Some(asked) => {
                    cx.td.asked.insert(extra, asked);
                }
                None => {}
            }
            return;
        }
        self.chats.update(&mut cx, key, value, touched);
        self.feed.update(&mut cx, key, value, touched);
        self.profiles.update(&mut cx, key, value, touched);
        self.panels.update(key, touched);
    }

    fn answer(&mut self, client: &str, extra: u64, value: &Value) {
        let Some(asked) = self.td.asked.remove(&extra) else {
            return;
        };
        let error = (value["@type"] == "error").then(|| {
            (
                value["code"].as_i64().unwrap_or_default(),
                value["message"].as_str().unwrap_or_default().to_string(),
            )
        });
        let asked_phone = matches!(asked, Asked::Phone(_));
        match (asked, error) {
            (Asked::Me(key), None) => {
                if let Some(me) = parse_user(value) {
                    let id = me.id;
                    self.user(&key, me, true);
                    // Signed in now, or a stored account's client started: the chats load.
                    if self
                        .accounts
                        .iter()
                        .any(|a| a.key == key && a.user_id == id)
                    {
                        self.account_ready(&key, id);
                    }
                }
            }
            (Asked::SignIn(key) | Asked::Phone(key), Some((code, message))) => {
                if let Some(pending) = self.pending.as_mut().filter(|p| p.key == key) {
                    log::info(&format!("Sign-in: {code} {message}"));
                    pending.busy = false;
                    pending.resending = false;
                    pending.qr_asked = false;
                    pending.phone_sent = false;
                    pending.problem = Some(problem(code, &message));
                    if asked_phone {
                        pending.phone_screen = true;
                    }
                }
            }
            (Asked::Me(_), Some((code, message))) => {
                log::warn(&format!("getMe of {client}: {code} {message}"));
            }
            (Asked::Other(what), Some((code, message))) => {
                log::warn(&format!("{what} of {client}: {code} {message}"));
            }
            (Asked::Render(output), _) => {
                let ok = value["@type"] == "fluxgram.rendered";
                if !ok {
                    log::warn(&format!("A sticker didn't render: {}", value["message"]));
                }
                self.feed.rendered(&output, ok);
                self.panels.rendered(&output, ok);
            }
            (Asked::Cache(key), None) => self.cache_size(&key, value),
            _ => {}
        }
    }

    // --- The cache (part 4) ---

    /// Looks at the account's cache when a download ends (at most every `CACHE_CHECK_EVERY`):
    /// over the limit, TDLib deletes the files used longest ago.
    fn check_cache(&mut self, key: &str) {
        if !self.stores.contains_key(key)
            || self
                .cache_checked
                .get(key)
                .is_some_and(|at| at.elapsed() < CACHE_CHECK_EVERY)
        {
            return;
        }
        self.cache_checked.insert(key.to_string(), Instant::now());
        self.td.send(
            key,
            json!({"@type": "getStorageStatisticsFast"}),
            Asked::Cache(key.to_string()),
        );
    }

    /// The cache's size: over the limit — `optimizeStorage` down to it (files of the last hour
    /// stay: they are on screen).
    fn cache_size(&mut self, key: &str, statistics: &Value) {
        let size = statistics["files_size"].as_u64().unwrap_or_default();
        let limit = cache_limit();
        if limit == 0 || size <= limit {
            return;
        }
        log::info(&format!(
            "{key}: the cache is {} MB, over its {} MB: optimizing",
            size >> 20,
            limit >> 20
        ));
        self.td.send(
            key,
            json!({"@type": "optimizeStorage", "size": limit, "ttl": 10 * 365 * 86_400,
                "count": 1_000_000, "immunity_delay": -1, "file_types": [], "chat_ids": [],
                "exclude_chat_ids": [], "return_deleted_file_statistics": false,
                "chat_limit": 0}),
            Asked::Other("optimizeStorage"),
        );
    }

    /// Clear Cache (Settings, confirmed): every account's media files and the rendered
    /// animations go; the open chats download what they show again.
    fn clear_cache(&mut self) {
        let keys: Vec<String> = self.stores.keys().cloned().collect();
        for key in keys {
            self.td.send(
                &key,
                json!({"@type": "optimizeStorage", "size": 0, "ttl": 0, "count": 0,
                    "immunity_delay": 0, "file_types": CLEARED.iter()
                        .map(|kind| json!({"@type": kind})).collect::<Vec<Value>>(),
                    "chat_ids": [], "exclude_chat_ids": [],
                    "return_deleted_file_statistics": false, "chat_limit": 0}),
                Asked::Other("optimizeStorage"),
            );
        }
        let frames = frames_dir();
        if let Err(err) = std::fs::remove_dir_all(&frames)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            log::warn(&format!("Couldn't clear the animations: {err}"));
        }
        self.feed.cache_cleared();
        self.panels.cache_cleared();
        notify::success(&tr("The cache is cleared"));
    }

    // --- Authorization ---

    fn auth(&mut self, key: &str, auth: Auth) {
        if self.pending.as_ref().is_some_and(|p| p.key == key) {
            self.pending_auth(auth);
        } else if self.accounts.iter().any(|a| a.key == key) {
            self.account_auth(key, auth);
        }
    }

    /// A stored account's client: opened with its key, or signed out elsewhere.
    fn account_auth(&mut self, key: &str, auth: Auth) {
        match auth {
            Auth::WaitParameters => match secrets::get(&secret_key(key)) {
                Some(database_key) => {
                    let request = parameters(key, &database_key);
                    self.td
                        .send(key, request, Asked::Other("setTdlibParameters"));
                }
                None => {
                    log::warn(&format!("No database key of {key}"));
                    self.forget(key, true);
                }
            },
            Auth::Ready => self
                .td
                .send(key, json!({"@type": "getMe"}), Asked::Me(key.into())),
            Auth::LoggingOut | Auth::Closing | Auth::Closed | Auth::Other => {}
            // No session any more: ended on another device, or by Telegram.
            signed_out if signed_out.is_signed_out() => self.forget(key, true),
            _ => {}
        }
    }

    fn pending_auth(&mut self, auth: Auth) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if pending.auth != auth {
            pending.problem = None;
        }
        // TDLib took the number: the phone screen is done.
        if pending.phone_sent && auth.after_phone() {
            pending.phone_sent = false;
            pending.phone_screen = false;
        }
        if !matches!(auth, Auth::WaitParameters | Auth::WaitPhone) {
            pending.qr_asked = false;
        }
        pending.auth = auth.clone();
        // A new state answers the request on its way; a number typed before still waits.
        pending.busy = pending.queued_phone.is_some();
        pending.resending = false;
        let key = pending.key.clone();
        let (request, asked) =
            match next_step(&auth, pending.phone_mode, pending.queued_phone.as_deref()) {
                Next::Parameters => (
                    parameters(&key, &pending.database_key),
                    Asked::SignIn(key.clone()),
                ),
                // The QR code's placeholder shows that it's on its way: nothing is busy.
                Next::RequestQr => {
                    pending.qr_asked = true;
                    (qr_request(), Asked::SignIn(key.clone()))
                }
                Next::SendPhone(phone) => {
                    pending.queued_phone = None;
                    pending.busy = true;
                    pending.phone_sent = true;
                    (phone_request(&phone), Asked::Phone(key.clone()))
                }
                Next::GetMe => {
                    pending.busy = true;
                    (json!({"@type": "getMe"}), Asked::Me(key.clone()))
                }
                Next::Renew => {
                    self.renew_pending();
                    return;
                }
                Next::Nothing => {
                    if matches!(auth, Auth::Closed | Auth::LoggingOut) {
                        // The sign-in's client went away by itself: a fresh one.
                        pending.problem = Some(Problem::ConnectionLost);
                        self.renew_pending();
                    }
                    return;
                }
            };
        self.td.send(&key, request, asked);
    }

    /// The user's own account came: a sign-in ends, or a stored account's name and photo change.
    fn user(&mut self, key: &str, me: Me, answer: bool) {
        if answer && self.pending.as_ref().is_some_and(|p| p.key == key) {
            self.signed_in(key, me);
            return;
        }
        let Some(account) = self.accounts.iter_mut().find(|a| a.key == key) else {
            return;
        };
        // `updateUser` comes for every user the account sees: only its own counts.
        if account.user_id != me.id {
            return;
        }
        let before = account.clone();
        account.first_name = me.first_name;
        account.last_name = me.last_name;
        account.username = me.username;
        account.phone = me.phone;
        account.color = me.color;
        let download = match me.photo {
            Some((_, Some(path))) => {
                account.photo = Some(path);
                None
            }
            Some((file, None)) => Some(file),
            None => {
                account.photo = None;
                None
            }
        };
        let changed = *account != before;
        if changed {
            self.save();
        }
        if let Some(file) = download {
            self.download_photo(key, file);
        }
    }

    fn signed_in(&mut self, key: &str, me: Me) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if self.accounts.iter().any(|a| a.user_id == me.id) {
            // The same account twice: this new session goes.
            notify::info(&tr("This account is already in FluxGram."));
            self.discarded.insert(key.to_string());
            self.td
                .send(key, json!({"@type": "logOut"}), Asked::Other("logOut"));
            return;
        }
        if let Err(err) = secrets::set(&secret_key(key), &pending.database_key) {
            log::error(&format!("Couldn't keep the database key: {err}"));
        }
        let account = Account {
            key: key.to_string(),
            user_id: me.id,
            first_name: me.first_name.clone(),
            last_name: me.last_name.clone(),
            phone: me.phone.clone(),
            username: me.username.clone(),
            color: me.color,
            photo: me.photo.as_ref().and_then(|(_, path)| path.clone()),
        };
        let name = account.name();
        self.accounts.push(account);
        self.live.insert(key.to_string(), Live::default());
        self.stores.insert(key.to_string(), Store::default());
        self.save();
        notify::success(&trf("Signed in as {0}", &[&name]));
        if let Some((file, None)) = me.photo {
            self.download_photo(key, file);
        }
    }

    fn download_photo(&mut self, key: &str, file: i64) {
        let Some(live) = self.live.get_mut(key) else {
            return;
        };
        if live.photo_file == Some(file) {
            return;
        }
        live.photo_file = Some(file);
        self.td.send(
            key,
            json!({"@type": "downloadFile", "file_id": file, "priority": 1, "offset": 0,
                "limit": 0, "synchronous": false}),
            Asked::Other("downloadFile"),
        );
    }

    fn file(&mut self, key: &str, file: &Value) {
        let Some(path) = crate::model::downloaded(file) else {
            return;
        };
        let id = file["id"].as_i64();
        let Some(live) = self.live.get_mut(key) else {
            return;
        };
        if id.is_none() || live.photo_file != id {
            return;
        }
        live.photo_file = None;
        if let Some(account) = self.accounts.iter_mut().find(|a| a.key == key) {
            account.photo = Some(path);
            self.save();
        }
    }

    /// The helper's `fluxgram.closed`: the client is gone.
    fn closed(&mut self, key: &str) {
        if self.discarded.remove(key) {
            remove_folder(key);
            return;
        }
        if self.pending.as_ref().is_some_and(|p| p.key == key) {
            self.renew_pending();
            return;
        }
        let Some(account) = self.accounts.iter().find(|a| a.key == key) else {
            return;
        };
        let asked = self.live.get(key).is_some_and(|live| live.signing_out);
        let name = account.name();
        self.remove_account(key);
        if !asked {
            notify::Notice::warning(&trf("{0} was signed out", &[&name]))
                .body(&tr("The session was ended in Telegram."))
                .send();
        }
    }

    /// An account without its session: it goes, its client closes and its folder with it.
    fn forget(&mut self, key: &str, tell: bool) {
        let Some(account) = self.accounts.iter().find(|a| a.key == key) else {
            return;
        };
        let name = account.name();
        self.account_gone(key);
        self.accounts.retain(|a| a.key != key);
        self.live.remove(key);
        let _ = secrets::remove(&secret_key(key));
        self.save();
        if self.td.helper.is_some() {
            self.discarded.insert(key.to_string());
            self.td
                .send(key, json!({"@type": "close"}), Asked::Other("close"));
        } else {
            remove_folder(key);
        }
        if tell {
            notify::Notice::warning(&trf("{0} was signed out", &[&name]))
                .body(&tr("The session was ended in Telegram."))
                .send();
        }
    }

    /// After `logOut`'s `fluxgram.closed`: the account, its folder and its key go.
    fn remove_account(&mut self, key: &str) {
        self.account_gone(key);
        self.accounts.retain(|a| a.key != key);
        self.live.remove(key);
        let _ = secrets::remove(&secret_key(key));
        remove_folder(key);
        self.save();
    }

    fn save(&self) {
        storage::set(ACCOUNTS, &self.accounts);
    }

    // --- The windows' requests ---

    fn request(&mut self, window: u64, request: Request) {
        match request {
            Request::Hello => {}
            Request::Profile(ask) => {
                let mut cx = Cx {
                    td: &mut self.td,
                    stores: &mut self.stores,
                };
                self.profiles.request(&mut cx, window, ask);
            }
            Request::Search(ask) => {
                let mut cx = Cx {
                    td: &mut self.td,
                    stores: &mut self.stores,
                };
                self.feed.search(&mut cx, window, ask);
            }
            Request::Hidden(ask) => {
                let mut cx = Cx {
                    td: &mut self.td,
                    stores: &mut self.stores,
                };
                self.chats.hidden(&mut cx, window, ask);
            }
            Request::Folders(ask) => self.chats.folders(ask),
            Request::Panel(ask) => {
                let mut cx = Cx {
                    td: &mut self.td,
                    stores: &mut self.stores,
                };
                self.panels.request(&mut cx, &mut self.feed, window, ask);
            }
            Request::ClearCache => self.clear_cache(),
            // The list's.
            Request::AddChat { .. }
            | Request::RemoveChat { .. }
            | Request::SearchChats { .. }
            | Request::InSight { .. } => {
                let mut cx = Cx {
                    td: &mut self.td,
                    stores: &mut self.stores,
                };
                self.chats.request(&mut cx, window, request);
            }
            // The feed's.
            Request::OpenChat { .. }
            | Request::CloseChat { .. }
            | Request::LoadOlder { .. }
            | Request::LoadNewer { .. }
            | Request::Visible { .. }
            | Request::Send { .. }
            | Request::Edit { .. }
            | Request::Delete { .. }
            | Request::Download { .. }
            | Request::JumpTo { .. }
            | Request::Draft { .. }
            | Request::MediaPlayed { .. }
            | Request::Quote { .. } => {
                let mut cx = Cx {
                    td: &mut self.td,
                    stores: &mut self.stores,
                };
                self.feed.request(&mut cx, window, request);
            }
            Request::AddAccount => self.add_account(),
            Request::UsePhone => {
                if let Some(pending) = &mut self.pending {
                    pending.phone_mode = true;
                    pending.phone_screen = true;
                    pending.problem = None;
                    // TDLib doesn't take a number while its QR code waits: a fresh client. A QR
                    // code still on its way gets one when it comes (`Next::Renew`).
                    if matches!(pending.auth, Auth::WaitQr { .. }) {
                        self.renew_pending();
                    }
                }
            }
            Request::UseQr => self.use_qr(),
            Request::Back => self.back(),
            Request::Submit { text } => self.submit(text.trim()),
            Request::Resend => {
                if let Some(pending) = self.pending.as_mut().filter(|p| !p.busy) {
                    pending.busy = true;
                    pending.resending = true;
                    pending.problem = None;
                    let key = pending.key.clone();
                    self.td.send(
                        &key,
                        json!({"@type": "resendAuthenticationCode", "reason": null}),
                        Asked::SignIn(key.clone()),
                    );
                }
            }
            Request::Cancel => self.cancel(),
            Request::SignOut { key } => self.sign_out(&key),
        }
    }

    fn add_account(&mut self) {
        if !config::set_up() {
            self.not_set_up = true;
            return;
        }
        if self.pending.is_some() {
            return;
        }
        let key = new_key();
        self.pending = Some(Pending::new(key.clone()));
        if self.td.helper.is_none() && self.restart.is_none() {
            self.start_helper();
        } else {
            self.open_client(&key);
        }
    }

    /// The sign-in starts over with a fresh client (its mode and number kept); the old one closes
    /// and its folder goes.
    fn renew_pending(&mut self) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        let old = std::mem::replace(&mut pending.key, new_key());
        pending.database_key = database_key();
        pending.auth = Auth::WaitParameters;
        pending.forget_requests();
        let key = pending.key.clone();
        self.discard(&old);
        self.open_client(&key);
    }

    /// Closes a client whose folder goes once it is closed.
    fn discard(&mut self, key: &str) {
        if self.td.helper.is_some() {
            self.discarded.insert(key.to_string());
            self.td
                .send(key, json!({"@type": "close"}), Asked::Other("close"));
        } else {
            remove_folder(key);
        }
    }

    fn back(&mut self) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        pending.problem = None;
        if pending.phone_screen || matches!(pending.auth, Auth::WaitPhone) {
            // The phone screen → the QR code.
            self.use_qr();
        } else {
            // The code, the password, the email → the number again.
            pending.phone_screen = true;
            pending.phone_mode = true;
        }
    }

    /// Back to the QR code (from the phone screen, or the code that didn't come): asked for at
    /// once when TDLib takes the request; a starting client asks for it by itself.
    fn use_qr(&mut self) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        let on_its_way = pending.qr_asked;
        pending.problem = None;
        pending.phone_screen = false;
        pending.phone_mode = false;
        pending.queued_phone = None;
        pending.forget_requests();
        if on_its_way {
            pending.qr_asked = true;
        } else if pending.auth.takes_phone() {
            pending.qr_asked = true;
            let key = pending.key.clone();
            self.td.send(&key, qr_request(), Asked::SignIn(key.clone()));
        }
    }

    fn submit(&mut self, text: &str) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        if pending.busy {
            return;
        }
        let key = pending.key.clone();
        let request = match pending.screen() {
            Screen::Phone => {
                if !looks_like_phone(text) {
                    pending.problem = Some(Problem::InvalidPhone);
                    return;
                }
                let phone: String = text
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '+')
                    .collect();
                // The phone screen stays, Next busy, until TDLib takes the number.
                pending.phone_mode = true;
                if !pending.auth.takes_phone() || pending.qr_asked {
                    // The client is starting, or is to be replaced once its QR code comes: the
                    // number waits for the client that takes it.
                    pending.queued_phone = Some(phone);
                    pending.busy = true;
                    pending.problem = None;
                    return;
                }
                pending.phone_sent = true;
                phone_request(&phone)
            }
            Screen::Code { .. } if !text.is_empty() => {
                json!({"@type": "checkAuthenticationCode", "code": text})
            }
            Screen::Password { .. } if !text.is_empty() => {
                json!({"@type": "checkAuthenticationPassword", "password": text})
            }
            Screen::EmailAddress if text.contains('@') => {
                json!({"@type": "setAuthenticationEmailAddress", "email_address": text})
            }
            Screen::EmailAddress => {
                pending.problem = Some(Problem::InvalidEmail);
                return;
            }
            Screen::EmailCode { .. } if !text.is_empty() => json!({
                "@type": "checkAuthenticationEmailCode",
                "code": {"@type": "emailAddressAuthenticationCode", "code": text}
            }),
            _ => return,
        };
        pending.busy = true;
        pending.problem = None;
        let asked = if request["@type"] == "setAuthenticationPhoneNumber" {
            Asked::Phone(key.clone())
        } else {
            Asked::SignIn(key.clone())
        };
        self.td.send(&key, request, asked);
    }

    fn cancel(&mut self) {
        self.not_set_up = false;
        if let Some(pending) = self.pending.take() {
            self.discard(&pending.key);
        }
    }

    fn sign_out(&mut self, key: &str) {
        if !self.accounts.iter().any(|a| a.key == key) {
            return;
        }
        match self.live.get_mut(key) {
            Some(live) if self.td.helper.is_some() => {
                live.signing_out = true;
                self.td
                    .send(key, json!({"@type": "logOut"}), Asked::Other("logOut"));
            }
            // No helper to tell Telegram: the account goes from this computer.
            _ => self.remove_account(key),
        }
    }

    // --- What the windows show ---

    fn snapshot(&self) -> Snapshot {
        let accounts = self
            .accounts
            .iter()
            .map(|account| AccountView {
                key: account.key.clone(),
                name: account.name(),
                phone: mask_phone(&account.phone),
                label: account.username.as_ref().map(|name| format!("@{name}")),
                color: account.color,
                photo: account.photo.clone(),
                signing_out: self
                    .live
                    .get(&account.key)
                    .is_some_and(|live| live.signing_out),
            })
            .collect();
        let sign_in = if self.not_set_up {
            Some(SignIn {
                screen: Screen::NotSetUp,
                busy: false,
                resending: false,
                problem: None,
            })
        } else {
            self.pending.as_ref().map(|pending| SignIn {
                screen: pending.screen(),
                busy: pending.busy,
                resending: pending.resending,
                problem: pending.problem.clone(),
            })
        };
        Snapshot { accounts, sign_in }
    }

    /// Sends the windows the snapshot when it changed.
    fn publish(&mut self) {
        // What is on show now stays on show while TDLib works between two screens.
        if let Some(pending) = &mut self.pending {
            pending.shown = pending.screen();
        }
        let Ok(text) = serde_json::to_string(&self.snapshot()) else {
            return;
        };
        if self.sent.as_deref() == Some(text.as_str()) {
            return;
        }
        let _ = service::broadcast_json(&Note::State(self.snapshot()));
        self.sent = Some(text);
    }
}

/// The file types Clear Cache deletes: the media of messages and their stickers (the small
/// thumbnails and profile photos stay — the lists look right at once).
const CLEARED: &[&str] = &[
    "fileTypePhoto",
    "fileTypeVideo",
    "fileTypeVoiceNote",
    "fileTypeVideoNote",
    "fileTypeAnimation",
    "fileTypeAudio",
    "fileTypeDocument",
    "fileTypeSticker",
];

/// The plugin's media settings (`[[settings]]` of the manifest), with the folder of the rendered
/// animations.
fn media_options() -> MediaOptions {
    let on = |key: &str| flux_plugin_api::setting::<bool>(key).unwrap_or(true);
    MediaOptions {
        autoplay: on("autoplay"),
        loop_stickers: on("loop-stickers"),
        photos: on("download-photos"),
        voice: on("download-voice"),
        gifs: on("download-gifs"),
        frames: frames_dir(),
        rendered: Default::default(),
    }
}

/// The cache's limit in bytes (the setting is in GB; 0 — none).
fn cache_limit() -> u64 {
    let gb = flux_plugin_api::setting::<String>("cache-limit")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(CACHE_LIMIT_GB);
    gb << 30
}

/// The rendered animations: `<data>/cache/frames`.
pub fn frames_dir() -> String {
    format!("{}/cache/frames", storage::dir())
}

/// `setTdlibParameters` of the client `key`.
fn parameters(key: &str, database_key: &str) -> Value {
    let language = host::i18n::language();
    Parameters {
        folder: &folder(key),
        database_key,
        language: &language,
        test_dc: config::TEST_DC,
        api_id: config::API_ID,
        api_hash: config::API_HASH,
    }
    .request()
}

fn phone_request(phone: &str) -> Value {
    json!({"@type": "setAuthenticationPhoneNumber", "phone_number": phone, "settings": null})
}

fn qr_request() -> Value {
    json!({"@type": "requestQrCodeAuthentication", "other_user_ids": []})
}

/// The account's folder: its database and files.
fn folder(key: &str) -> String {
    format!("{}/accounts/{key}", storage::dir())
}

fn remove_folder(key: &str) {
    let path = folder(key);
    if let Err(err) = std::fs::remove_dir_all(&path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        log::warn(&format!("Couldn't remove {path}: {err}"));
    }
}

/// The secret with the database's encryption key of the account `key`.
fn secret_key(key: &str) -> String {
    format!("database/{key}")
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    if let Err(err) = getrandom::fill(&mut bytes) {
        log::error(&format!("No randomness: {err}"));
    }
    bytes
}

/// A new account's key: "a" and eight hex digits.
fn new_key() -> String {
    let bytes: [u8; 4] = random();
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("a{hex}")
}

/// A new database encryption key: 32 random bytes, base64.
fn database_key() -> String {
    base64::engine::general_purpose::STANDARD.encode(random::<32>())
}
