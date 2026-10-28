//! One poller for all the project windows. Flux runs an instance of the plugin per window, each
//! with its own timers: left alone, every window would ask Tracker for news, and Tracker answers
//! 429 to a second search at a time. So the instances elect a leader through a lease file in the
//! plugin's folder (`<data-dir>/lead.json`: the holder's id and its last heartbeat):
//!
//! - nobody holds it — the first instance to create the file (`create_new`) leads;
//! - the holder beats every [`BEAT`] — at its ticks, and between the requests of a long poll; one
//!   silent for longer than [`STALE`] is taken over (a temporary file renamed over the lease). The
//!   heartbeat doesn't depend on `poll-minutes`, nor does the stale time: a window that crashed, or
//!   a Flux quit without its plugins' `deactivate`, holds the polling up for a minute and a half;
//! - an instance leads only once it has held the lease at two ticks in a row — two that took a
//!   stale lease at the same moment see who won before either polls;
//! - the leader lets the lease go when its window closes (`deactivate`): another one takes it at
//!   its next tick.
//!
//! The leader polls Tracker and writes what it learns to the cache ([`crate::cache`]); the others
//! read the cache and send no notifications. The instance's lease is kept here ([`start`],
//! [`tick`], [`beat`], [`release`]): the poll beats through it.

use std::cell::RefCell;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How often an instance checks the lease (and the cache's files).
pub const TICK: Duration = Duration::from_secs(5);
/// How often the leader writes its heartbeat.
pub const BEAT: i64 = 15;
/// A holder silent this long (seconds) is gone: a few heartbeats, longer than one request may
/// block the plugin (20 s).
pub const STALE: i64 = 90;

thread_local! {
    /// This instance's lease, from the start.
    static LEASE: RefCell<Option<Lease>> = const { RefCell::new(None) };
}

/// The start: this instance takes part in the election (leading from its second tick at the
/// earliest).
pub fn start(now: i64) {
    let mut lease = Lease::of_plugin();
    lease.claim(now, STALE);
    LEASE.with(|current| *current.borrow_mut() = Some(lease));
}

/// A tick: whether this instance leads now.
pub fn tick(now: i64) -> bool {
    LEASE.with(|current| {
        current
            .borrow_mut()
            .as_mut()
            .is_some_and(|lease| lease.claim(now, STALE))
    })
}

/// The leader's heartbeat in the middle of a long piece of work (the poll's requests).
pub fn beat(now: i64) {
    LEASE.with(|current| {
        if let Some(lease) = current.borrow_mut().as_mut() {
            lease.keep(now);
        }
    })
}

/// The window closes: the lease goes to another instance.
pub fn release() {
    LEASE.with(|current| {
        if let Some(mut lease) = current.borrow_mut().take() {
            lease.release();
        }
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Holder {
    id: String,
    beat: i64,
}

/// What the lease file says.
enum Found {
    Missing,
    /// There, but not readable: being created, or left half written; how old (seconds).
    Broken(i64),
    Held(Holder),
}

/// This instance's view of the lease.
pub struct Lease {
    path: PathBuf,
    id: String,
    /// Held at the last claim.
    held: bool,
    /// Held at two claims in a row: this instance leads.
    leading: bool,
    last_beat: i64,
}

impl Lease {
    pub fn new(path: PathBuf, id: &str) -> Lease {
        Lease {
            path,
            id: id.to_string(),
            held: false,
            leading: false,
            last_beat: 0,
        }
    }

    /// The plugin's lease: `<data-dir>/lead.json`.
    pub fn of_plugin() -> Lease {
        let dir = PathBuf::from(flux_plugin_api::storage::dir());
        Lease::new(dir.join("lead.json"), &crate::cache::instance())
    }

    /// A tick: takes the lease, keeps it beating, or sees another holder. `stale` — seconds of
    /// silence after which a holder is gone. Returns whether this instance leads now.
    pub fn claim(&mut self, now: i64, stale: i64) -> bool {
        let held = match self.found(now) {
            Found::Held(holder) if holder.id == self.id => {
                if now - self.last_beat >= BEAT || now < self.last_beat {
                    self.write(now)
                } else {
                    true
                }
            }
            Found::Held(holder) if now - holder.beat <= stale && holder.beat <= now + stale => {
                false
            }
            // A stale holder, or a file left broken a while ago: take it over.
            Found::Held(_) => self.write(now),
            Found::Broken(age) if age > 10 => self.write(now),
            Found::Broken(_) => false,
            Found::Missing => self.create(now),
        };
        // Confirm: of two instances that wrote at once, the file names one.
        let held = held && matches!(self.found(now), Found::Held(holder) if holder.id == self.id);
        self.leading = held && self.held;
        self.held = held;
        self.leading
    }

    /// The leader's heartbeat between ticks, when it's due — only while the lease is still its
    /// own (one taken over isn't taken back).
    pub fn keep(&mut self, now: i64) {
        if !self.held || now - self.last_beat < BEAT {
            return;
        }
        match self.found(now) {
            Found::Held(holder) if holder.id == self.id => {
                self.write(now);
            }
            _ => {
                self.held = false;
                self.leading = false;
            }
        }
    }

    /// Lets the lease go, if it's this instance's.
    pub fn release(&mut self) {
        if let Found::Held(holder) = self.found(0)
            && holder.id == self.id
        {
            let _ = fs::remove_file(&self.path);
        }
        self.held = false;
        self.leading = false;
    }

    fn found(&self, now: i64) -> Found {
        let Ok(bytes) = fs::read(&self.path) else {
            return Found::Missing;
        };
        match serde_json::from_slice::<Holder>(&bytes) {
            Ok(holder) => Found::Held(holder),
            Err(_) => {
                let modified = fs::metadata(&self.path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |at| at.as_secs() as i64);
                Found::Broken(now - modified)
            }
        }
    }

    fn holder(&self, now: i64) -> Vec<u8> {
        serde_json::to_vec(&Holder {
            id: self.id.clone(),
            beat: now,
        })
        .unwrap_or_default()
    }

    /// The lease's first holder: only one instance creates the file.
    fn create(&mut self, now: i64) -> bool {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let created = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(&self.holder(now)));
        if created.is_ok() {
            self.last_beat = now;
        }
        created.is_ok()
    }

    /// The heartbeat, or a takeover: the whole file replaced.
    fn write(&mut self, now: i64) -> bool {
        let Some(dir) = self.path.parent() else {
            return false;
        };
        let written = crate::cache::write_atomically(dir, &self.path, &self.holder(now));
        if written {
            self.last_beat = now;
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::tests::scratch;

    #[test]
    fn one_leader_of_many_windows() {
        let dir = scratch("lead-one");
        let path = dir.join("lead.json");
        let mut a = Lease::new(path.clone(), "a");
        let mut b = Lease::new(path.clone(), "b");
        // The first tick creates the lease; leading starts at the second one.
        assert!(!a.claim(100, 240));
        assert!(!b.claim(100, 240));
        assert!(a.claim(105, 240));
        assert!(!b.claim(105, 240));
        // The leader beats; the other keeps waiting.
        assert!(a.claim(130, 240));
        assert!(!b.claim(300, 240));
        // The leader goes silent: taken over after the stale time, leading at the next tick.
        assert!(!b.claim(371, 240));
        assert!(b.claim(376, 240));
        // The old leader is back: it sees the new holder and steps down.
        assert!(!a.claim(377, 240));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_closed_window_lets_the_lease_go() {
        let dir = scratch("lead-release");
        let path = dir.join("lead.json");
        let mut a = Lease::new(path.clone(), "a");
        let mut b = Lease::new(path.clone(), "b");
        a.claim(10, 240);
        assert!(a.claim(15, 240));
        a.release();
        assert!(!path.exists());
        // Releasing someone else's lease doesn't remove it.
        assert!(!b.claim(16, 240));
        assert!(b.claim(21, 240));
        a.release();
        assert!(path.exists());
        assert!(b.claim(26, 240));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_leader_beats_through_a_long_poll() {
        let dir = scratch("lead-keep");
        let path = dir.join("lead.json");
        let mut a = Lease::new(path.clone(), "a");
        let mut b = Lease::new(path.clone(), "b");
        a.claim(0, STALE);
        assert!(a.claim(5, STALE));
        // A poll of a minute: its requests beat; the other window doesn't take over.
        for now in [20, 40, 60, 80] {
            a.keep(now);
        }
        assert!(!b.claim(95, STALE));
        // Silent past the stale time: taken over; the old leader's beat doesn't take it back.
        assert!(!b.claim(171, STALE));
        assert!(b.claim(176, STALE));
        a.keep(177);
        assert!(b.claim(181, STALE));
        assert!(!a.claim(182, STALE));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_broken_lease_is_taken_once_old() {
        let dir = scratch("lead-broken");
        let path = dir.join("lead.json");
        fs::write(&path, "").unwrap();
        let mut a = Lease::new(path.clone(), "a");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        // Just created by someone: wait.
        assert!(!a.claim(now, 240));
        // Left broken: taken.
        assert!(!a.claim(now + 60, 240));
        assert!(a.claim(now + 65, 240));
        let _ = fs::remove_dir_all(dir);
    }
}
