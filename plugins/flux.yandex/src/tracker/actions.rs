//! The Claude actions of the settings (`actions`, records): a name, a prompt with the task's data
//! in `{…}`, Claude's mode, whether the task is taken to work first, whether the board shows it.
//! The board's button is the favorite one — the first marked, else the first.

use serde::Deserialize;

use super::model::{Comment, Issue, IssueLink, value_text};
use crate::client;
use crate::settings;
use crate::text;

/// Comments a prompt gets: the last ones.
const PROMPT_COMMENTS: usize = 20;

/// Claude's mode for an action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// As Claude's settings say.
    #[default]
    Default,
    /// Plan mode: Claude studies and proposes before changing anything.
    Plan,
}

/// An action of the settings.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Action {
    pub name: String,
    pub prompt: String,
    pub mode: Mode,
    /// Take the task to work first: the status, the branch, the tabs, the timer.
    pub start: bool,
    /// The board's button.
    pub favorite: bool,
}

impl Action {
    /// The name as the user reads it: the manifest's defaults (Take to Work, Plan) in the user's
    /// language, one's own as written.
    pub fn title(&self) -> String {
        flux_plugin_api::tr(&self.name)
    }
}

/// The actions of the settings, in order; nameless ones left out.
pub fn actions() -> Vec<Action> {
    let actions: Vec<Action> = flux_plugin_api::setting(settings::ACTIONS).unwrap_or_default();
    actions
        .into_iter()
        .filter(|action| !action.name.trim().is_empty())
        .collect()
}

/// The board's action: the first marked, else the first.
pub fn favorite(actions: &[Action]) -> Option<&Action> {
    actions
        .iter()
        .find(|action| action.favorite)
        .or_else(|| actions.first())
}

/// What a prompt is filled from: the card's task, its links and comments, its branch.
pub struct PromptData<'a> {
    pub issue: &'a Issue,
    pub links: &'a [IssueLink],
    pub comments: &'a [Comment],
    /// The task's branch: the one it's worked on, else the name it would get.
    pub branch: &'a str,
}

/// The prompt with the task's data in its `{…}`; an unknown name stays as it is.
pub fn fill_prompt(template: &str, data: &PromptData) -> String {
    let issue = data.issue;
    let named = |value: &Option<super::model::Named>| {
        value
            .as_ref()
            .map(|named| named.display.clone())
            .unwrap_or_default()
    };
    let parent = issue
        .parent
        .as_ref()
        .map(|parent| match &parent.key {
            Some(key) if !parent.display.is_empty() => format!("{key} \"{}\"", parent.display),
            Some(key) => key.clone(),
            None => parent.display.clone(),
        })
        .unwrap_or_default();
    let links = data
        .links
        .iter()
        .map(|link| {
            let status = link
                .status
                .as_ref()
                .map(|status| format!(" ({})", status.display))
                .unwrap_or_default();
            format!(
                "- {}: {} \"{}\"{status}",
                link.relation(),
                link.object.key.as_deref().unwrap_or_default(),
                link.object.display
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let skip = data.comments.len().saturating_sub(PROMPT_COMMENTS);
    let comments = data.comments[skip..]
        .iter()
        .map(|comment| {
            let author = named(&comment.created_by);
            let date = comment
                .created_at
                .as_deref()
                .map(|at| at.get(..10).unwrap_or(at))
                .unwrap_or_default();
            format!("{author}, {date}:\n{}", comment.text.trim())
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let url = client::web_url(&issue.key);
    let sprint = issue.sprint_name();
    let values = [
        ("key", issue.key.as_str()),
        ("summary", issue.summary.as_str()),
        ("description", issue.description.as_deref().unwrap_or_default()),
        ("url", url.as_str()),
        ("status", issue.status_name()),
        ("type", &named(&issue.kind)),
        ("priority", &named(&issue.priority)),
        ("assignee", &named(&issue.assignee)),
        ("author", &named(&issue.created_by)),
        ("queue", &named(&issue.queue)),
        ("deadline", issue.deadline.as_deref().unwrap_or_default()),
        ("sprint", sprint.as_str()),
        ("parent", parent.as_str()),
        ("branch", data.branch),
        ("links", links.as_str()),
        ("comments", comments.as_str()),
    ];
    let filled = text::fill(template, &values);
    // Blank lines left by empty values don't pile up.
    let mut out = String::new();
    let mut blank = 0;
    for line in filled.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// The task as Markdown, for Claude's attachment (Discuss with Claude): the title, the fields, the
/// link, the description, the links and the comments.
pub fn task_markdown(data: &PromptData) -> String {
    let issue = data.issue;
    let field = |name: &str, value: &str| {
        if value.is_empty() {
            String::new()
        } else {
            format!("- {name}: {value}\n")
        }
    };
    let named = |value: &Option<super::model::Named>| {
        value
            .as_ref()
            .map(|named| named.display.clone())
            .unwrap_or_default()
    };
    let mut out = format!("# {} {}\n\n", issue.key, issue.summary);
    out.push_str(&field("Status", issue.status_name()));
    out.push_str(&field("Type", &named(&issue.kind)));
    out.push_str(&field("Priority", &named(&issue.priority)));
    out.push_str(&field("Assignee", &named(&issue.assignee)));
    out.push_str(&field("Deadline", issue.deadline.as_deref().unwrap_or_default()));
    out.push_str(&field("Link", &client::web_url(&issue.key)));
    if let Some(description) = issue.description.as_deref().filter(|text| !text.is_empty()) {
        out.push_str(&format!("\n## Description\n\n{description}\n"));
    }
    if !data.links.is_empty() {
        out.push_str("\n## Links\n\n");
        for link in data.links {
            out.push_str(&format!(
                "- {}: {} {}\n",
                link.relation(),
                link.object.key.as_deref().unwrap_or_default(),
                link.object.display
            ));
        }
    }
    let skip = data.comments.len().saturating_sub(PROMPT_COMMENTS);
    if skip < data.comments.len() {
        out.push_str("\n## Comments\n");
        for comment in &data.comments[skip..] {
            out.push_str(&format!(
                "\n**{}**, {}:\n\n{}\n",
                named(&comment.created_by),
                comment
                    .created_at
                    .as_deref()
                    .map(|at| at.get(..10).unwrap_or(at))
                    .unwrap_or_default(),
                comment.text.trim()
            ));
        }
    }
    text::ellipsis(&out, 20_000)
}

/// A changed field's line for the history: "Status: Open → In Progress".
pub fn change_line(field: &str, from: &serde_json::Value, to: &serde_json::Value) -> String {
    let from = value_text(from);
    let to = value_text(to);
    match (from.is_empty(), to.is_empty()) {
        (true, true) => field.to_string(),
        (true, false) => format!("{field}: {to}"),
        (false, true) => format!("{field}: {from} → —"),
        (false, false) => format!("{field}: {from} → {to}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::model::Named;

    fn issue() -> Issue {
        serde_json::from_str(
            r#"{"key": "TRACK-142", "summary": "Ошибка входа через SSO",
                "description": "Падает на шаге 2.",
                "status": {"key": "open", "display": "Открыт"},
                "type": {"key": "bug", "display": "Ошибка"},
                "assignee": {"display": "Егор"},
                "parent": {"key": "TRACK-100", "display": "Вход"}}"#,
        )
        .unwrap()
    }

    #[test]
    fn fills_the_prompt_from_the_card() {
        let issue = issue();
        let links: Vec<IssueLink> = serde_json::from_str(
            r#"[{"type": {"inward": "Блокирует", "outward": "Зависит от"}, "direction": "outward",
                 "object": {"key": "WEB-7", "display": "Форма"}, "status": {"display": "Открыт"}}]"#,
        )
        .unwrap();
        let comments = vec![Comment {
            text: "Воспроизводится.".into(),
            created_by: Some(Named {
                display: "Анна".into(),
                ..Named::default()
            }),
            created_at: Some("2026-10-13T09:00:00.000+0000".into()),
            ..Comment::default()
        }];
        let data = PromptData {
            issue: &issue,
            links: &links,
            comments: &comments,
            branch: "TRACK-142-oshibka",
        };
        let prompt = fill_prompt(
            "Задача {key} «{summary}» ({status}, {assignee}): {url}\n\n{description}\n\nСвязи:\n{links}\n\nКомментарии:\n{comments}\n\nВетка {branch}, родитель {parent}. {unknown}",
            &data,
        );
        assert!(prompt.starts_with(
            "Задача TRACK-142 «Ошибка входа через SSO» (Открыт, Егор): https://tracker.yandex.ru/TRACK-142"
        ));
        assert!(prompt.contains("- Зависит от: WEB-7 \"Форма\" (Открыт)"));
        assert!(prompt.contains("Анна, 2026-10-13:\nВоспроизводится."));
        assert!(prompt.contains("Ветка TRACK-142-oshibka, родитель TRACK-100 \"Вход\". {unknown}"));
    }

    #[test]
    fn empty_values_leave_no_pile_of_blank_lines() {
        let mut issue = issue();
        issue.description = None;
        let data = PromptData {
            issue: &issue,
            links: &[],
            comments: &[],
            branch: "",
        };
        assert_eq!(fill_prompt("{key}\n\n{description}\n\n{links}\n\nEnd", &data), "TRACK-142\n\nEnd");
    }

    #[test]
    fn the_favorite_is_the_first_marked_or_the_first() {
        let action = |name: &str, favorite: bool| Action {
            name: name.into(),
            favorite,
            ..Action::default()
        };
        let actions = [action("Plan", false), action("Take", true), action("Other", true)];
        assert_eq!(favorite(&actions).unwrap().name, "Take");
        assert_eq!(favorite(&actions[..1]).unwrap().name, "Plan");
        assert!(favorite(&[]).is_none());
    }

    #[test]
    fn reads_actions_as_the_settings_store_them() {
        let actions: Vec<Action> = serde_json::from_str(
            r#"[{"name": "Take to Work", "prompt": "Do {key}", "mode": "default", "start": true, "favorite": true},
                {"name": "Plan", "prompt": "Plan {key}", "mode": "plan"}]"#,
        )
        .unwrap();
        assert_eq!(actions[0].mode, Mode::Default);
        assert!(actions[0].start);
        assert_eq!(actions[1].mode, Mode::Plan);
        assert!(!actions[1].favorite);
    }

    #[test]
    fn history_lines() {
        use serde_json::json;
        assert_eq!(
            change_line("Статус", &json!({"display": "Открыт"}), &json!({"display": "В работе"})),
            "Статус: Открыт → В работе"
        );
        assert_eq!(change_line("Теги", &json!(null), &json!(["a"])), "Теги: a");
    }
}
