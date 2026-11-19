//! What the service needs from Flux, as one trait: the network and timers (the transport), the
//! plugin's data, secrets and settings, the strings, Claude for remote work (`claude-remote`) and
//! the user's tasks (`tasks`).
//! [`SdkHost`] is the real one, over the SDK; the tests have a fake ([`crate::testing`]), so the
//! bot's logic runs natively without Flux.

use flux_plugin_api::remote::{
    self, Awake, Mode, Project, RegionState, Response, RollbackCheck, Session, SystemState,
    TranscriptItem, UsageLimits,
};
use flux_plugin_api::settings::Tone;
use flux_plugin_api::tasks::{self, Task, TaskRun, TaskStarted};
use serde_json::Value;

use crate::model::ToWindow;

/// A new session, as the bot asks for it ([`Host::new_session`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewSession {
    /// The project's folder, absolute.
    pub project: String,
    pub title: Option<String>,
    /// The first message; none — nothing is sent.
    pub prompt: Option<String>,
    /// Chips, by (label, Markdown Claude reads).
    pub attachments: Vec<(String, String)>,
    /// Pictures: absolute paths in the plugin's data folder.
    pub images: Vec<String>,
    pub mode: Option<Mode>,
}

pub trait Host {
    // --- The network and time ---
    /// Starts a POST with a JSON body; the answer comes as `http-*` events with the returned id.
    fn http_post(&mut self, url: &str, body: Vec<u8>, timeout_ms: u32) -> u64;
    /// Starts a POST with a body of this content type (an upload: `multipart/form-data`).
    fn http_post_raw(
        &mut self,
        url: &str,
        content_type: &str,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> u64;
    /// Starts a GET (a file's download): its bytes come as `http-*` events.
    fn http_get(&mut self, url: &str, timeout_ms: u32) -> u64;
    fn http_cancel(&mut self, request: u64);
    /// A timer firing once; its id comes back as `timer`.
    fn timer(&mut self, ms: u32) -> u64;
    fn cancel_timer(&mut self, timer: u64);
    /// Milliseconds since 1970.
    fn now_ms(&self) -> u64;
    fn random(&mut self, bytes: usize) -> Vec<u8>;

    // --- The plugin's data ---
    /// The plugin's data folder, absolute: `std::fs` reads and writes there (the photos and files
    /// from the phone).
    fn data_dir(&self) -> String;
    fn load(&self, key: &str) -> Option<Value>;
    /// Writes a key; none — removes it.
    fn save(&mut self, key: &str, value: Option<&Value>);
    fn secret(&self, key: &str) -> Option<String>;
    fn setting(&self, key: &str) -> Option<Value>;
    /// Writes one of the plugin's settings, as the user would on its page (`/settings`); Flux
    /// doesn't tell the plugin `settings-changed` about its own write.
    fn set_setting(&mut self, key: &str, value: &Value) -> Result<(), String>;
    /// A `status` line on the settings page; none — cleared.
    fn status(&mut self, key: &str, line: Option<(&str, Tone, bool)>);
    /// The text in Flux's interface language (`locales/<language>.toml`).
    fn tr(&self, text: &str) -> String;
    fn log(&mut self, text: &str);
    /// A notification in the window the user works in.
    fn notify(&mut self, title: &str);
    /// How far the user's time zone is ahead of UTC at `at` (seconds since 1970), in seconds.
    fn utc_offset(&self, at: i64) -> i32;
    /// Tells every window's instance.
    fn broadcast(&mut self, message: &ToWindow);

    // --- Claude for remote work ---
    fn watch(&mut self, on: bool) -> Result<(), String>;
    fn sessions(&mut self) -> Result<Vec<Session>, String>;
    fn transcript(&mut self, session: &str, count: u32) -> Result<Vec<TranscriptItem>, String>;
    fn send(&mut self, session: &str, text: &str, images: &[String]) -> Result<(), String>;
    fn answer(&mut self, session: &str, request: &str, response: &Response) -> Result<(), String>;
    fn interrupt(&mut self, session: &str) -> Result<(), String>;
    fn set_mode(&mut self, session: &str, mode: Mode) -> Result<(), String>;
    /// `/rename`: the session's title in Flux (its tab); the topic follows its title.
    fn rename(&mut self, session: &str, title: &str) -> Result<(), String>;
    /// `/delete`: closes the session as the Claude console's Close does — it leaves «Now» for the
    /// history.
    fn close(&mut self, session: &str) -> Result<(), String>;
    /// Opens a session in a project (its window opens in the background): the session's id.
    fn new_session(&mut self, spec: &NewSession) -> Result<String, String>;
    fn projects(&mut self) -> Result<Vec<Project>, String>;
    fn set_awake(&mut self, awake: Awake) -> Result<(), String>;
    fn system_status(&mut self) -> Result<SystemState, String>;
    /// The subscription's usage (Flux may ask the CLI: it can take a while).
    fn limits(&mut self) -> Result<UsageLimits, String>;
    /// The country check; `check_again` — asks again (the user changed the network).
    fn region(&mut self, check_again: bool) -> Result<RegionState, String>;
    /// A turn's unified diff.
    fn turn_diff(&mut self, session: &str, turn: u32) -> Result<String, String>;
    /// What rolling a turn back would do.
    fn check_rollback(&mut self, session: &str, turn: u32) -> Result<RollbackCheck, String>;
    /// Rolls a turn back (the user was asked).
    fn rollback(&mut self, session: &str, turn: u32) -> Result<(), String>;

    // --- The user's tasks (a tracker's plugin gives them) ---
    /// The user's open tasks; an error — no source answered (none is installed).
    fn my_tasks(&mut self) -> Result<Vec<Task>, String>;
    /// The tasks the user's words find, as the tracker's search finds them.
    fn search_tasks(&mut self, words: &str) -> Result<Vec<Task>, String>;
    /// One task with its text and its source's actions; none — no source knows it.
    fn task(&mut self, key: &str) -> Result<Option<Task>, String>;
    /// Runs a source's action on a task in a project, as at the computer but asking nothing; waits
    /// for it (up to about a minute): the session it opened and its notes.
    fn run_task(&mut self, run: &TaskRun) -> Result<TaskStarted, String>;
}

/// A translated template with `{0}`, `{1}`… filled in.
pub fn trf(host: &dyn Host, template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut text = host.tr(template);
    for (index, arg) in args.iter().enumerate() {
        text = text.replace(&format!("{{{index}}}"), &arg.to_string());
    }
    text
}

/// Flux, through the SDK.
pub struct SdkHost;

impl Host for SdkHost {
    fn http_post(&mut self, url: &str, body: Vec<u8>, timeout_ms: u32) -> u64 {
        flux_plugin_api::http::post(url)
            .header("Content-Type", "application/json")
            .body(body)
            .timeout(std::time::Duration::from_millis(u64::from(timeout_ms)))
            .start()
    }

    fn http_post_raw(
        &mut self,
        url: &str,
        content_type: &str,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> u64 {
        flux_plugin_api::http::post(url)
            .header("Content-Type", content_type)
            .body(body)
            .timeout(std::time::Duration::from_millis(u64::from(timeout_ms)))
            .start()
    }

    fn http_get(&mut self, url: &str, timeout_ms: u32) -> u64 {
        flux_plugin_api::http::get(url)
            .timeout(std::time::Duration::from_millis(u64::from(timeout_ms)))
            .start()
    }

    fn http_cancel(&mut self, request: u64) {
        flux_plugin_api::http::cancel(request);
    }

    fn timer(&mut self, ms: u32) -> u64 {
        flux_plugin_api::timers::after(std::time::Duration::from_millis(u64::from(ms)))
    }

    fn cancel_timer(&mut self, timer: u64) {
        flux_plugin_api::timers::cancel(timer);
    }

    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or_default()
    }

    fn random(&mut self, bytes: usize) -> Vec<u8> {
        let mut out = vec![0; bytes];
        if getrandom::fill(&mut out).is_err() {
            // No randomness from the system: the clock, mixed — still unguessable enough for a
            // code that lives minutes.
            let mut seed = self.now_ms() ^ 0x9E37_79B9_7F4A_7C15;
            for byte in &mut out {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *byte = seed as u8;
            }
        }
        out
    }

    fn data_dir(&self) -> String {
        flux_plugin_api::storage::dir()
    }

    fn load(&self, key: &str) -> Option<Value> {
        flux_plugin_api::storage::get(key)
    }

    fn save(&mut self, key: &str, value: Option<&Value>) {
        match value {
            Some(value) => flux_plugin_api::storage::set(key, value),
            None => flux_plugin_api::storage::remove(key),
        }
    }

    fn secret(&self, key: &str) -> Option<String> {
        flux_plugin_api::secrets::get(key).filter(|secret| !secret.trim().is_empty())
    }

    fn setting(&self, key: &str) -> Option<Value> {
        flux_plugin_api::setting(key)
    }

    fn set_setting(&mut self, key: &str, value: &Value) -> Result<(), String> {
        flux_plugin_api::settings::set(key, value)
    }

    fn status(&mut self, key: &str, line: Option<(&str, Tone, bool)>) {
        match line {
            Some((text, _, true)) => flux_plugin_api::settings::busy(key, text),
            Some((text, tone, false)) => flux_plugin_api::settings::status(key, text, tone),
            None => flux_plugin_api::settings::clear_status(key),
        }
    }

    fn tr(&self, text: &str) -> String {
        flux_plugin_api::tr(text)
    }

    fn log(&mut self, text: &str) {
        flux_plugin_api::log::info(text);
    }

    fn notify(&mut self, title: &str) {
        flux_plugin_api::notify::info(title);
    }

    fn utc_offset(&self, at: i64) -> i32 {
        flux_plugin_api::system::utc_offset(at)
    }

    fn broadcast(&mut self, message: &ToWindow) {
        let _ = flux_plugin_api::service::broadcast_json(message);
    }

    fn watch(&mut self, on: bool) -> Result<(), String> {
        if on {
            remote::watch()
        } else {
            remote::unwatch()
        }
    }

    fn sessions(&mut self) -> Result<Vec<Session>, String> {
        remote::sessions()
    }

    fn transcript(&mut self, session: &str, count: u32) -> Result<Vec<TranscriptItem>, String> {
        remote::transcript(session, count)
    }

    fn send(&mut self, session: &str, text: &str, images: &[String]) -> Result<(), String> {
        remote::send_with_images(session, text, images)
    }

    fn answer(&mut self, session: &str, request: &str, response: &Response) -> Result<(), String> {
        remote::answer(session, request, response)
    }

    fn interrupt(&mut self, session: &str) -> Result<(), String> {
        remote::interrupt(session)
    }

    fn set_mode(&mut self, session: &str, mode: Mode) -> Result<(), String> {
        remote::set_mode(session, mode)
    }

    fn rename(&mut self, session: &str, title: &str) -> Result<(), String> {
        remote::rename(session, title)
    }

    fn close(&mut self, session: &str) -> Result<(), String> {
        remote::close(session)
    }

    fn new_session(&mut self, spec: &NewSession) -> Result<String, String> {
        let mut new = remote::new_session(&spec.project);
        if let Some(title) = &spec.title {
            new = new.title(title);
        }
        if let Some(prompt) = &spec.prompt {
            new = new.prompt(prompt);
        }
        for (label, markdown) in &spec.attachments {
            new = new.attach(label, markdown);
        }
        for image in &spec.images {
            new = new.image(image);
        }
        if let Some(mode) = spec.mode {
            new = new.mode(mode);
        }
        new.start()
    }

    fn projects(&mut self) -> Result<Vec<Project>, String> {
        remote::projects()
    }

    fn set_awake(&mut self, awake: Awake) -> Result<(), String> {
        remote::set_awake(awake)
    }

    fn system_status(&mut self) -> Result<SystemState, String> {
        remote::system_status()
    }

    fn limits(&mut self) -> Result<UsageLimits, String> {
        remote::limits()
    }

    fn region(&mut self, check_again: bool) -> Result<RegionState, String> {
        remote::region(check_again)
    }

    fn turn_diff(&mut self, session: &str, turn: u32) -> Result<String, String> {
        remote::turn_diff(session, Some(turn))
    }

    fn check_rollback(&mut self, session: &str, turn: u32) -> Result<RollbackCheck, String> {
        remote::check_rollback(session, Some(turn))
    }

    fn rollback(&mut self, session: &str, turn: u32) -> Result<(), String> {
        remote::rollback(session, Some(turn))
    }

    fn my_tasks(&mut self) -> Result<Vec<Task>, String> {
        tasks::mine()
    }

    fn search_tasks(&mut self, words: &str) -> Result<Vec<Task>, String> {
        tasks::search(words)
    }

    fn task(&mut self, key: &str) -> Result<Option<Task>, String> {
        tasks::get(key)
    }

    fn run_task(&mut self, run: &TaskRun) -> Result<TaskStarted, String> {
        tasks::run_with(run)
    }
}
