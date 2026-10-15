//! Links on tasks' keys: `TRACK-142` of the user's queues in code, commit messages and Claude's
//! chat — a tooltip (the title, the status, the assignee), a click opens the card.

use flux_plugin_api::links;

use super::Tracker;
use super::api;
use super::model::Queue;
use crate::auth::Auth;

/// The pattern of the queues' keys; none without queues.
pub(super) fn set(queues: &[Queue]) {
    let prefixes: Vec<&str> = queues
        .iter()
        .map(|queue| queue.key.as_str())
        .filter(|key| !key.is_empty())
        .collect();
    if prefixes.is_empty() {
        links::clear();
        return;
    }
    let pattern = links::pattern("task", &links::key_regex(&prefixes)).tooltip();
    if let Err(err) = links::set([pattern]) {
        flux_plugin_api::log::warn(&format!("Links: {err}"));
    }
}

pub(super) fn clear() {
    links::clear();
}

impl Tracker {
    /// A link's tooltip: from the cache, else the task's.
    pub(super) fn describe_link(&mut self, request: u64, text: &str, auth: &Auth) {
        let key = text.to_ascii_uppercase();
        if let Some(markdown) = self.tooltips.get(&key) {
            links::tooltip(request, markdown);
            return;
        }
        match api::issue(auth, &key) {
            Ok(issue) => {
                let mut details = vec![issue.status_name().to_string()];
                if let Some(assignee) = &issue.assignee {
                    details.push(assignee.display.clone());
                }
                details.retain(|detail| !detail.is_empty());
                let markdown = format!(
                    "**{}** {}\n\n{}",
                    issue.key,
                    issue.summary,
                    details.join(" · ")
                );
                links::tooltip(request, &markdown);
                self.tooltips.insert(key, markdown);
            }
            Err(_) => links::no_tooltip(request),
        }
    }
}
