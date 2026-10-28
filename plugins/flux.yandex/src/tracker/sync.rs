//! The Tracker's start and its cache ([`crate::cache`]): the window shows what the last run (or
//! another window) read at once, before any request; then the network work runs in the
//! background as short steps — one request each, the window drawn between them, the visible list
//! first — and what it reads goes back to the cache. Each window checks the cache's files every
//! few seconds: a list another window has just read, the leader's news of my tasks.

use std::collections::VecDeque;
use std::time::Duration;

use flux_plugin_api::{log, timers};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{FieldMeta, Issue, Queue};
use super::poll::{NEWS, News};
use super::{Tracker, api, links};
use crate::auth::Auth;
use crate::cache::{self, Cache, MAX_FILES};
use crate::client;
use crate::time;

/// A step of the background work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// The list the window shows.
    List,
    /// Who the user is.
    Myself,
    /// The queues: links to tasks, the filter's menu.
    Queues,
    /// The leader's poll.
    Poll,
}

/// The first step waits a moment: the window shows the cached list first.
const FIRST_STEP: Duration = Duration::from_millis(300);
/// Between two steps: the user's clicks come in between.
const STEP_PAUSE: Duration = Duration::from_millis(30);
/// Who the user is and the queues are read again after this long (seconds).
const DICTIONARY_AGE: i64 = 12 * 3600;
/// A shown list is read again after this long at least (seconds; `poll-minutes` if longer).
const LIST_AGE: i64 = 5 * 60;
/// A list that failed is tried again after this long (seconds).
const LIST_RETRY: i64 = 60;

/// The cache's files.
pub(super) const MYSELF: &str = "myself";
const QUEUES: &str = "queues";
const FIELDS: &str = "fields";

/// A list's file: its query (the hash in the name could collide), the tasks, how many in all.
#[derive(Serialize, Deserialize)]
pub(super) struct CachedList {
    pub query: String,
    pub issues: Vec<Issue>,
    pub total: u32,
}

/// The file of a list: `list-<hash of the query>`.
pub(super) fn list_name(query: &str) -> String {
    format!("list-{}", cache::hash(query))
}

/// The background work of a window.
#[derive(Default)]
pub(super) struct Chain {
    steps: VecDeque<Step>,
    timer: Option<u64>,
}

impl Chain {
    pub fn timer(&self) -> Option<u64> {
        self.timer
    }

    pub fn has(&self, step: Step) -> bool {
        self.steps.contains(&step)
    }

    pub fn stop(&mut self) {
        self.steps.clear();
        if let Some(timer) = self.timer.take() {
            timers::cancel(timer);
        }
    }

    fn schedule(&mut self, delay: Duration) {
        if self.timer.is_none() && !self.steps.is_empty() {
            self.timer = Some(timers::after(delay));
        }
    }
}

impl Tracker {
    /// Takes the account of `auth`: its cache, what's kept there. Another account than before —
    /// what was known is forgotten, the other accounts' files deleted (all of them at a
    /// sign-out). Returns whether the account changed.
    pub(super) fn use_account(&mut self, auth: &Auth) -> bool {
        let account = cache::account(auth);
        if self.started && account == self.account {
            return false;
        }
        let signed_out = self.started && self.account.is_some() && account.is_none();
        self.started = true;
        self.account = account.clone();
        self.forget();
        let root = cache::root();
        match &account {
            Some(account) => {
                cache::forget_others(&root, Some(account));
                self.cache = Cache::at(&root, account);
            }
            None => {
                if signed_out {
                    cache::forget_others(&root, None);
                }
                self.cache = Cache::none();
            }
        }
        self.load_cached();
        true
    }

    /// What the cache has: who the user is, the queues, the fields, the list, the poll's baseline.
    fn load_cached(&mut self) {
        if let Some(saved) = self.cache.load::<Value>(MYSELF) {
            self.me = api::read_myself(&saved.data);
            self.me_saved = saved.saved;
        }
        if let Some(saved) = self.cache.load::<Vec<Queue>>(QUEUES) {
            self.queues = saved.data;
            self.queues_saved = saved.saved;
            links::set(&self.queues);
        }
        if let Some(saved) = self.cache.load::<Vec<FieldMeta>>(FIELDS) {
            self.field_meta = Some(saved.data);
        }
        self.load_cached_list();
        self.poll.refresh(&self.cache);
        self.news_stamp = self.cache.stamp(NEWS);
        self.news_seen = self.cache.load::<News>(NEWS).map_or(0, |news| news.data.at);
    }

    /// The list of the filter from the cache, if it's there (and newer than the one shown).
    pub(super) fn load_cached_list(&mut self) -> bool {
        let query = self.filter_query();
        let name = list_name(&query);
        let stamp = self.cache.stamp(&name);
        let Some(saved) = self
            .cache
            .load::<CachedList>(&name)
            .filter(|saved| saved.data.query == query)
        else {
            return false;
        };
        if self.list.query == query && saved.saved < self.list.saved {
            self.list.stamp = stamp;
            return false;
        }
        self.list.issues = saved.data.issues;
        self.list.total = saved.data.total;
        self.list.query = query;
        self.list.saved = saved.saved;
        self.list.loaded_at = Some(saved.saved);
        self.list.error = None;
        // Another window read it: what failed here is past.
        self.list.problem = None;
        self.list.next_try = 0;
        self.list.stamp = stamp;
        // As fresh as it was read: its age decides ([`Tracker::list_due`]).
        self.list.stale = false;
        true
    }

    /// The list just read, into the cache (the newest lists kept).
    pub(super) fn save_list(&mut self) {
        let name = list_name(&self.list.query);
        let list = CachedList {
            query: self.list.query.clone(),
            issues: self.list.issues.clone(),
            total: self.list.total,
        };
        if self.cache.save(&name, &list, self.list.saved) {
            self.list.stamp = self.cache.stamp(&name);
            self.cache.prune("list-", MAX_FILES);
        }
    }

    /// The fields of tasks, read: into the cache.
    pub(super) fn save_fields(&self, metas: &[FieldMeta]) {
        self.cache.save(FIELDS, &metas, time::now());
    }

    /// The start's work, after a moment: the list (when the window shows it and it isn't
    /// fresh), who the user is, the queues.
    pub(super) fn start(&mut self) {
        let now = time::now();
        if self.list_due(now) {
            self.queue(Step::List);
        }
        if self.me.is_none() || now - self.me_saved > DICTIONARY_AGE {
            self.queue(Step::Myself);
        }
        if self.queues.is_empty() || now - self.queues_saved > DICTIONARY_AGE {
            self.queue(Step::Queues);
        }
        self.chain.schedule(FIRST_STEP);
    }

    /// Adds a step at the end, unless it's waiting already.
    pub(super) fn queue(&mut self, step: Step) {
        if !self.chain.has(step) {
            self.chain.steps.push_back(step);
        }
        self.chain.schedule(STEP_PAUSE);
    }

    /// The visible list first.
    pub(super) fn queue_list_first(&mut self) {
        self.chain.steps.retain(|step| *step != Step::List);
        self.chain.steps.push_front(Step::List);
        self.chain.schedule(STEP_PAUSE);
    }

    /// The list wants reading: stale, of another query, old — not before a failed one may be
    /// tried again.
    pub(super) fn list_due(&self, now: i64) -> bool {
        if now < self.list.next_try {
            return false;
        }
        let age = LIST_AGE.max(self.poll.interval);
        self.list.stale
            || self.list.query != self.filter_query()
            || self.list.loaded_at.is_none()
            || now - self.list.saved >= age
    }

    /// The chain's timer: one step. Background work waits out a 429.
    pub(super) fn run_step(&mut self, auth: &Auth) {
        self.chain.timer = None;
        if let Some(wait) = client::backoff_left() {
            self.chain.timer = Some(timers::after(wait.max(STEP_PAUSE)));
            return;
        }
        let Some(step) = self.chain.steps.pop_front() else {
            return;
        };
        if auth.headers().is_some() && auth.organization().is_some() {
            let now = time::now();
            match step {
                Step::List => {
                    if self.shown && self.card.is_none() {
                        if self.list_due(now) {
                            self.load_list(auth);
                            self.render(auth);
                        }
                    } else {
                        // Read when it shows.
                        self.list.stale |= self.list_due(now);
                    }
                }
                Step::Myself => match api::myself_value(auth) {
                    Ok(value) => {
                        self.me = api::read_myself(&value);
                        self.me_saved = now;
                        self.cache.save(MYSELF, &value, now);
                    }
                    Err(err) => log::warn(&format!("Myself: {}", err.message())),
                },
                Step::Queues => match api::queues(auth) {
                    Ok(queues) => {
                        self.cache.save(QUEUES, &queues, now);
                        self.queues = queues;
                        self.queues_saved = now;
                        links::set(&self.queues);
                    }
                    Err(err) => log::warn(&format!("Queues: {}", err.message())),
                },
                Step::Poll => {
                    if self.leading {
                        self.poll_now(auth);
                    }
                }
            }
        }
        self.chain.schedule(STEP_PAUSE);
    }

    /// Every few seconds ([`crate::lead::TICK`]): the leader's poll when it's time; a list another
    /// window has read, the leader's news — from the cache; the shown list when it's old.
    pub fn tick(&mut self, leading: bool, auth: &Auth) {
        self.leading = leading;
        if auth.headers().is_none() || auth.organization().is_none() {
            return;
        }
        let now = time::now();
        self.poll.refresh(&self.cache);
        if leading && self.poll.due(now) && !self.chain.has(Step::Poll) {
            self.queue(Step::Poll);
        }
        // A list another window has read.
        let name = list_name(&self.filter_query());
        let stamp = self.cache.stamp(&name);
        if stamp.is_some() && stamp != self.list.stamp && self.load_cached_list() {
            self.list.stale = false;
            self.render(auth);
        }
        // The leader's news of my tasks: the list is stale unless it was read after them.
        let news = self.cache.stamp(NEWS);
        if news != self.news_stamp {
            self.news_stamp = news;
            if let Some(news) = self.cache.load::<News>(NEWS)
                && news.data.at > self.news_seen
            {
                self.news_seen = news.data.at;
                if self.list.saved < news.data.at {
                    self.list.stale = true;
                }
            }
        }
        if self.shown && self.card.is_none() && self.list_due(now) && !self.chain.has(Step::List) {
            self.queue(Step::List);
        }
    }

    /// The list failed: it stays stale and is read again at a tick (the tasks shown kept) — after
    /// Tracker's wait for a 429, a minute after anything else.
    pub(super) fn list_failed(&mut self, now: i64, err: &client::ClientError) {
        self.list.stale = true;
        let wait = match err {
            client::ClientError::TooMany => client::backoff_left()
                .map_or(2, |wait| wait.as_secs_f64().ceil() as i64)
                .max(1),
            _ => LIST_RETRY,
        };
        // The tick reads it again then.
        self.list.next_try = now + wait;
    }
}
