//! Tracker's API v3 as the Tracker side uses it, over [`crate::client`]: tasks (search, one task,
//! its changes of fields, transitions, comments, links, history, work log), what the fields take
//! (priorities, a queue's types and components, users, a board's sprints), queues, who the token is.

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::model::{
    ChangelogEntry, Comment, Component, Issue, IssueLink, Myself, Priority, Queue, QueueTypes,
    Sprint, Transition, User,
};
use crate::auth::Auth;
use crate::client::{self, ClientError};
use crate::text;

/// Tasks of a search page.
pub const PAGE: u32 = 100;

fn parse<T: DeserializeOwned>(value: Value) -> Result<T, ClientError> {
    serde_json::from_value(value).map_err(|err| ClientError::Other(err.to_string()))
}

/// A task's key made safe for a path: a key as Tracker writes it, upper case.
fn key_path(key: &str) -> Result<String, ClientError> {
    let key = key.trim().to_ascii_uppercase();
    if text::is_key(&key) {
        Ok(key)
    } else {
        Err(ClientError::Other(format!("Not a task's key: {key}")))
    }
}

/// Who the token is.
pub fn myself(auth: &Auth) -> Result<Myself, ClientError> {
    parse(client::get(auth, "myself")?)
}

/// What to look for.
pub enum Search<'a> {
    /// A query in Tracker's query language.
    Query(&'a str),
    /// These keys.
    Keys(&'a [String]),
}

/// A page of a search, and how many tasks match in all (`X-Total-Count`).
pub fn search(auth: &Auth, what: Search, per_page: u32) -> Result<(Vec<Issue>, u32), ClientError> {
    let body = match what {
        Search::Query(query) => json!({ "query": query }),
        Search::Keys(keys) => json!({ "keys": keys }),
    };
    let path = format!("issues/_search?perPage={per_page}");
    let (value, headers) = client::send(auth, "POST", &path, Some(&body))?;
    let issues: Vec<Issue> = parse(value)?;
    let total = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-total-count"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(issues.len() as u32);
    Ok((issues, total))
}

/// One task.
pub fn issue(auth: &Auth, key: &str) -> Result<Issue, ClientError> {
    parse(client::get(auth, &format!("issues/{}", key_path(key)?))?)
}

/// The transitions the task's status allows.
pub fn transitions(auth: &Auth, key: &str) -> Result<Vec<Transition>, ClientError> {
    parse(client::get(
        auth,
        &format!("issues/{}/transitions", key_path(key)?),
    )?)
}

/// Moves the task by a transition.
pub fn execute(auth: &Auth, key: &str, transition: &str) -> Result<(), ClientError> {
    let id: String = transition
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    client::post(
        auth,
        &format!("issues/{}/transitions/{id}/_execute", key_path(key)?),
        &json!({}),
    )
    .map(|_| ())
}

/// The task's comments, oldest first.
pub fn comments(auth: &Auth, key: &str) -> Result<Vec<Comment>, ClientError> {
    parse(client::get(
        auth,
        &format!("issues/{}/comments?perPage=100", key_path(key)?),
    )?)
}

/// Adds a comment in Markdown.
pub fn add_comment(auth: &Auth, key: &str, text: &str) -> Result<Comment, ClientError> {
    parse(client::post(
        auth,
        &format!("issues/{}/comments", key_path(key)?),
        &json!({ "text": text, "markupType": "md" }),
    )?)
}

/// The task's links to other tasks.
pub fn links(auth: &Auth, key: &str) -> Result<Vec<IssueLink>, ClientError> {
    parse(client::get(auth, &format!("issues/{}/links", key_path(key)?))?)
}

/// A page of the task's changes, oldest first, after the change `after` (none — from the start).
pub fn changelog(
    auth: &Auth,
    key: &str,
    after: Option<&str>,
    per_page: u32,
) -> Result<Vec<ChangelogEntry>, ClientError> {
    let mut path = format!("issues/{}/changelog?perPage={per_page}", key_path(key)?);
    if let Some(after) = after {
        let after: String = after.chars().filter(char::is_ascii_alphanumeric).collect();
        path.push_str(&format!("&id={after}"));
    }
    parse(client::get(auth, &path)?)
}

/// Logs time spent on the task: `start` — Tracker's ISO time with the local offset, `minutes` —
/// how long.
pub fn add_worklog(
    auth: &Auth,
    key: &str,
    start: &str,
    minutes: u32,
    comment: &str,
) -> Result<(), ClientError> {
    let mut body = json!({ "start": start, "duration": text::iso_duration(minutes) });
    if !comment.trim().is_empty() {
        body["comment"] = Value::String(comment.trim().to_string());
    }
    client::post(auth, &format!("issues/{}/worklog", key_path(key)?), &body).map(|_| ())
}

/// The queues the user sees.
pub fn queues(auth: &Auth) -> Result<Vec<Queue>, ClientError> {
    parse(client::get(auth, "queues?perPage=100")?)
}

/// Changes the task's fields — `PATCH /v3/issues/{key}` with Tracker's formats (see
/// [`super::edit`]) — and returns the task as it is now.
pub fn update(auth: &Auth, key: &str, changes: &Value) -> Result<Issue, ClientError> {
    let (value, _) = client::send(
        auth,
        "PATCH",
        &format!("issues/{}", key_path(key)?),
        Some(changes),
    )?;
    parse(value)
}

/// The organization's priorities, named in the interface language.
pub fn priorities(auth: &Auth) -> Result<Vec<Priority>, ClientError> {
    parse(client::get(auth, "priorities")?)
}

/// The issue types the queue allows.
pub fn issue_types(auth: &Auth, queue: &str) -> Result<Vec<super::model::Named>, ClientError> {
    let types: QueueTypes = parse(client::get(
        auth,
        &format!("queues/{}?expand=issueTypesConfig", queue_path(queue)?),
    )?)?;
    Ok(types
        .issue_types_config
        .into_iter()
        .map(|config| config.issue_type)
        .collect())
}

/// The queue's components.
pub fn components(auth: &Auth, queue: &str) -> Result<Vec<Component>, ClientError> {
    parse(client::get(
        auth,
        &format!("queues/{}/components", queue_path(queue)?),
    )?)
}

/// The organization's users who still work there (the dismissed left out), at most a thousand.
pub fn users(auth: &Auth) -> Result<Vec<User>, ClientError> {
    let users: Vec<User> = parse(client::get(auth, "users?perPage=1000")?)?;
    Ok(users.into_iter().filter(|user| !user.dismissed).collect())
}

/// A board's sprints; none for a board that has no sprints (a kanban board answers 400).
pub fn sprints(auth: &Auth, board: &str) -> Result<Vec<Sprint>, ClientError> {
    let board: String = board.chars().filter(char::is_ascii_digit).collect();
    match client::get(auth, &format!("boards/{board}/sprints")) {
        Ok(value) => parse(value),
        Err(ClientError::NotFound) => Ok(Vec::new()),
        Err(ClientError::Other(_)) => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

/// A queue's key made safe for a path.
fn queue_path(queue: &str) -> Result<String, ClientError> {
    let queue = queue.trim().to_ascii_uppercase();
    if !queue.is_empty() && queue.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(queue)
    } else {
        Err(ClientError::Other(format!("Not a queue's key: {queue}")))
    }
}
