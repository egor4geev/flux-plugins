//! The board's filters (part 10.5; the author: «фильтры на доску затянуть»), as Tracker's board
//! filters: by the assignee, the type, the priority, the tags, the components, and words of the key
//! or the title — over the tasks the board has read (its own filter or query still decides which
//! tasks those are). Several values of one filter — any of them; the filters together — all of
//! them. Each board keeps its filter, and the user's own filters of a board, saved under a name,
//! are a click away. Pure, so tests read it.

use std::cmp::Ordering;

#[cfg(not(test))]
use flux_plugin_api::{tr, trf};
use serde::{Deserialize, Serialize};

use crate::tracker::model::{Issue, Named};

use super::model::{Me, id_text};

/// The translations are Flux's, asked of the host — in tests on this Mac there is no host: the
/// English text itself.
#[cfg(test)]
fn tr(text: &str) -> String {
    text.to_string()
}

#[cfg(test)]
fn trf(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    args.iter()
        .enumerate()
        .fold(template.to_string(), |text, (index, arg)| {
            text.replace(&format!("{{{index}}}"), &arg.to_string())
        })
}

/// The value of tasks without an assignee.
pub const NOBODY: &str = "none";

/// What a filter looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facet {
    Assignee,
    Type,
    Priority,
    Tags,
    Components,
}

impl Facet {
    pub const ALL: [Facet; 5] = [
        Facet::Assignee,
        Facet::Type,
        Facet::Priority,
        Facet::Tags,
        Facet::Components,
    ];

    /// The id of its menu badge on the board (and the anchor of its picker).
    pub fn id(self) -> &'static str {
        match self {
            Facet::Assignee => "filter-assignee",
            Facet::Type => "filter-type",
            Facet::Priority => "filter-priority",
            Facet::Tags => "filter-tags",
            Facet::Components => "filter-components",
        }
    }

    pub fn of(id: &str) -> Option<Facet> {
        Facet::ALL.into_iter().find(|facet| facet.id() == id)
    }

    /// Its name on the board: "Assignee".
    pub fn title(self) -> String {
        match self {
            Facet::Assignee => tr("Assignee"),
            Facet::Type => tr("Type"),
            Facet::Priority => tr("Priority"),
            Facet::Tags => tr("Tags"),
            Facet::Components => tr("Components"),
        }
    }

    /// The values a task has for it: (key, name). A task without an assignee has [`NOBODY`].
    pub fn values(self, task: &Issue) -> Vec<(String, String)> {
        let named = |named: &Named| (key_of(named), named.display.clone());
        match self {
            Facet::Assignee => match &task.assignee {
                Some(assignee) => vec![named(assignee)],
                None => vec![(NOBODY.to_string(), tr("No assignee"))],
            },
            Facet::Type => task.kind.iter().map(named).collect(),
            Facet::Priority => task.priority.iter().map(named).collect(),
            Facet::Tags => task
                .tags
                .iter()
                .filter(|tag| !tag.trim().is_empty())
                .map(|tag| (tag.clone(), tag.clone()))
                .collect(),
            Facet::Components => task.components.iter().map(named).collect(),
        }
    }
}

/// A reference's key for a filter: a person and a component by their id, a type and a priority
/// by their key.
fn key_of(named: &Named) -> String {
    match (&named.key, id_text(&named.id)) {
        (Some(key), _) if !key.is_empty() => key.clone(),
        (_, id) if !id.is_empty() => id,
        _ => named.display.clone(),
    }
}

/// A chosen value: its key and its name (shown even when no task on the board has it any more).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub key: String,
    pub name: String,
}

/// A board's filter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoardFilter {
    pub assignees: Vec<Choice>,
    pub types: Vec<Choice>,
    pub priorities: Vec<Choice>,
    pub tags: Vec<Choice>,
    pub components: Vec<Choice>,
    /// Words of the key or the title.
    pub text: String,
}

impl BoardFilter {
    /// Nothing is filtered.
    pub fn is_empty(&self) -> bool {
        Facet::ALL.iter().all(|facet| self.chosen(*facet).is_empty()) && self.text.trim().is_empty()
    }

    pub fn chosen(&self, facet: Facet) -> &[Choice] {
        match facet {
            Facet::Assignee => &self.assignees,
            Facet::Type => &self.types,
            Facet::Priority => &self.priorities,
            Facet::Tags => &self.tags,
            Facet::Components => &self.components,
        }
    }

    fn chosen_mut(&mut self, facet: Facet) -> &mut Vec<Choice> {
        match facet {
            Facet::Assignee => &mut self.assignees,
            Facet::Type => &mut self.types,
            Facet::Priority => &mut self.priorities,
            Facet::Tags => &mut self.tags,
            Facet::Components => &mut self.components,
        }
    }

    /// Adds the value to the facet's, or takes it away when it's there.
    pub fn toggle(&mut self, facet: Facet, key: &str, name: &str) {
        let chosen = self.chosen_mut(facet);
        match chosen.iter().position(|choice| choice.key == key) {
            Some(index) => {
                chosen.remove(index);
            }
            None => chosen.push(Choice {
                key: key.to_string(),
                name: name.to_string(),
            }),
        }
    }

    /// The facet filters nothing any more.
    pub fn clear(&mut self, facet: Facet) {
        self.chosen_mut(facet).clear();
    }

    /// The task passes: one of the chosen values of every facet that has some, and every word of
    /// the text in its key or its title.
    pub fn matches(&self, task: &Issue) -> bool {
        let facets = Facet::ALL.iter().all(|facet| {
            let chosen = self.chosen(*facet);
            chosen.is_empty()
                || facet
                    .values(task)
                    .iter()
                    .any(|(key, _)| chosen.iter().any(|choice| &choice.key == key))
        });
        if !facets {
            return false;
        }
        let words = self.text.to_lowercase();
        let haystack = format!("{} {}", task.key, task.summary).to_lowercase();
        words.split_whitespace().all(|word| haystack.contains(word))
    }

    /// The same filter, whatever order its values were chosen in (a saved filter is found by it).
    pub fn same_as(&self, other: &BoardFilter) -> bool {
        fn keys(choices: &[Choice]) -> Vec<&str> {
            let mut keys: Vec<&str> = choices.iter().map(|choice| choice.key.as_str()).collect();
            keys.sort_unstable();
            keys
        }
        Facet::ALL
            .iter()
            .all(|facet| keys(self.chosen(*facet)) == keys(other.chosen(*facet)))
            && self.text.trim().to_lowercase() == other.text.trim().to_lowercase()
    }

    /// In words, for a name it could be saved under and the saved filters' menu: "Anna Petrova ·
    /// Bug · sso, auth · «login»".
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Facet::ALL
            .iter()
            .map(|facet| self.chosen(*facet))
            .filter(|chosen| !chosen.is_empty())
            .map(|chosen| {
                chosen
                    .iter()
                    .map(|choice| choice.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect();
        if !self.text.trim().is_empty() {
            parts.push(format!("«{}»", self.text.trim()));
        }
        parts.join(" · ")
    }

    /// The text of the facet's menu badge: its name, with what's chosen — "Assignee: Anna Petrova",
    /// "Tags: sso +2".
    pub fn chip(&self, facet: Facet) -> String {
        match self.chosen(facet) {
            [] => facet.title(),
            [one] => format!("{}: {}", facet.title(), one.name),
            [first, rest @ ..] => format!("{}: {} +{}", facet.title(), first.name, rest.len()),
        }
    }
}

/// A filter of the user's own, saved for a board under a name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFilter {
    pub name: String,
    pub filter: BoardFilter,
}

/// A value a facet offers on the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetValue {
    pub key: String,
    pub name: String,
    /// How many of the board's tasks have it.
    pub count: usize,
    /// It's the user (the assignee facet).
    pub mine: bool,
}

/// The values of a facet on the board's tasks, with how many tasks have each — and the chosen
/// values no task has now (to take them away). In an order that reads: for people, the user first
/// and No assignee last; priorities from the loudest; the rest by name.
pub fn options(tasks: &[Issue], facet: Facet, chosen: &[Choice], me: Option<&Me>) -> Vec<FacetValue> {
    let mut found: Vec<FacetValue> = Vec::new();
    for task in tasks {
        for (key, name) in facet.values(task) {
            match found.iter_mut().find(|option| option.key == key) {
                Some(option) => option.count += 1,
                None => {
                    let mine = facet == Facet::Assignee
                        && me.is_some_and(|me| task.assignee.as_ref().is_some_and(|a| me.is(a)));
                    found.push(FacetValue {
                        key,
                        name,
                        count: 1,
                        mine,
                    });
                }
            }
        }
    }
    for choice in chosen {
        if !found.iter().any(|option| option.key == choice.key) {
            found.push(FacetValue {
                key: choice.key.clone(),
                name: choice.name.clone(),
                count: 0,
                mine: false,
            });
        }
    }
    found.sort_by(|a, b| order(facet, a, b));
    found
}

fn order(facet: Facet, a: &FacetValue, b: &FacetValue) -> Ordering {
    let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
    match facet {
        Facet::Assignee => {
            let rank = |option: &FacetValue| {
                if option.mine {
                    0
                } else if option.key == NOBODY {
                    2
                } else {
                    1
                }
            };
            rank(a).cmp(&rank(b)).then_with(by_name)
        }
        Facet::Priority => priority_rank(&a.key)
            .cmp(&priority_rank(&b.key))
            .then_with(by_name),
        _ => by_name(),
    }
}

/// Tracker's priorities from the loudest; one of its own after them.
fn priority_rank(key: &str) -> u8 {
    match key.to_ascii_lowercase().as_str() {
        "blocker" | "highest" => 0,
        "critical" => 1,
        "major" | "high" => 2,
        "normal" | "medium" => 3,
        "minor" | "low" => 4,
        "trivial" | "lowest" => 5,
        _ => 6,
    }
}

/// A name for the filter being saved, from what it filters (the user may change it).
pub fn suggested_name(filter: &BoardFilter) -> String {
    let words = filter.describe();
    if words.is_empty() {
        tr("My filter")
    } else if words.chars().count() > 40 {
        let cut: String = words.chars().take(39).collect();
        format!("{}…", cut.trim_end())
    } else {
        words
    }
}

/// How many tasks a column shows now of how many it has: "2 of 5" — or just "5" when the filters
/// hide none of them.
pub fn count_text(shown: usize, all: usize) -> String {
    if shown == all {
        shown.to_string()
    } else {
        trf("{0} of {1}", &[&shown, &all])
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn task(value: Value) -> Issue {
        serde_json::from_value(value).unwrap()
    }

    fn tasks() -> Vec<Issue> {
        vec![
            task(json!({"key": "TRACK-142", "summary": "Ошибка входа через SSO",
                        "assignee": {"id": "1", "display": "Егор Агеев"},
                        "type": {"id": "1", "key": "bug", "display": "Ошибка"},
                        "priority": {"id": "4", "key": "critical", "display": "Критичный"},
                        "tags": ["sso", "auth"],
                        "components": [{"id": "11", "display": "Авторизация"}]})),
            task(json!({"key": "TRACK-130", "summary": "Таймауты выгрузки отчётов",
                        "assignee": {"id": "2", "display": "Анна Петрова"},
                        "type": {"id": "1", "key": "bug", "display": "Ошибка"},
                        "priority": {"id": "5", "key": "major", "display": "Высокий"},
                        "tags": ["export", "backend"],
                        "components": [{"id": "13", "display": "Отчёты"}]})),
            task(json!({"key": "TRACK-155", "summary": "Повторы в клиенте платежей",
                        "type": {"id": "2", "key": "task", "display": "Задача"},
                        "priority": {"id": "3", "key": "normal", "display": "Средний"},
                        "tags": ["backend"]})),
        ]
    }

    fn keys(tasks: &[Issue], filter: &BoardFilter) -> Vec<String> {
        tasks
            .iter()
            .filter(|task| filter.matches(task))
            .map(|task| task.key.clone())
            .collect()
    }

    #[test]
    fn any_value_of_a_facet_all_facets_together() {
        let tasks = tasks();
        let mut filter = BoardFilter::default();
        assert!(filter.is_empty());
        assert_eq!(keys(&tasks, &filter).len(), 3);
        filter.toggle(Facet::Tags, "backend", "backend");
        assert_eq!(keys(&tasks, &filter), ["TRACK-130", "TRACK-155"]);
        filter.toggle(Facet::Tags, "sso", "sso");
        assert_eq!(keys(&tasks, &filter).len(), 3);
        filter.toggle(Facet::Type, "bug", "Ошибка");
        assert_eq!(keys(&tasks, &filter), ["TRACK-142", "TRACK-130"]);
        filter.toggle(Facet::Assignee, NOBODY, "No assignee");
        assert!(keys(&tasks, &filter).is_empty());
        // Toggling again takes the value away.
        filter.toggle(Facet::Assignee, NOBODY, "No assignee");
        filter.clear(Facet::Type);
        filter.clear(Facet::Tags);
        assert!(filter.is_empty());
        filter.toggle(Facet::Assignee, NOBODY, "No assignee");
        assert_eq!(keys(&tasks, &filter), ["TRACK-155"]);
    }

    #[test]
    fn words_of_the_key_or_the_title() {
        let tasks = tasks();
        let filter = |text: &str| BoardFilter {
            text: text.into(),
            ..BoardFilter::default()
        };
        assert_eq!(keys(&tasks, &filter("sso")), ["TRACK-142"]);
        assert_eq!(keys(&tasks, &filter("track-13")), ["TRACK-130"]);
        assert_eq!(keys(&tasks, &filter("  ВХОДА   ошибка ")), ["TRACK-142"]);
        assert!(keys(&tasks, &filter("вход платеж")).is_empty());
    }

    #[test]
    fn a_facets_values_on_the_board() {
        let tasks = tasks();
        let me = Me::from_myself(&json!({"uid": "1"}));
        let assignees = options(&tasks, Facet::Assignee, &[], Some(&me));
        let names: Vec<(&str, usize, bool)> = assignees
            .iter()
            .map(|option| (option.name.as_str(), option.count, option.mine))
            .collect();
        assert_eq!(
            names,
            [("Егор Агеев", 1, true), ("Анна Петрова", 1, false), ("No assignee", 1, false)]
        );
        let priorities: Vec<String> = options(&tasks, Facet::Priority, &[], None)
            .into_iter()
            .map(|option| option.key)
            .collect();
        assert_eq!(priorities, ["critical", "major", "normal"]);
        let tags: Vec<(String, usize)> = options(&tasks, Facet::Tags, &[], None)
            .into_iter()
            .map(|option| (option.key, option.count))
            .collect();
        assert_eq!(
            tags,
            [
                ("auth".to_string(), 1),
                ("backend".to_string(), 2),
                ("export".to_string(), 1),
                ("sso".to_string(), 1)
            ]
        );
        // A chosen value no task has any more stays, to be taken away.
        let gone = [Choice {
            key: "qa".into(),
            name: "qa".into(),
        }];
        let tags = options(&tasks, Facet::Tags, &gone, None);
        assert!(tags.iter().any(|option| option.key == "qa" && option.count == 0));
    }

    #[test]
    fn chips_descriptions_and_saved_filters() {
        let mut filter = BoardFilter::default();
        assert_eq!(filter.chip(Facet::Assignee), "Assignee");
        filter.toggle(Facet::Assignee, "2", "Анна Петрова");
        assert_eq!(filter.chip(Facet::Assignee), "Assignee: Анна Петрова");
        filter.toggle(Facet::Tags, "sso", "sso");
        filter.toggle(Facet::Tags, "auth", "auth");
        filter.toggle(Facet::Tags, "qa", "qa");
        assert_eq!(filter.chip(Facet::Tags), "Tags: sso +2");
        filter.text = " login ".into();
        assert_eq!(filter.describe(), "Анна Петрова · sso, auth, qa · «login»");
        assert_eq!(suggested_name(&filter), "Анна Петрова · sso, auth, qa · «login»");
        assert_eq!(suggested_name(&BoardFilter::default()), "My filter");
        // The same filter whatever order its values were chosen in.
        let mut other = BoardFilter::default();
        other.toggle(Facet::Tags, "qa", "qa");
        other.toggle(Facet::Tags, "auth", "auth");
        other.toggle(Facet::Tags, "sso", "sso");
        other.toggle(Facet::Assignee, "2", "Анна Петрова");
        other.text = "LOGIN".into();
        assert!(filter.same_as(&other));
        other.toggle(Facet::Tags, "qa", "qa");
        assert!(!filter.same_as(&other));
        // Kept in the storage as JSON, and read back as it was.
        let saved = SavedFilter {
            name: "Анна: SSO".into(),
            filter: filter.clone(),
        };
        let text = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<SavedFilter>(&text).unwrap(), saved);
        // An older or partial filter reads with the rest empty.
        let partial: BoardFilter = serde_json::from_str(r#"{"text": "sso"}"#).unwrap();
        assert_eq!(partial.text, "sso");
        assert!(partial.tags.is_empty());
    }

    #[test]
    fn counts_of_a_column() {
        assert_eq!(count_text(5, 5), "5");
        assert_eq!(count_text(2, 5), "2 of 5");
    }

    #[test]
    fn facets_by_their_badges() {
        for facet in Facet::ALL {
            assert_eq!(Facet::of(facet.id()), Some(facet));
        }
        assert_eq!(Facet::of("only-mine"), None);
    }
}
