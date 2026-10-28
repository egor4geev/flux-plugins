//! The disk cache: what the windows show, in files of the plugin's folder (`data-dir`), so that a
//! start shows the last known tasks at once, before any request, and the project windows (a plugin
//! instance each) share what one of them has read. The storage of the API (`storage.json`) isn't
//! used for it: that's one file for all the windows, rewritten whole on every change.
//!
//! - One folder per account: `<data-dir>/cache/<account>/`, `<account>` — a hash of the
//!   credentials and the organization ([`account`]; never the token itself). Another account or
//!   organization is another folder; the others are deleted when one becomes current
//!   ([`forget_others`]), all of them at a sign-out.
//! - One file per key (`list-<hash of the query>.json`, `board-<board>.json`, `myself.json`…):
//!   `{"v": VERSION, "saved": <unix seconds>, "data": …}` — a file of another version is ignored.
//!   It's written whole to a temporary file of its own, then renamed over the old one: a reader
//!   never sees half a file; of two windows writing at once, the last one wins.
//! - Bounded: a value over [`MAX_BYTES`] isn't kept; lists and boards keep the newest
//!   [`MAX_FILES`] files of their kind ([`Cache::prune`]).
//!
//! The functions take the folder, so the tests run on a temporary one; [`root`] is the plugin's.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::auth::Auth;

/// The files' format: a file of another version is ignored (and replaced at the next write).
pub const VERSION: u32 = 1;
/// The largest file kept.
pub const MAX_BYTES: usize = 4 << 20;
/// Files of one kind (lists, boards) kept: the newest.
pub const MAX_FILES: usize = 12;

#[derive(Serialize)]
struct Envelope<'a, T: Serialize> {
    v: u32,
    saved: i64,
    data: &'a T,
}

#[derive(Deserialize)]
struct Read<T> {
    v: u32,
    saved: i64,
    data: T,
}

/// A value read from the cache, and when it was saved (unix seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Saved<T> {
    pub saved: i64,
    pub data: T,
}

/// The cache of one account; [`Cache::none`] keeps nothing (signed out, no organization).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cache {
    dir: Option<PathBuf>,
}

impl Cache {
    pub fn none() -> Cache {
        Cache { dir: None }
    }

    /// The account's folder under `root`.
    pub fn at(root: &Path, account: &str) -> Cache {
        Cache {
            dir: Some(root.join(account)),
        }
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("{name}.json")))
    }

    /// The value under `name`: none when there's no file, it's of another version or doesn't read
    /// as `T`.
    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Option<Saved<T>> {
        let bytes = fs::read(self.path(name)?).ok()?;
        let read: Read<T> = serde_json::from_slice(&bytes).ok()?;
        (read.v == VERSION).then_some(Saved {
            saved: read.saved,
            data: read.data,
        })
    }

    /// Keeps `value` under `name` as of `now`: atomically, nothing over [`MAX_BYTES`]. False — not
    /// kept (no account, too big, the disk).
    pub fn save<T: Serialize>(&self, name: &str, value: &T, now: i64) -> bool {
        let (Some(dir), Some(path)) = (&self.dir, self.path(name)) else {
            return false;
        };
        let Ok(json) = serde_json::to_vec(&Envelope {
            v: VERSION,
            saved: now,
            data: value,
        }) else {
            return false;
        };
        if json.len() > MAX_BYTES {
            let _ = fs::remove_file(&path);
            return false;
        }
        write_atomically(dir, &path, &json)
    }

    /// When the file under `name` last changed: another window wrote it when this changes.
    pub fn stamp(&self, name: &str) -> Option<SystemTime> {
        fs::metadata(self.path(name)?).ok()?.modified().ok()
    }

    /// Keeps the newest `keep` files whose names start with `prefix`.
    pub fn prune(&self, prefix: &str, keep: usize) {
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(prefix) && name.ends_with(".json")
            })
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        for (_, path) in files.into_iter().skip(keep) {
            let _ = fs::remove_file(path);
        }
    }
}

/// Writes `bytes` to `path` through a temporary file of this instance in `dir`, renamed over it.
pub fn write_atomically(dir: &Path, path: &Path, bytes: &[u8]) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = dir.join(format!(".{name}.{}.tmp", instance()));
    if fs::write(&temporary, bytes).is_err() {
        let _ = fs::remove_file(&temporary);
        return false;
    }
    if fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(&temporary);
        return false;
    }
    true
}

/// Deletes the accounts' folders under `root` but `keep` (none — all of them).
pub fn forget_others(root: &Path, keep: Option<&str>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if keep.is_some_and(|keep| entry.file_name().to_string_lossy() == keep) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

/// The plugin's cache folder: `<data-dir>/cache`.
pub fn root() -> PathBuf {
    PathBuf::from(flux_plugin_api::storage::dir()).join("cache")
}

/// The account's folder name: the request headers (the credentials, the organization) hashed;
/// none signed out or without an organization.
pub fn account(auth: &Auth) -> Option<String> {
    auth.organization()?;
    let headers = auth.headers()?;
    let text: Vec<String> = headers
        .iter()
        .map(|(name, value)| format!("{}={value}", name.to_ascii_lowercase()))
        .collect();
    Some(hash(&text.join("\n")))
}

/// A short stable hash as hex (FNV-1a): names of folders and files.
pub fn hash(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// This plugin instance's random id: its temporary files, its lease ([`crate::lead`]).
pub fn instance() -> String {
    thread_local! {
        static ID: String = {
            let mut bytes = [0u8; 8];
            if getrandom::fill(&mut bytes).is_err() {
                let nanos = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0);
                bytes = nanos.to_le_bytes();
            }
            format!("{:016x}", u64::from_le_bytes(bytes))
        };
    }
    ID.with(Clone::clone)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A fresh temporary folder of a test.
    pub fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flux-yandex-{name}-{}", instance()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn keeps_values_by_key_and_version() {
        let root = scratch("cache-values");
        let cache = Cache::at(&root, "acc");
        assert_eq!(cache.load::<Vec<String>>("list-1"), None);
        assert!(cache.save("list-1", &vec!["TRACK-1".to_string()], 100));
        let saved = cache.load::<Vec<String>>("list-1").unwrap();
        assert_eq!(
            saved,
            Saved {
                saved: 100,
                data: vec!["TRACK-1".to_string()]
            }
        );
        assert!(cache.stamp("list-1").is_some());
        // Another shape doesn't read; another version is ignored.
        assert_eq!(cache.load::<u32>("list-1"), None);
        fs::write(
            root.join("acc/list-2.json"),
            r#"{"v": 999, "saved": 1, "data": []}"#,
        )
        .unwrap();
        assert_eq!(cache.load::<Vec<String>>("list-2"), None);
        // A broken file is no value.
        fs::write(root.join("acc/list-3.json"), "{\"v\": 1, \"sav").unwrap();
        assert_eq!(cache.load::<Vec<String>>("list-3"), None);
        // No temporary files left behind.
        let left: Vec<_> = fs::read_dir(root.join("acc"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(left.is_empty());
        // Signed out: nothing is kept.
        assert!(!Cache::none().save("list-1", &1, 1));
        assert_eq!(Cache::none().load::<u32>("list-1"), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bounded_and_pruned() {
        let root = scratch("cache-bounds");
        let cache = Cache::at(&root, "acc");
        let huge = "x".repeat(MAX_BYTES + 1);
        assert!(!cache.save("list-huge", &huge, 1));
        assert_eq!(cache.load::<String>("list-huge"), None);
        for index in 0..5 {
            assert!(cache.save(&format!("list-{index}"), &index, index));
            // Distinct modification times.
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        cache.save("myself", &1, 1);
        cache.prune("list-", 2);
        let mut names: Vec<String> = fs::read_dir(root.join("acc"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["list-3.json", "list-4.json", "myself.json"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn another_account_forgets_the_others() {
        let root = scratch("cache-accounts");
        Cache::at(&root, "a").save("myself", &1, 1);
        Cache::at(&root, "b").save("myself", &2, 1);
        forget_others(&root, Some("b"));
        assert_eq!(Cache::at(&root, "a").load::<u32>("myself"), None);
        assert_eq!(Cache::at(&root, "b").load::<u32>("myself").unwrap().data, 2);
        forget_others(&root, None);
        assert_eq!(Cache::at(&root, "b").load::<u32>("myself"), None);
        assert_ne!(hash("OAuth a"), hash("OAuth b"));
        assert_eq!(hash("x").len(), 16);
        let _ = fs::remove_dir_all(root);
    }
}
