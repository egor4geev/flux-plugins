//! The task source (API 0.6, `task-source = true`): the user's tasks for another plugin — the bot
//! that brings Flux's Claude to the phone. The plugin has no service: a query comes to one of its
//! windows' instances, a run to its instance in the window of the run's project.
//!
//! - `mine` — My Tasks: the `my-query` setting's search, as the Tracker window lists them.
//! - `search` — as Open Task (⌥⇧N) finds: the user's tasks the text matches first, then Tracker's
//!   search — a key for that task, words for the tasks with them in the title, the latest changed
//!   first.
//! - `get` — the task with the Markdown Discuss with Claude attaches (its fields, description,
//!   links and comments) and the Claude actions of the settings, each named by its name: the user
//!   may reorder them.
//! - `run` — the action as the card's button does it ([`super::work`]): the status, the branch,
//!   the tabs, the timer, the commit template, Claude's prompt — but quietly, nobody is at the
//!   computer. Nothing is asked: Git's quiet calls, and a branch local changes keep from being
//!   checked out is left alone, Claude starts on the current one; what would be a notification is
//!   a note. The answer, `tasks::started`, is Claude's session with the notes, or why there is
//!   none.
//!
//! Signed out or without an organization, every query fails with where to sign in.

use flux_plugin_api::tasks::{self, Query, Task, TaskQuery, TaskRun};
#[cfg(not(test))]
use flux_plugin_api::{tr, trf};

use super::Tracker;
use super::actions::{self, PromptData};
use super::api::{self, Search};
#[cfg(test)]
use super::facts::{tr, trf};
use super::model::Issue;
use super::window::{find_typed, matching, my_query};
use super::work::Caller;
use crate::auth::Auth;
use crate::client::{self, ClientError};
use crate::text as words;

impl Tracker {
    /// Another plugin's query: answered at once (`tasks::respond`); a run — when Claude's session
    /// opens (`tasks::started`).
    pub(super) fn task_query(&mut self, query: TaskQuery, auth: &Auth) {
        let found = match query.query {
            Query::Run(run) => return self.run_for_plugin(query.id, run, auth),
            Query::Mine => mine(auth),
            Query::Search(text) => search(&text, auth),
            Query::Get(key) => self.task(&key, auth),
        };
        tasks::respond(query.id, found);
    }

    /// The task with its text for Claude and the actions on it; none for a key Tracker doesn't
    /// know.
    fn task(&mut self, key: &str, auth: &Auth) -> Result<Vec<Task>, String> {
        ready(auth)?;
        let key = key.trim().to_ascii_uppercase();
        if !words::is_key(&key) {
            return Ok(Vec::new());
        }
        let (issue, links, comments) = match self.task_data(&key, auth) {
            Ok(data) => data,
            Err(ClientError::NotFound) => return Ok(Vec::new()),
            Err(err) => return Err(err.message()),
        };
        let branch = self.branch_of(&issue);
        let markdown = actions::task_markdown(&PromptData {
            issue: &issue,
            links: &links,
            comments: &comments,
            branch: &branch,
        });
        let task = actions::actions()
            .iter()
            .fold(listed(&issue).markdown(&markdown), |task, action| {
                task.action(action.task_action())
            });
        Ok(vec![task])
    }

    /// An action of the settings on the task, as its button does it but quietly; the answer when
    /// Claude's session opens.
    fn run_for_plugin(&mut self, query: u64, run: TaskRun, auth: &Auth) {
        if let Err(why) = ready(auth) {
            return tasks::started(query, Err(why));
        }
        let all = actions::actions();
        let Some(action) = actions::by_id(&all, &run.action).cloned() else {
            let why = trf(
                "No Claude action \"{0}\" in the Yandex Tracker plugin's settings.",
                &[&run.action],
            );
            return tasks::started(query, Err(why));
        };
        let key = run.key.trim().to_ascii_uppercase();
        let caller = Caller::Run {
            query,
            notes: Vec::new(),
        };
        self.run_action(&key, action, caller, auth);
    }
}

/// Signed in, with an organization; else where to do it.
fn ready(auth: &Auth) -> Result<(), String> {
    if auth.headers().is_none() {
        Err(tr(
            "Sign in to Yandex in the Yandex Tracker plugin's settings.",
        ))
    } else if auth.organization().is_none() {
        Err(tr(
            "Choose the organization in the Yandex Tracker plugin's settings.",
        ))
    } else {
        Ok(())
    }
}

/// My Tasks.
fn mine(auth: &Auth) -> Result<Vec<Task>, String> {
    ready(auth)?;
    Ok(my_tasks(auth)?.iter().map(listed).collect())
}

/// The `my-query` setting's tasks, as the Tracker window's first page has them.
fn my_tasks(auth: &Auth) -> Result<Vec<Issue>, String> {
    api::search(
        auth,
        Search::Query(&my_query()),
        api::PAGE,
        api::PICKER_FIELDS,
    )
    .map(|(issues, _)| issues)
    .map_err(|err| err.message())
}

/// The tasks as Open Task finds them for the text.
fn search(text: &str, auth: &Auth) -> Result<Vec<Task>, String> {
    ready(auth)?;
    let mine = my_tasks(auth)?;
    let local = matching(&mine, text);
    let found = if text.trim().is_empty() {
        Vec::new()
    } else {
        find_typed(auth, text).map_err(|err| err.message())?
    };
    Ok(in_open_task_order(&local, &found)
        .into_iter()
        .map(listed)
        .collect())
}

/// Open Task's order: the user's tasks the text matches, then the others Tracker found.
fn in_open_task_order<'a>(mine: &[&'a Issue], found: &'a [Issue]) -> Vec<&'a Issue> {
    let others = found
        .iter()
        .filter(|issue| !mine.iter().any(|known| known.key == issue.key));
    mine.iter().copied().chain(others).collect()
}

/// A task of a list: its key, title, status and page; its text and actions are `get`'s.
fn listed(issue: &Issue) -> Task {
    tasks::task(&issue.key, &issue.summary)
        .status(issue.status_name())
        .url(&client::web_url(&issue.key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::window::{Lookup, lookup};
    use crate::tracker::work::branch_note;

    fn issue(key: &str, summary: &str) -> Issue {
        serde_json::from_value(serde_json::json!({
            "key": key, "summary": summary, "status": {"key": "open", "display": "Открыт"}
        }))
        .unwrap()
    }

    #[test]
    fn a_key_finds_that_task_words_the_titles() {
        assert_eq!(lookup(" track-142 "), Lookup::Key("TRACK-142".into()));
        assert_eq!(
            lookup("вход \"SSO\""),
            Lookup::Query(r#"Summary: "вход \"SSO\"" "Sort by": Updated DESC"#.into())
        );
        assert_eq!(
            lookup("142"),
            Lookup::Query(r#"Summary: "142" "Sort by": Updated DESC"#.into())
        );
    }

    #[test]
    fn the_users_tasks_first_then_the_found_ones() {
        let mine = [
            issue("TRACK-142", "Ошибка входа через SSO"),
            issue("WEB-7", "Форма входа"),
            issue("TRACK-150", "Отчёт"),
        ];
        let local = matching(&mine, "ВХОД");
        let found = [
            issue("WEB-9", "Вход по ссылке"),
            issue("WEB-7", "Форма входа"),
        ];
        let keys: Vec<&str> = in_open_task_order(&local, &found)
            .iter()
            .map(|issue| issue.key.as_str())
            .collect();
        assert_eq!(keys, ["TRACK-142", "WEB-7", "WEB-9"]);
        // A part of a key matches the user's tasks too; no text — all of them.
        let keys = |typed: &str| -> Vec<String> {
            matching(&mine, typed)
                .iter()
                .map(|issue| issue.key.clone())
                .collect()
        };
        assert_eq!(keys("track-1"), ["TRACK-142", "TRACK-150"]);
        assert_eq!(keys(" ").len(), 3);
    }

    #[test]
    fn a_listed_task_has_its_status_and_page() {
        let task = listed(&issue("TRACK-142", "Ошибка входа"));
        assert_eq!(
            (task.key.as_str(), task.title.as_str(), task.status.as_str()),
            ("TRACK-142", "Ошибка входа", "Открыт")
        );
        assert_eq!(
            task.url.as_deref(),
            Some("https://tracker.yandex.ru/TRACK-142")
        );
        assert!(task.markdown.is_empty() && task.actions.is_empty());
    }

    #[test]
    fn the_note_when_the_branch_stays() {
        // Flux's sentence names the branch: it says it all.
        assert_eq!(
            branch_note(
                "TRACK-142-fix",
                Some("main"),
                "Local changes are in the way of checking out 'TRACK-142-fix'"
            ),
            "Local changes are in the way of checking out 'TRACK-142-fix'. Claude works on the \
             current branch, main."
        );
        // Git's own words: the branch first.
        assert_eq!(
            branch_note("TRACK-142-fix", None, "fatal: cannot lock ref."),
            "Couldn't check out the branch TRACK-142-fix: fatal: cannot lock ref. Claude works on \
             the current branch."
        );
        assert_eq!(
            branch_note("TRACK-142-fix", Some("main"), ""),
            "Couldn't check out the branch TRACK-142-fix. Claude works on the current branch, main."
        );
    }
}
