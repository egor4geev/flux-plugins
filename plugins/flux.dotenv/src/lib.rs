//! The checks of `.env` files (`flux.dotenv`): keys set twice, files with values that Git doesn't
//! ignore, keys of `.env.example` a file doesn't set and keys it doesn't list. They are published as
//! the plugin's problems — on the open documents as they are typed, on the project's other `.env`
//! files as they are on disk, so the Problems window lists them all.
//!
//! - The project's `.env` files are found when the plugin starts (the project root and two levels
//!   below it, without `node_modules` and hidden folders) and when one is saved.
//! - Git answers which of them it ignores and which are committed (`git check-ignore`,
//!   `git ls-files` — the `processes` permission): again when the repository changes.
//! - The templates (`.env.example`, `.env.sample`, `.env.template`) are what the other files of
//!   their folder are compared with; they hold no secrets, so Git may keep them.

mod env;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use flux_plugin_api::diagnostics::{self, Diagnostic};
use flux_plugin_api::host::{editors, project};
use flux_plugin_api::{
    Event, Plugin, Range, log, process, register_plugin, setting, timers, tr, trf,
};

/// Folders the project scan doesn't go into: dependencies and build output.
const SKIPPED: [&str; 8] = [
    "node_modules",
    "vendor",
    "dist",
    "build",
    "out",
    "target",
    "coverage",
    "bower_components",
];
/// How deep the scan goes below the project root: `apps/web/.env` is found.
const DEPTH: usize = 2;
/// The scan stops after this many folder entries: a huge project costs a moment, not the limit.
const MAX_ENTRIES: usize = 20_000;
/// The repository changes in bursts (a checkout, a commit): Git is asked once they settle.
const GIT_DELAY: Duration = Duration::from_millis(800);

/// How Git sees a `.env` file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GitState {
    Ignored,
    /// Neither ignored nor committed: the next `git add .` takes it.
    NotIgnored,
    /// Committed: its values are in the repository's history.
    Tracked,
    /// Not in a repository, or Git didn't answer.
    Unknown,
}

struct Dotenv {
    root: Option<PathBuf>,
    /// Open documents of the language: editor id → absolute path.
    open: HashMap<u64, String>,
    /// The project's `.env` files on disk (absolute paths), found by the scan or saved since.
    files: BTreeSet<String>,
    git: HashMap<String, GitState>,
    /// Files with the plugin's problems: those that have none now are cleared.
    published: HashSet<String>,
    git_timer: Option<u64>,
}

impl Plugin for Dotenv {
    fn new() -> Self {
        Dotenv {
            root: None,
            open: HashMap::new(),
            files: BTreeSet::new(),
            git: HashMap::new(),
            published: HashSet::new(),
            git_timer: None,
        }
    }

    fn activate(&mut self) {
        self.root = project::root().map(PathBuf::from);
        for editor in editors::list() {
            if let Some(path) = editor.path.filter(|path| is_dotenv_path(path)) {
                self.open.insert(editor.id, path);
            }
        }
        self.scan();
        self.ask_git();
        self.check_all();
    }

    fn deactivate(&mut self) {
        diagnostics::clear();
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::EditorOpened(editor) => {
                if let Some(path) = editor.path.filter(|path| is_dotenv_path(path)) {
                    self.open.insert(editor.id, path.clone());
                    // Deeper than the scan goes: Git is asked about it now.
                    if self.in_project(&path) && self.files.insert(path.clone()) {
                        self.ask_git();
                    }
                    self.check(&path);
                }
            }
            Event::EditorChanged(id) => {
                if let Some(path) = self.open.get(&id).cloned() {
                    self.check_with_neighbours(&path);
                }
            }
            Event::EditorSaved(editor) => {
                let Some(path) = editor.path.filter(|path| is_dotenv_path(path)) else {
                    return;
                };
                // A document saved under a new name is another file now.
                self.open.insert(editor.id, path.clone());
                if self.in_project(&path) && self.files.insert(path.clone()) {
                    self.ask_git();
                }
                self.check_with_neighbours(&path);
            }
            Event::EditorClosed(id) => {
                // The file on disk, without the closed document's unsaved changes.
                if let Some(path) = self.open.remove(&id) {
                    self.check_with_neighbours(&path);
                }
            }
            Event::GitChanged => {
                if let Some(timer) = self.git_timer.take() {
                    timers::cancel(timer);
                }
                self.git_timer = Some(timers::after(GIT_DELAY));
            }
            // A new `.env` file shows in Git's status too: the scan finds it.
            Event::Timer(id) if Some(id) == self.git_timer => {
                self.git_timer = None;
                self.scan();
                self.ask_git();
                self.check_all();
            }
            Event::SettingsChanged => self.check_all(),
            _ => {}
        }
    }
}

impl Dotenv {
    fn in_project(&self, path: &str) -> bool {
        self.root
            .as_ref()
            .is_some_and(|root| Path::new(path).starts_with(root))
    }

    /// The project's `.env` files on disk, and the open ones the scan doesn't reach.
    fn scan(&mut self) {
        self.files.clear();
        let Some(root) = self.root.clone() else {
            return;
        };
        let open: Vec<String> = self
            .open
            .values()
            .filter(|path| self.in_project(path) && Path::new(path).is_file())
            .cloned()
            .collect();
        self.files.extend(open);
        let mut entries = 0;
        let mut folders = vec![(root, 0)];
        while let Some((folder, depth)) = folders.pop() {
            let Ok(read) = std::fs::read_dir(&folder) else {
                continue;
            };
            for item in read.flatten() {
                entries += 1;
                if entries > MAX_ENTRIES {
                    log::warn("The project is too large to look for all of its .env files");
                    return;
                }
                let name = item.file_name().to_string_lossy().into_owned();
                let Ok(kind) = item.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if depth < DEPTH && !name.starts_with('.') && !SKIPPED.contains(&name.as_str())
                    {
                        folders.push((item.path(), depth + 1));
                    }
                } else if kind.is_file() && env::is_dotenv(&name) {
                    self.files
                        .insert(item.path().to_string_lossy().into_owned());
                }
            }
        }
    }

    /// Which of the files Git ignores and which it has: two calls for all of them.
    fn ask_git(&mut self) {
        self.git.clear();
        let Some(root) = self.root.clone() else {
            return;
        };
        let relative: Vec<String> = self
            .files
            .iter()
            .filter_map(|path| Path::new(path).strip_prefix(&root).ok())
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        if relative.is_empty() {
            return;
        }
        // `check-ignore -z` takes the paths on its input only.
        let ignored = git_paths(
            process::command("git")
                .args(["check-ignore", "--stdin", "-z"])
                .stdin(relative.join("\0")),
        );
        let tracked = git_paths(
            process::command("git")
                .args(["ls-files", "-z", "--"])
                .args(&relative),
        );
        for name in &relative {
            let path = root.join(name).to_string_lossy().into_owned();
            self.git
                .insert(path, git_state(name, ignored.as_ref(), tracked.as_ref()));
        }
    }

    /// Every file with something to say: the project's and the open ones.
    fn check_all(&mut self) {
        let paths: BTreeSet<String> = self
            .files
            .iter()
            .chain(self.open.values())
            .cloned()
            .collect();
        for path in &paths {
            self.check(path);
        }
        let stale: Vec<String> = self
            .published
            .iter()
            .filter(|path| !paths.contains(*path))
            .cloned()
            .collect();
        for path in stale {
            diagnostics::publish(&path, &[]);
            self.published.remove(&path);
        }
    }

    /// `path`, and when it is a template, the files of its folder compared with it.
    fn check_with_neighbours(&mut self, path: &str) {
        self.check(path);
        if !env::is_template(&file_name(path)) {
            return;
        }
        let folder = Path::new(path).parent().map(Path::to_path_buf);
        let neighbours: BTreeSet<String> = self
            .files
            .iter()
            .chain(self.open.values())
            .filter(|other| {
                *other != path && Path::new(other).parent().map(Path::to_path_buf) == folder
            })
            .cloned()
            .collect();
        for other in &neighbours {
            self.check(other);
        }
    }

    /// The open document's text, otherwise the file's.
    fn text_of(&self, path: &str) -> Option<String> {
        let open = self
            .open
            .iter()
            .find(|(_, open)| open.as_str() == path)
            .map(|(id, _)| *id);
        match open {
            Some(id) => editors::text(id).or_else(|| std::fs::read_to_string(path).ok()),
            None => std::fs::read_to_string(path).ok(),
        }
    }

    /// The template next to `path`, its name and its text.
    fn example_for(&self, path: &str) -> Option<(String, String)> {
        let folder = Path::new(path).parent()?;
        env::EXAMPLES.iter().find_map(|name| {
            let candidate = folder.join(name).to_string_lossy().into_owned();
            self.text_of(&candidate)
                .map(|text| (name.to_string(), text))
        })
    }

    /// Publishes the problems of one file.
    fn check(&mut self, path: &str) {
        let Some(text) = self.text_of(path) else {
            diagnostics::publish(path, &[]);
            self.published.remove(path);
            return;
        };
        let name = file_name(path);
        let template = env::is_template(&name);
        let entries = env::parse(&text);
        let whole_file = Range::on_line(0, 0, env::first_line_length(&text));
        let mut problems = Vec::new();

        for duplicate in env::duplicates(&entries) {
            let entry = duplicate.entry;
            problems.push(
                Diagnostic::warning(
                    Range::on_line(entry.line, entry.start, entry.end),
                    &trf(
                        "{0} is already set on line {1}",
                        &[&entry.key, &(duplicate.first_line + 1)],
                    ),
                )
                .code("duplicate-key"),
            );
        }

        if !template && setting::<bool>("git-check").unwrap_or(true) && env::has_values(&entries) {
            let message = match self.git.get(path) {
                Some(GitState::NotIgnored) => Some(tr(
                    "Git doesn't ignore this file: its values can get into a commit. Add it to .gitignore.",
                )),
                Some(GitState::Tracked) => Some(tr(
                    "This file is committed to Git: its values are in the repository.",
                )),
                _ => None,
            };
            if let Some(message) = message {
                problems.push(Diagnostic::warning(whole_file, &message).code("not-ignored"));
            }
        }

        if !template
            && setting::<bool>("compare-example").unwrap_or(true)
            && let Some((example_name, example_text)) = self.example_for(path)
        {
            let example = env::parse(&example_text);
            let (missing, extra) = env::compare(&entries, &example);
            if !missing.is_empty() {
                problems.push(
                    Diagnostic::hint(
                        whole_file,
                        &trf(
                            "Not set here, but listed in {0}: {1}",
                            &[&example_name, &missing.join(", ")],
                        ),
                    )
                    .code("missing-key"),
                );
            }
            for entry in extra {
                problems.push(
                    Diagnostic::hint(
                        Range::on_line(entry.line, entry.start, entry.end),
                        &trf("{0} isn't listed in {1}", &[&entry.key, &example_name]),
                    )
                    .code("unlisted-key"),
                );
            }
        }

        if problems.is_empty() && !self.published.contains(path) {
            return;
        }
        diagnostics::publish(path, &problems);
        if problems.is_empty() {
            self.published.remove(path);
        } else {
            self.published.insert(path.to_string());
        }
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_dotenv_path(path: &str) -> bool {
    env::is_dotenv(&file_name(path))
}

/// How Git sees `name` (relative to the project root), from what `git check-ignore` and `git
/// ls-files` printed; `None` — the command failed. Git doesn't ignore a file it has, whatever
/// `.gitignore` says.
fn git_state(
    name: &str,
    ignored: Option<&HashSet<String>>,
    tracked: Option<&HashSet<String>>,
) -> GitState {
    match (ignored, tracked) {
        (_, Some(tracked)) if tracked.contains(name) => GitState::Tracked,
        (Some(ignored), Some(_)) if ignored.contains(name) => GitState::Ignored,
        (Some(_), Some(_)) => GitState::NotIgnored,
        _ => GitState::Unknown,
    }
}

/// The paths a Git command prints (`-z`: separated by NUL, as given); `None` — not a repository,
/// no Git, or another failure. `check-ignore` exits with 1 when it prints none.
fn git_paths(command: process::Command) -> Option<HashSet<String>> {
    let what = command.as_raw().args.join(" ");
    let output = command
        .timeout(Duration::from_secs(10))
        .run()
        .map_err(|err| log::info(&format!("git {what}: {err}")))
        .ok()?;
    if !matches!(output.exit_code, Some(0 | 1)) {
        log::info(&format!("git {what}: {}", output.stderr_text().trim()));
        return None;
    }
    Some(
        output
            .stdout_text()
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

register_plugin!(Dotenv);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_git_sees_a_file() {
        let set = |names: &[&str]| {
            names
                .iter()
                .map(|name| name.to_string())
                .collect::<HashSet<_>>()
        };
        let ignored = set(&[".env", "apps/web/.env.local"]);
        let tracked = set(&[".env", ".env.production"]);
        let state = |name| git_state(name, Some(&ignored), Some(&tracked));
        assert_eq!(
            state(".env"),
            GitState::Tracked,
            "committed, then ignored: still in the repository"
        );
        assert_eq!(state("apps/web/.env.local"), GitState::Ignored);
        assert_eq!(state(".env.production"), GitState::Tracked);
        assert_eq!(state(".env.test"), GitState::NotIgnored);
        // Outside a repository, or without Git: nothing is said.
        assert_eq!(git_state(".env.test", None, None), GitState::Unknown);
        assert_eq!(
            git_state(".env.test", Some(&ignored), None),
            GitState::Unknown
        );
        assert_eq!(git_state(".env", None, Some(&tracked)), GitState::Tracked);
    }

    #[test]
    fn dotenv_paths() {
        assert!(is_dotenv_path("/p/apps/web/.env.local"));
        assert!(!is_dotenv_path("/p/.envrc"));
        assert_eq!(file_name("/p/a/.env"), ".env");
    }
}
