//! News by polling — Tracker doesn't push to a plugin on this Mac: every `poll-minutes` the tasks
//! assigned to me (new ones, status changes) and the ones I'm summoned to
//! (`"Pending reply from": me()`). The first poll after the start or a sign-in only learns what
//! is there; what the plugin itself changed isn't news. One search at a time.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use flux_plugin_api::{log, notify, timers, tr, trf};

use super::Tracker;
use super::api::{self, Search};
use crate::auth::Auth;
use crate::settings::{self, NOTIFY_ASSIGNED, NOTIFY_STATUS, NOTIFY_SUMMONED, POLL_MINUTES};

const MINE: &str = "Assignee: me() Resolution: empty()";
const SUMMONED: &str = "\"Pending reply from\": me() Resolution: empty()";

#[derive(Default)]
pub(super) struct Poll {
    timer: Option<u64>,
    /// The first poll is done: what comes now is news.
    known: bool,
    /// My tasks: key → status key.
    statuses: HashMap<String, String>,
    summoned: HashSet<String>,
    /// Changed by the plugin since the last poll: not news.
    quiet: HashSet<String>,
}

impl Poll {
    /// The timer by `poll-minutes` again.
    pub fn restart(&mut self) {
        if let Some(timer) = self.timer.take() {
            timers::cancel(timer);
        }
        let minutes = settings::number(POLL_MINUTES).clamp(1, 60) as u64;
        self.timer = Some(timers::every(Duration::from_secs(minutes * 60)));
    }

    /// Forgets what it knew (another account, another organization).
    pub fn reset(&mut self) {
        self.known = false;
        self.statuses.clear();
        self.summoned.clear();
        self.quiet.clear();
    }

    pub fn timer(&self) -> Option<u64> {
        self.timer
    }

    /// The plugin changed the task: its next status isn't news.
    pub fn quiet(&mut self, key: &str) {
        self.quiet.insert(key.to_string());
    }
}

impl Tracker {
    pub(super) fn poll_now(&mut self, auth: &Auth) {
        if auth.headers().is_none() || auth.organization().is_none() {
            return;
        }
        let notify_assigned = settings::on(NOTIFY_ASSIGNED);
        let notify_status = settings::on(NOTIFY_STATUS);
        let notify_summoned = settings::on(NOTIFY_SUMMONED);
        if notify_assigned || notify_status || !self.poll.known {
            match api::search(auth, Search::Query(MINE), api::PAGE) {
                Ok((issues, _)) => {
                    for issue in &issues {
                        let status = issue
                            .status
                            .as_ref()
                            .map(|status| status.key_or_id())
                            .unwrap_or_default();
                        let quiet = self.poll.quiet.contains(&issue.key);
                        match self.poll.statuses.insert(issue.key.clone(), status.clone()) {
                            None if self.poll.known && notify_assigned && !quiet => {
                                news(&trf("{0} is assigned to you", &[&issue.key]), &issue.summary, &issue.key);
                            }
                            Some(before) if before != status && notify_status && !quiet => {
                                news(
                                    &trf("{0}: {1}", &[&issue.key, &issue.status_name()]),
                                    &issue.summary,
                                    &issue.key,
                                );
                            }
                            _ => {}
                        }
                    }
                    let keys: HashSet<&str> = issues.iter().map(|issue| issue.key.as_str()).collect();
                    self.poll.statuses.retain(|key, _| keys.contains(key.as_str()));
                    if self.poll.known && !issues.is_empty() {
                        self.list.stale = true;
                    }
                }
                Err(err) => {
                    log::warn(&format!("Polling: {}", err.message()));
                    return;
                }
            }
        }
        if notify_summoned || !self.poll.known {
            match api::search(auth, Search::Query(SUMMONED), api::PAGE) {
                Ok((issues, _)) => {
                    let now: HashSet<String> = issues.iter().map(|issue| issue.key.clone()).collect();
                    if self.poll.known && notify_summoned {
                        for issue in issues.iter().filter(|issue| !self.poll.summoned.contains(&issue.key)) {
                            news(
                                &trf("You're summoned in {0}", &[&issue.key]),
                                &issue.summary,
                                &issue.key,
                            );
                        }
                    }
                    self.poll.summoned = now;
                }
                Err(err) => log::warn(&format!("Polling: {}", err.message())),
            }
        }
        self.poll.quiet.clear();
        self.poll.known = true;
        if self.shown && self.card.is_none() && self.list.stale {
            self.load_list(auth);
            self.render(auth);
        }
    }
}

/// A notification with Open — the task's card.
fn news(title: &str, summary: &str, key: &str) {
    notify::Notice::info(title)
        .body(summary)
        .action_with(&tr("Open"), "open-notified-task", key)
        .send();
}
