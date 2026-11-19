//! `fluxgram.cache` (README.md → Plugin → program): a cache folder of the plugin's — the stickers'
//! files of frames — measured, trimmed to a limit oldest-used first, or emptied. A WebAssembly
//! plugin may not list or delete files: the program does it for it.
//!
//! "Used" is a file's modification time: a render writes it, and the plugin has the files it
//! takes again touched (`touch`: set to now) — the least recently used go first. Only what is in
//! the folder counts: its files and its folders' files, symbolic links neither followed nor
//! removed. The work runs on a thread of its own, so the plugin's and TDLib's lines aren't held up
//! by a folder of thousands of files.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::SystemTime;

use serde_json::Value;

use crate::relay::InputError;

/// How deep folders in the cache folder are looked into.
const MAX_DEPTH: usize = 8;

/// What the plugin asked for.
#[derive(Debug, PartialEq)]
pub struct Cache {
    /// The folder: a whole path, without `..`.
    pub dir: String,
    /// Its most bytes: older files are removed until it holds no more; none — only measured.
    pub limit: Option<u64>,
    /// Every file is removed (with `limit` 0).
    pub clear: bool,
    /// Files of the folder taken again: touched first, so they go last.
    pub touch: Vec<String>,
}

/// A file of the folder: where, how big, when last used.
struct Entry {
    path: PathBuf,
    size: u64,
    used: SystemTime,
}

/// Is `dir` a folder the request may name: a whole path, nothing like `..` in it.
pub fn valid_dir(dir: &str) -> bool {
    let path = Path::new(dir);
    path.is_absolute()
        && path
            .components()
            .all(|part| !matches!(part, Component::ParentDir | Component::CurDir))
}

/// Does the work on a thread of its own; the answer goes out through `write` (a whole line).
pub fn submit(cache: Cache, extra: Option<Value>, write: fn(&[u8])) {
    let spawned = thread::Builder::new().name("cache".into()).spawn({
        let extra = extra.clone();
        move || write(answer(&run(&cache), extra.as_ref()).as_bytes())
    });
    if let Err(error) = spawned {
        let message = format!("Can't trim the cache: no thread ({error})");
        write(InputError::new(message, extra.as_ref()).line().as_bytes());
    }
}

/// The answer line: `fluxgram.cacheSize`, or `fluxgram.error` with the reason.
pub fn answer(result: &Result<u64, String>, extra: Option<&Value>) -> String {
    match result {
        Ok(size) => {
            let mut line = format!("{{\"@type\":\"fluxgram.cacheSize\",\"size\":{size}");
            if let Some(extra) = extra {
                line.push_str(",\"@extra\":");
                line.push_str(&extra.to_string());
            }
            line.push_str("}\n");
            line
        }
        Err(message) => InputError::new(message.clone(), extra).line(),
    }
}

/// Touches, trims or empties the folder as asked: the bytes it holds then. A folder that isn't
/// there holds none.
pub fn run(cache: &Cache) -> Result<u64, String> {
    let dir = Path::new(&cache.dir);
    match fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(format!("Not a folder: {}", cache.dir)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("Can't read the cache: {error}")),
    }
    for file in &cache.touch {
        // Only the folder's own files; one gone or busy stays as it is.
        let path = Path::new(file);
        if path.starts_with(dir) && valid_dir(file) && is_file(path) {
            let _ = touch(path);
        }
    }

    let mut entries = Vec::new();
    list(dir, 0, &mut entries).map_err(|error| format!("Can't read the cache: {error}"))?;
    let mut total: u64 = entries.iter().map(|entry| entry.size).sum();
    let limit = if cache.clear { Some(0) } else { cache.limit };
    if let Some(limit) = limit {
        // The least recently used first; the same time — by path, for the same order each time.
        entries.sort_by(|a, b| a.used.cmp(&b.used).then_with(|| a.path.cmp(&b.path)));
        for entry in entries {
            if total <= limit {
                break;
            }
            // A file another program holds (Windows) stays, and counts.
            if fs::remove_file(&entry.path).is_ok() {
                total -= entry.size;
            }
        }
    }
    Ok(total)
}

/// The files under `dir`, its folders' too (up to [`MAX_DEPTH`]); links are skipped, not followed.
fn list(dir: &Path, depth: usize, entries: &mut Vec<Entry>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        // The entry's own type: a link is a link here, not what it points to.
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if depth < MAX_DEPTH {
                list(&entry.path(), depth + 1, entries)?;
            }
        } else if kind.is_file() {
            // Gone meanwhile: not counted.
            let Ok(meta) = entry.metadata() else { continue };
            entries.push(Entry {
                path: entry.path(),
                size: meta.len(),
                used: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(())
}

/// A file itself, not a link to one.
fn is_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

/// Sets a file's modification time to now. Windows: with the right to its attributes only, so a
/// file open in Flux is touched too.
fn touch(path: &Path) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
        options.access_mode(FILE_WRITE_ATTRIBUTES);
    }
    #[cfg(not(windows))]
    options.read(true);
    options.open(path)?.set_modified(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fluxgram-cache-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A file of `size` bytes last used `age` seconds ago.
    fn file(path: &Path, size: usize, age: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7; size]).unwrap();
        let used = SystemTime::now() - Duration::from_secs(age);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(used)
            .unwrap();
    }

    fn cache(dir: &Path, limit: Option<u64>, clear: bool, touch: &[&Path]) -> Cache {
        Cache {
            dir: dir.to_string_lossy().into_owned(),
            limit,
            clear,
            touch: touch
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        }
    }

    #[test]
    fn measures_trims_and_clears() {
        let dir = scratch("trim");
        let (a, b, c, d) = (
            dir.join("a.fxa"),
            dir.join("b.fxa"),
            dir.join("deep/c.fxa"),
            dir.join("d.fxa"),
        );
        file(&a, 100, 400);
        file(&b, 200, 300);
        file(&c, 300, 200);
        file(&d, 400, 100);

        // Only measured: nothing goes.
        assert_eq!(run(&cache(&dir, None, false, &[])), Ok(1000));
        assert!(a.exists());

        // Under 700: the oldest, a and b, go (1000 → 700).
        assert_eq!(run(&cache(&dir, Some(700), false, &[])), Ok(700));
        assert!(!a.exists() && !b.exists() && c.exists() && d.exists());

        // c taken again (touched): d is the oldest now.
        assert_eq!(run(&cache(&dir, Some(300), false, &[&c])), Ok(300));
        assert!(c.exists() && !d.exists());

        // Already under: nothing goes.
        assert_eq!(run(&cache(&dir, Some(5000), false, &[])), Ok(300));

        // Emptied; the folders stay.
        assert_eq!(run(&cache(&dir, Some(0), true, &[])), Ok(0));
        assert!(!c.exists() && dir.join("deep").is_dir());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_folder_or_a_file() {
        let dir = scratch("missing");
        assert_eq!(run(&cache(&dir.join("none"), Some(0), true, &[])), Ok(0));
        let text = dir.join("text");
        fs::write(&text, "x").unwrap();
        let error = run(&cache(&text, None, false, &[])).unwrap_err();
        assert!(error.starts_with("Not a folder"), "{error}");
        fs::remove_dir_all(&dir).ok();
    }

    /// Links are neither followed nor removed: what they point to, out of the folder, stays —
    /// and touching through one does nothing.
    #[cfg(unix)]
    #[test]
    fn links_stay_out() {
        let dir = scratch("links");
        let outside = scratch("links-outside");
        let kept = outside.join("kept.fxa");
        file(&kept, 500, 1000);
        file(&outside.join("deep/also.fxa"), 500, 1000);
        let inside = dir.join("cache");
        file(&inside.join("own.fxa"), 10, 50);
        std::os::unix::fs::symlink(&kept, inside.join("link.fxa")).unwrap();
        std::os::unix::fs::symlink(outside.join("deep"), inside.join("folder")).unwrap();
        let before = fs::metadata(&kept).unwrap().modified().unwrap();

        // Only its own file counts, and goes.
        let link = inside.join("link.fxa");
        assert_eq!(run(&cache(&inside, None, false, &[&link, &kept])), Ok(10));
        assert_eq!(fs::metadata(&kept).unwrap().modified().unwrap(), before);
        assert_eq!(run(&cache(&inside, Some(0), true, &[])), Ok(0));
        assert!(kept.exists() && outside.join("deep/also.fxa").exists());
        assert!(fs::symlink_metadata(&link).is_ok(), "the link itself stays");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn folders_the_request_may_name() {
        assert!(valid_dir("/a/cache/frames"));
        assert!(!valid_dir("cache/frames"));
        assert!(!valid_dir("/a/../b"));
        assert!(!valid_dir(""));
    }

    #[test]
    fn answers() {
        assert_eq!(
            answer(&Ok(1234), Some(&Value::from(5))),
            "{\"@type\":\"fluxgram.cacheSize\",\"size\":1234,\"@extra\":5}\n"
        );
        assert_eq!(
            answer(&Err("Not a folder: /a".into()), None),
            "{\"@type\":\"fluxgram.error\",\"message\":\"Not a folder: /a\"}\n"
        );
    }
}
