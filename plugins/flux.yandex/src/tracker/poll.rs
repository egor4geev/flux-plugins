//! News by polling — Tracker doesn't push to a plugin on this Mac. One window polls for all of
//! them (the leader, [`crate::lead`]) every `poll-minutes`: the tasks assigned to me (new ones,
//! status changes) and the ones I'm summoned to (`"Pending reply from": me()`).
//!
//! - Only what changed since the last poll: `Updated: >= now()-<N>m` added to the queries, with
//!   the fields a notification says (`fields=`). Every half hour, and when there's no baseline,
//!   the whole lists (page by page): the tasks that left them are forgotten. A server that refuses
//!   the clause gets the whole lists from then on.
//! - The baseline — my tasks' statuses, the tasks I'm summoned to, when they were polled — is in
//!   the cache: the first poll after a start compares with it (nothing missed while Flux was
//!   closed, no burst of old news); without one, or one older than a day, it only learns.
//! - What the plugin itself changed isn't news, in whichever window: the keys go to the cache's
//!   `quiet` file.
//! - A change in my tasks (any change, not only a status) marks the list stale, and the other
//!   windows hear of it through the cache's `news` file.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use flux_plugin_api::{log, notify, tr, trf};
use serde::{Deserialize, Serialize};

use super::Tracker;
use super::api::{self, Search};
use super::model::Issue;
use crate::auth::Auth;
use crate::cache::Cache;
use crate::client::ClientError;
use crate::settings::{self, NOTIFY_ASSIGNED, NOTIFY_STATUS, NOTIFY_SUMMONED, POLL_MINUTES};
use crate::time;

const MINE: &str = "Assignee: me() Resolution: empty()";
const SUMMONED: &str = "\"Pending reply from\": me() Resolution: empty()";
const ORDER: &str = "\"Sort by\": Updated DESC";
/// The cache's files: the baseline, the plugin's own changes, the news for the other windows.
pub(super) const BASELINE: &str = "poll";
const QUIET: &str = "quiet";
pub(super) const NEWS: &str = "news";
/// A baseline older than this (seconds) isn't compared with: the first poll only learns.
const BASELINE_AGE: i64 = 24 * 3600;
/// The whole lists are read again this often (seconds).
const RECONCILE: i64 = 30 * 60;
/// Pages of a whole list read at most.
const MAX_PAGES: u32 = 5;
/// Notifications of one poll; the rest are counted in one more.
const MAX_NEWS: usize = 5;
/// The changes since the last poll are asked with this much more (minutes): the clocks, a slow
/// request.
const MARGIN_MINUTES: i64 = 2;
/// The plugin's own changes are remembered this long (seconds).
const QUIET_AGE: i64 = 15 * 60;
/// A failed poll is tried again after this long at most (seconds).
const RETRY: i64 = 60;

/// What the last poll saw, kept in the cache.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Baseline {
    /// When the last poll started (unix seconds): the next one asks what changed since.
    pub at: i64,
    /// When the whole lists were read last.
    pub full_at: i64,
    /// My tasks: key → status key.
    pub statuses: BTreeMap<String, String>,
    pub summoned: BTreeSet<String>,
    /// My tasks are more than the pages read: a task not known isn't news of an assignment.
    pub partial: bool,
}

/// The other windows hear that my tasks changed at `at` (unix seconds).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct News {
    pub at: i64,
}

#[derive(Default)]
pub(super) struct Poll {
    /// `poll-minutes`, in seconds.
    pub interval: i64,
    pub baseline: Option<Baseline>,
    /// The baseline file as last read: another leader's poll changes it.
    stamp: Option<std::time::SystemTime>,
    /// The next poll not before (a failed one waits).
    pub next_try: i64,
    /// Changed by the plugin in this window since the last poll: not news.
    quiet: HashSet<String>,
    /// The server refused `Updated: >= now()-…`: the whole lists every time.
    incremental_failed: bool,
}

impl Poll {
    /// `poll-minutes` again.
    pub fn read_interval(&mut self) {
        self.interval = settings::number(POLL_MINUTES).clamp(1, 60) * 60;
    }

    /// Forgets what it knew (another account, another organization).
    pub fn reset(&mut self) {
        self.baseline = None;
        self.stamp = None;
        self.next_try = 0;
        self.quiet.clear();
    }

    /// The baseline of the cache, when it's newer than the one known (another window polled).
    pub fn refresh(&mut self, cache: &Cache) {
        let stamp = cache.stamp(BASELINE);
        if stamp.is_none() || stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        if let Some(saved) = cache.load::<Baseline>(BASELINE)
            && self
                .baseline
                .as_ref()
                .is_none_or(|known| saved.data.at > known.at)
        {
            self.baseline = Some(saved.data);
        }
    }

    /// Time for the leader's poll at `now`.
    pub fn due(&self, now: i64) -> bool {
        let last = self.baseline.as_ref().map_or(0, |baseline| baseline.at);
        now >= self.next_try && (now - last >= self.interval || now < last)
    }
}

/// What a notification is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Assigned,
    Status,
    Summoned,
}

/// A notification to send.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Notice {
    pub kind: Kind,
    pub key: String,
    pub summary: String,
    pub status: String,
}

/// What a poll's answers mean against the baseline.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Found {
    pub news: Vec<Notice>,
    /// Something in my tasks changed: the lists are stale.
    pub changed: bool,
}

/// The answers of a query: its tasks, whether there were more than the pages read, whether it
/// was the whole list (else what changed since the baseline).
pub(super) struct Answer {
    pub issues: Vec<Issue>,
    pub partial: bool,
    pub whole: bool,
}

/// Compares the answers with the baseline and brings it up to date. `known` — the baseline is
/// worth comparing with (else only learned); `quiet` — the plugin's own changes.
pub(super) fn compare(
    base: &mut Baseline,
    known: bool,
    mine: Option<&Answer>,
    summoned: Option<&Answer>,
    quiet: impl Fn(&str) -> bool,
) -> Found {
    let mut found = Found::default();
    let notice = |kind, issue: &Issue| Notice {
        kind,
        key: issue.key.clone(),
        summary: issue.summary.clone(),
        status: issue.status_name().to_string(),
    };
    if let Some(answer) = mine {
        // Anything updated since the last poll is a change of the lists.
        if !answer.whole && !answer.issues.is_empty() {
            found.changed = true;
        }
        for issue in &answer.issues {
            let status = issue
                .status
                .as_ref()
                .map(|status| status.key_or_id())
                .unwrap_or_default();
            match base.statuses.insert(issue.key.clone(), status.clone()) {
                None => {
                    found.changed = true;
                    if !base.partial && !quiet(&issue.key) {
                        found.news.push(notice(Kind::Assigned, issue));
                    }
                }
                Some(before) if before != status => {
                    found.changed = true;
                    if !quiet(&issue.key) {
                        found.news.push(notice(Kind::Status, issue));
                    }
                }
                _ => {}
            }
        }
        if answer.whole {
            if !answer.partial {
                let keys: HashSet<&str> = answer
                    .issues
                    .iter()
                    .map(|issue| issue.key.as_str())
                    .collect();
                let before = base.statuses.len();
                base.statuses.retain(|key, _| keys.contains(key.as_str()));
                found.changed |= base.statuses.len() != before;
            }
            base.partial = answer.partial;
        }
    }
    if let Some(answer) = summoned {
        for issue in &answer.issues {
            if base.summoned.insert(issue.key.clone()) {
                found.news.push(notice(Kind::Summoned, issue));
            }
        }
        if answer.whole && !answer.partial {
            let keys: HashSet<&str> = answer
                .issues
                .iter()
                .map(|issue| issue.key.as_str())
                .collect();
            base.summoned.retain(|key| keys.contains(key.as_str()));
        }
    }
    if !known {
        // Only learned: what's there isn't news.
        found = Found::default();
    }
    found
}

/// The query of a poll: what changed in the last `minutes`, or the whole list.
pub(super) fn poll_query(query: &str, since_minutes: Option<i64>) -> String {
    match since_minutes {
        Some(minutes) => format!("{query} Updated: >= now()-{minutes}m {ORDER}"),
        None => format!("{query} {ORDER}"),
    }
}

impl Tracker {
    /// The leader's poll: what changed in my tasks, the notifications, the news for the windows.
    pub(super) fn poll_now(&mut self, auth: &Auth) {
        if auth.headers().is_none() || auth.organization().is_none() {
            return;
        }
        let notify_assigned = settings::on(NOTIFY_ASSIGNED);
        let notify_status = settings::on(NOTIFY_STATUS);
        let notify_summoned = settings::on(NOTIFY_SUMMONED);
        let now = time::now();
        if !(notify_assigned || notify_status || notify_summoned) {
            // Nothing to tell: no polling; the next one (notifications on again) only learns.
            self.poll.baseline = None;
            self.poll.next_try = now + self.poll.interval;
            return;
        }
        self.poll.refresh(&self.cache);
        let known = self
            .poll
            .baseline
            .as_ref()
            .is_some_and(|base| now - base.at <= BASELINE_AGE && base.at <= now + 60);
        let mut base = match &self.poll.baseline {
            Some(base) if known => base.clone(),
            _ => Baseline::default(),
        };
        let whole = !known || now - base.full_at >= RECONCILE || self.poll.incremental_failed;
        let since = (!whole).then(|| ((now - base.at).max(0) + 59) / 60 + MARGIN_MINUTES);
        let answers = self
            .ask(auth, MINE, since)
            .and_then(|mine| Ok((mine, self.ask(auth, SUMMONED, since)?)));
        let (mine, summoned) = match answers {
            Ok(answers) => answers,
            Err(err) => {
                log::warn(&format!("Polling: {}", err.message()));
                self.poll.next_try = now + RETRY.min(self.poll.interval);
                return;
            }
        };
        let quiet = self.quiet_keys(base.at, now);
        let found = compare(&mut base, known, Some(&mine), Some(&summoned), |key| {
            quiet.contains(key)
        });
        base.at = now;
        if mine.whole && summoned.whole {
            base.full_at = now;
        }
        self.cache.save(BASELINE, &base, now);
        self.poll.stamp = self.cache.stamp(BASELINE);
        self.poll.baseline = Some(base);
        self.poll.next_try = now + self.poll.interval;
        self.poll.quiet.clear();
        let news: Vec<Notice> = found
            .news
            .into_iter()
            .filter(|notice| match notice.kind {
                Kind::Assigned => notify_assigned,
                Kind::Status => notify_status,
                Kind::Summoned => notify_summoned,
            })
            .collect();
        tell(&news);
        if found.changed {
            self.list.stale = true;
            // The list first, then the news: the other windows find it in the cache.
            if self.shown && self.card.is_none() {
                self.load_list(auth);
                self.render(auth);
            }
            self.cache.save(NEWS, &News { at: now }, now);
        }
    }

    /// A query of the poll: what changed in the last `since` minutes — the whole list when there
    /// are more changes than a page, or the server refuses the clause — or the whole list.
    fn ask(&mut self, auth: &Auth, query: &str, since: Option<i64>) -> Result<Answer, ClientError> {
        if let Some(minutes) = since.filter(|_| !self.poll.incremental_failed) {
            let changed = poll_query(query, Some(minutes));
            let answer = api::search(auth, Search::Query(&changed), api::PAGE, api::POLL_FIELDS);
            crate::lead::beat(time::now());
            match answer {
                Ok((issues, total)) if total as usize <= issues.len() => {
                    return Ok(Answer {
                        issues,
                        partial: false,
                        whole: false,
                    });
                }
                Ok(_) => {}
                Err(ClientError::BadRequest(words)) => {
                    log::warn(&format!(
                        "Polling: Tracker refused `Updated: >=` ({words}): whole lists"
                    ));
                    self.poll.incremental_failed = true;
                }
                Err(err) => return Err(err),
            }
        }
        let whole = poll_query(query, None);
        let mut issues = Vec::new();
        let mut partial = false;
        for page in 1..=MAX_PAGES {
            let (found, total) = api::search_page(
                auth,
                Search::Query(&whole),
                api::PAGE,
                page,
                api::POLL_FIELDS,
            )?;
            crate::lead::beat(time::now());
            let count = found.len();
            issues.extend(found);
            if issues.len() as u32 >= total || count < api::PAGE as usize {
                break;
            }
            partial = page == MAX_PAGES;
        }
        Ok(Answer {
            issues,
            partial,
            whole: true,
        })
    }

    /// The plugin changed the task: its next change isn't news — in this window and the leader.
    pub(super) fn quiet(&mut self, key: &str) {
        self.poll.quiet.insert(key.to_string());
        let now = time::now();
        let mut keys: HashMap<String, i64> = self
            .cache
            .load(QUIET)
            .map(|saved| saved.data)
            .unwrap_or_default();
        keys.retain(|_, at| now - *at <= QUIET_AGE);
        keys.insert(key.to_string(), now);
        self.cache.save(QUIET, &keys, now);
    }

    /// The keys the plugin changed since `since` (a little before: the clocks), in any window.
    fn quiet_keys(&self, since: i64, now: i64) -> HashSet<String> {
        let mut keys = self.poll.quiet.clone();
        if let Some(saved) = self.cache.load::<HashMap<String, i64>>(QUIET) {
            keys.extend(
                saved
                    .data
                    .into_iter()
                    .filter(|(_, at)| *at >= since - 120 && now - *at <= QUIET_AGE)
                    .map(|(key, _)| key),
            );
        }
        keys
    }
}

/// The notifications, each with Open — the task's card; past [`MAX_NEWS`], one that counts the
/// rest.
fn tell(news: &[Notice]) {
    for notice in news.iter().take(MAX_NEWS) {
        let title = match notice.kind {
            Kind::Assigned => trf("{0} is assigned to you", &[&notice.key]),
            Kind::Status => trf("{0}: {1}", &[&notice.key, &notice.status]),
            Kind::Summoned => trf("You're summoned in {0}", &[&notice.key]),
        };
        notify::Notice::info(&title)
            .body(&notice.summary)
            .action_with(&tr("Open"), "open-notified-task", &notice.key)
            .send();
    }
    if news.len() > MAX_NEWS {
        notify::Notice::info(&trf(
            "And {0} more changes in your tasks",
            &[&(news.len() - MAX_NEWS)],
        ))
        .send();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::model::Named;

    fn task(key: &str, status: &str) -> Issue {
        Issue {
            key: key.into(),
            summary: format!("Task {key}"),
            status: Some(Named {
                key: Some(status.into()),
                display: status.into(),
                ..Named::default()
            }),
            ..Issue::default()
        }
    }

    fn answer(issues: Vec<Issue>, whole: bool) -> Answer {
        Answer {
            issues,
            partial: false,
            whole,
        }
    }

    fn kinds(found: &Found) -> Vec<(Kind, &str)> {
        found
            .news
            .iter()
            .map(|notice| (notice.kind, notice.key.as_str()))
            .collect()
    }

    #[test]
    fn the_first_poll_only_learns() {
        let mut base = Baseline::default();
        let mine = answer(vec![task("A-1", "open"), task("A-2", "inProgress")], true);
        let summoned = answer(vec![task("A-3", "open")], true);
        let found = compare(&mut base, false, Some(&mine), Some(&summoned), |_| false);
        assert_eq!(found, Found::default());
        assert_eq!(base.statuses.len(), 2);
        assert!(base.summoned.contains("A-3"));
    }

    #[test]
    fn what_changed_since_the_baseline_is_news() {
        let mut base = Baseline::default();
        compare(
            &mut base,
            false,
            Some(&answer(
                vec![task("A-1", "open"), task("A-2", "open")],
                true,
            )),
            None,
            |_| false,
        );
        // Only what changed comes: a new task, a status, an edit that isn't news, the plugin's own.
        let mine = answer(
            vec![
                task("A-9", "open"),
                task("A-1", "inProgress"),
                task("A-2", "open"),
            ],
            false,
        );
        let summoned = answer(vec![task("A-5", "open")], false);
        let found = compare(&mut base, true, Some(&mine), Some(&summoned), |_| false);
        assert_eq!(
            kinds(&found),
            [
                (Kind::Assigned, "A-9"),
                (Kind::Status, "A-1"),
                (Kind::Summoned, "A-5")
            ]
        );
        assert!(found.changed);
        // The same again: nothing new; an edit still marks the lists.
        let found = compare(
            &mut base,
            true,
            Some(&answer(vec![task("A-2", "open")], false)),
            None,
            |_| false,
        );
        assert!(found.news.is_empty() && found.changed);
        // Nothing updated: no change at all.
        let found = compare(
            &mut base,
            true,
            Some(&answer(vec![], false)),
            Some(&answer(vec![], false)),
            |_| false,
        );
        assert_eq!(found, Found::default());
        // The plugin's own change isn't news.
        let found = compare(
            &mut base,
            true,
            Some(&answer(vec![task("A-2", "closed")], false)),
            None,
            |key| key == "A-2",
        );
        assert!(found.news.is_empty() && found.changed);
    }

    #[test]
    fn the_whole_list_forgets_what_left_it() {
        let mut base = Baseline::default();
        compare(
            &mut base,
            false,
            Some(&answer(
                vec![task("A-1", "open"), task("A-2", "open")],
                true,
            )),
            Some(&answer(vec![task("A-1", "open")], true)),
            |_| false,
        );
        // A-2 was reassigned, the reply in A-1 given: the whole lists drop them.
        let found = compare(
            &mut base,
            true,
            Some(&answer(vec![task("A-1", "open")], true)),
            Some(&answer(vec![], true)),
            |_| false,
        );
        assert!(found.news.is_empty() && found.changed);
        assert_eq!(base.statuses.keys().collect::<Vec<_>>(), ["A-1"]);
        assert!(base.summoned.is_empty());
        // Assigned again later, summoned again: news again.
        let found = compare(
            &mut base,
            true,
            Some(&answer(vec![task("A-2", "open")], false)),
            Some(&answer(vec![task("A-1", "open")], false)),
            |_| false,
        );
        assert_eq!(
            kinds(&found),
            [(Kind::Assigned, "A-2"), (Kind::Summoned, "A-1")]
        );
        // More tasks than the pages read: an unknown one isn't an assignment.
        let partial = Answer {
            issues: vec![task("A-1", "open")],
            partial: true,
            whole: true,
        };
        compare(&mut base, true, Some(&partial), None, |_| false);
        assert_eq!(base.statuses.len(), 2);
        let found = compare(
            &mut base,
            true,
            Some(&answer(vec![task("A-7", "open")], false)),
            None,
            |_| false,
        );
        assert!(found.news.is_empty() && found.changed);
    }

    #[test]
    fn asks_what_changed_since_the_last_poll() {
        assert_eq!(
            poll_query(MINE, Some(4)),
            "Assignee: me() Resolution: empty() Updated: >= now()-4m \"Sort by\": Updated DESC"
        );
        assert_eq!(
            poll_query(SUMMONED, None),
            "\"Pending reply from\": me() Resolution: empty() \"Sort by\": Updated DESC"
        );
        let mut poll = Poll {
            interval: 120,
            ..Poll::default()
        };
        assert!(poll.due(1_000));
        poll.baseline = Some(Baseline {
            at: 1_000,
            ..Baseline::default()
        });
        assert!(!poll.due(1_100));
        assert!(poll.due(1_120));
        poll.next_try = 1_200;
        assert!(!poll.due(1_150));
    }

    #[test]
    fn the_baseline_is_the_cache_s() {
        let root = crate::cache::tests::scratch("poll-baseline");
        let cache = Cache::at(&root, "acc");
        let mut poll = Poll {
            interval: 120,
            ..Poll::default()
        };
        poll.refresh(&cache);
        assert!(poll.baseline.is_none() && poll.due(500));
        // Another window (or the last run) polled at 500: this one compares with it.
        let mut statuses = BTreeMap::new();
        statuses.insert("A-1".to_string(), "open".to_string());
        let base = Baseline {
            at: 500,
            statuses,
            ..Baseline::default()
        };
        assert!(cache.save(BASELINE, &base, 500));
        poll.refresh(&cache);
        assert_eq!(poll.baseline.as_ref(), Some(&base));
        assert!(!poll.due(560) && poll.due(620));
        // An older baseline doesn't replace a newer one.
        poll.baseline = Some(Baseline {
            at: 900,
            ..Baseline::default()
        });
        std::thread::sleep(std::time::Duration::from_millis(15));
        cache.save(BASELINE, &base, 500);
        poll.refresh(&cache);
        assert_eq!(poll.baseline.as_ref().map(|base| base.at), Some(900));
        let _ = std::fs::remove_dir_all(root);
    }
}
