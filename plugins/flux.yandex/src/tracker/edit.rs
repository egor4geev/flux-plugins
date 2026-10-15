//! Changing a task's fields from its card: a click on an editable property opens a picker under
//! it (the assignee — the organization's people; the priority, the type, the sprint, the
//! components — what Tracker offers; the deadline and story points — quick choices) or a form (the
//! tags; a date or points of one's own; the title and the description), and the change goes to
//! Tracker as `PATCH /v3/issues/{key}` in its formats. The other fields of the task
//! ([`super::fields`]) change the same way by their type: a person — the people; a day — quick
//! choices; one of a list — the list; a line, several lines or a number — a form. The card shows
//! the task as Tracker returns it after the change; the board and the list hear of it.

use flux_plugin_api::dialog::{Form, Values};
use flux_plugin_api::picker::{self, Picker};
use flux_plugin_api::view::{SpanStyle, Tone, span};
use flux_plugin_api::{log, notify};
#[cfg(not(test))]
use flux_plugin_api::{tr, trf};
use serde_json::{Value, json};

#[cfg(test)]
use super::facts::{tr, trf};
use super::fields::{self, EditKind};
use super::model::{Component, Issue, Myself, Named, Priority, Sprint, User, id_text};
use super::{Asked, PickerPurpose, Tracker, WINDOW, api, facts};
use crate::auth::Auth;
use crate::time;

/// A day: (year, month, day).
pub type Day = (i64, u32, u32);

/// The properties of the card the user can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditField {
    Assignee,
    Priority,
    Type,
    Deadline,
    StoryPoints,
    Sprint,
    Tags,
    Components,
}

impl EditField {
    /// By the property's id on the card (`facts::card_fields`).
    pub fn of(id: &str) -> Option<EditField> {
        Some(match id {
            "assignee" => EditField::Assignee,
            "priority" => EditField::Priority,
            "type" => EditField::Type,
            "deadline" => EditField::Deadline,
            "story-points" => EditField::StoryPoints,
            "sprint" => EditField::Sprint,
            "tags" => EditField::Tags,
            "components" => EditField::Components,
            _ => return None,
        })
    }

    /// The property's id: a picker opens under `fields/<id>`.
    pub fn id(self) -> &'static str {
        match self {
            EditField::Assignee => "assignee",
            EditField::Priority => "priority",
            EditField::Type => "type",
            EditField::Deadline => "deadline",
            EditField::StoryPoints => "story-points",
            EditField::Sprint => "sprint",
            EditField::Tags => "tags",
            EditField::Components => "components",
        }
    }
}

// --- The changes, as Tracker takes them ---

/// The assignee by a login (or a uid); none — nobody.
pub fn assignee_change(handle: Option<&str>) -> Value {
    json!({ "assignee": handle })
}

/// A priority by its key ("critical").
pub fn priority_change(key: &str) -> Value {
    json!({ "priority": key })
}

/// A type by its key ("bug").
pub fn type_change(key: &str) -> Value {
    json!({ "type": key })
}

/// The deadline, "2026-10-20"; none — no deadline.
pub fn deadline_change(day: Option<Day>) -> Value {
    json!({ "deadline": day.map(date_text) })
}

/// Story points; none — none.
pub fn story_points_change(points: Option<f64>) -> Value {
    json!({ "storyPoints": points })
}

/// The tags, all of them (an empty list removes them).
pub fn tags_change(tags: &[String]) -> Value {
    json!({ "tags": tags })
}

/// A component added or taken away, by its id.
pub fn component_change(id: &str, add: bool) -> Value {
    let operation = if add { "add" } else { "remove" };
    json!({ "components": { operation: [id] } })
}

/// The sprint by its id; none — out of the sprints.
pub fn sprint_change(id: Option<&str>) -> Value {
    match id {
        Some(id) => json!({ "sprint": [{ "id": id }] }),
        None => json!({ "sprint": [] }),
    }
}

/// The title and the description, those that changed; none when nothing did.
pub fn task_change(issue: &Issue, summary: &str, description: &str) -> Option<Value> {
    let mut change = serde_json::Map::new();
    let summary = summary.trim();
    if !summary.is_empty() && summary != issue.summary.trim() {
        change.insert("summary".into(), Value::String(summary.to_string()));
    }
    let before = issue.description.as_deref().unwrap_or_default();
    if description.trim_end() != before.trim_end() {
        change.insert("description".into(), Value::String(description.trim_end().to_string()));
    }
    (!change.is_empty()).then_some(Value::Object(change))
}

// --- Days and numbers as people write them ---

/// "2026-10-20".
pub fn date_text((year, month, day): Day) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// The day of the week: 0 — Monday … 6 — Sunday.
pub fn weekday((year, month, day): Day) -> i64 {
    (time::days_from_civil(year, month, day) + 3).rem_euclid(7)
}

/// The day `days` after `day`.
pub fn add_days((year, month, day): Day, days: i64) -> Day {
    time::civil_from_days(time::days_from_civil(year, month, day) + days)
}

/// The quick choices of a deadline, from today, in order: today, tomorrow, the end of the week
/// (this Friday, when it is after tomorrow), next week (next Monday), in two weeks — a day that is
/// already among them isn't repeated.
pub fn deadline_choices(today: Day) -> Vec<(&'static str, Day)> {
    let weekday = weekday(today);
    let mut choices = vec![("today", today), ("tomorrow", add_days(today, 1))];
    if weekday < 3 {
        choices.push(("friday", add_days(today, 4 - weekday)));
    }
    choices.push(("monday", add_days(today, 7 - weekday)));
    choices.push(("two-weeks", add_days(today, 14)));
    let mut seen: Vec<Day> = Vec::new();
    choices.retain(|(_, day)| {
        let new = !seen.contains(day);
        seen.push(*day);
        new
    });
    choices
}

/// A day as people write it: "2026-10-20", "20.10.2026", "20.10", "20/10". Without the year — this
/// one, or the next when the date has passed; none when it isn't a real day.
pub fn parse_day(text: &str, today: Day) -> Option<Day> {
    let text = text.trim();
    if let Some(day) = time::parse_date(text).filter(|_| text.len() == 10) {
        return valid(day);
    }
    let parts: Vec<&str> = text.split(['.', '/']).map(str::trim).collect();
    let number = |part: &str| part.parse::<i64>().ok();
    let (day, month, year) = match parts.as_slice() {
        [day, month] => (number(day)?, number(month)?, None),
        [day, month, year] => {
            let year = number(year)?;
            (number(day)?, number(month)?, Some(if year < 100 { 2000 + year } else { year }))
        }
        _ => return None,
    };
    let (month, day) = (u32::try_from(month).ok()?, u32::try_from(day).ok()?);
    match year {
        Some(year) => valid((year, month, day)),
        None => {
            let this_year = valid((today.0, month, day))?;
            if this_year < today {
                valid((today.0 + 1, month, day))
            } else {
                Some(this_year)
            }
        }
    }
}

/// The day, if the calendar has it (no 31 June).
fn valid((year, month, day): Day) -> Option<Day> {
    let real = time::civil_from_days(time::days_from_civil(year, month, day));
    (real == (year, month, day) && (1..=12).contains(&month) && day >= 1).then_some(real)
}

/// Story points as written: "3", "0.5", "1,5"; none when it isn't a sensible number.
pub fn parse_points(text: &str) -> Option<f64> {
    let points: f64 = text.trim().replace(',', ".").parse().ok()?;
    (points.is_finite() && (0.0..=1000.0).contains(&points)).then_some(points)
}

/// Tags as written: by commas, semicolons or lines, trimmed, without repeats, in order.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in text.split([',', ';', '\n']).map(str::trim) {
        if !tag.is_empty() && !tags.iter().any(|known| known == tag) {
            tags.push(tag.to_string());
        }
    }
    tags
}

/// A person's field: an id as Tracker gives it (`{"id": …}`), or a login.
fn person_id(value: &Value) -> Option<String> {
    match value {
        Value::Object(person) => person.get("id").map(id_text),
        Value::String(login) if !login.is_empty() => Some(login.clone()),
        _ => None,
    }
}

/// The picker of a person's field (the assignee, a QA engineer): you first, then `nobody` (No
/// assignee), then the people; the current one (by id; none — `nobody`) checked.
fn people_picker(
    me: Option<&Myself>,
    users: &[User],
    current: Option<&str>,
    nobody: &str,
) -> Picker {
    let check = |item: picker::Item, on: bool| if on { item.icon("check") } else { item };
    let mut picker = Picker::new(&tr("Find a person"));
    if let Some(me) = me {
        let mine = current == Some(id_text(&me.uid).as_str()) || current == Some(me.login.as_str());
        picker = picker.item(check(person("me", &me.display, &me.login).detail(&tr("you")), mine));
    }
    picker = picker.item(check(picker::item("none", nobody), current.is_none()));
    let people = tr("People");
    for user in users {
        if me.is_some_and(|me| id_text(&me.uid) == id_text(&user.uid)) {
            continue;
        }
        let on = current == Some(id_text(&user.uid).as_str()) || current == Some(user.login.as_str());
        let key = format!("user:{}", user.handle());
        picker = picker.item(check(person(&key, &user.display, &user.login).group(&people), on));
    }
    picker
}

/// A person of the assignee picker: the login is a part of the label, so Flux finds people by
/// their name or their login.
fn person(key: &str, display: &str, login: &str) -> picker::Item {
    picker::item(key, display).spans([span(display), span(&format!("  {login}")).tone(Tone::Dim)])
}

/// A sprint's state, in words.
fn sprint_state(status: &str) -> String {
    match status {
        "in_progress" | "inProgress" => tr("in progress"),
        "draft" => tr("planned"),
        other => other.to_string(),
    }
}

impl Tracker {
    /// A click on an editable property of the card.
    pub(super) fn edit_field(&mut self, id: &str, auth: &Auth) {
        let Some(key) = self.card_key() else {
            return;
        };
        let Some(issue) = self.card.as_ref().and_then(|card| card.issue.clone()) else {
            return;
        };
        let Some(field) = EditField::of(id) else {
            return self.edit_other_field(&key, &issue, id, auth);
        };
        match field {
            EditField::Assignee => self.assignee_picker(&key, &issue, auth),
            EditField::Priority => self.priority_picker(&key, &issue, auth),
            EditField::Type => self.type_picker(&key, &issue, auth),
            EditField::Deadline => self.deadline_picker(&key, &issue),
            EditField::StoryPoints => self.points_picker(&key, &issue),
            EditField::Sprint => self.sprint_picker(&key, &issue, auth),
            EditField::Tags => {
                let id = tags_form(&issue.tags.join(", "), None).send();
                self.asked.insert(id, Asked::Tags(key));
            }
            EditField::Components => self.components_picker(&key, &issue, auth),
        }
    }

    fn open_field_picker(&mut self, key: &str, field: EditField, picker: Picker) {
        let id = picker
            .under(WINDOW, &format!("fields/{}", field.id()))
            .open();
        self.pickers.insert(
            id,
            PickerPurpose::Field {
                task: key.to_string(),
                field,
            },
        );
    }

    /// The organization's people, read once.
    fn learn_users(&mut self, auth: &Auth) {
        if self.users.is_none() {
            match api::users(auth) {
                Ok(mut users) => {
                    users.sort_by_key(|user| user.display.to_lowercase());
                    self.users = Some(users);
                }
                Err(err) => log::warn(&format!("Users: {}", err.message())),
            }
        }
    }

    fn assignee_picker(&mut self, key: &str, issue: &Issue, auth: &Auth) {
        self.learn_users(auth);
        let current = issue.assignee.as_ref().map(|assignee| id_text(&assignee.id));
        let users = self.users.as_deref().unwrap_or_default();
        let picker = people_picker(self.me.as_ref(), users, current.as_deref(), &tr("No assignee"));
        self.open_field_picker(key, EditField::Assignee, picker);
    }

    fn priority_picker(&mut self, key: &str, issue: &Issue, auth: &Auth) {
        if self.priorities.is_empty() {
            match api::priorities(auth) {
                Ok(mut priorities) => {
                    priorities.sort_by_key(|priority| std::cmp::Reverse(priority.order));
                    self.priorities = priorities;
                }
                Err(err) => {
                    notify::warning(&tr("Couldn't read the priorities"), Some(&err.message()));
                    return;
                }
            }
        }
        let current = issue.priority.as_ref().map(Named::key_or_id);
        let items = self.priorities.iter().map(|priority: &Priority| {
            let item = picker::item(&format!("priority:{}", priority.key), &priority.display());
            let tone = super::facts::priority_tag(&Named {
                key: Some(priority.key.clone()),
                display: priority.display(),
                ..Named::default()
            })
            .map(|(_, tone)| tone)
            .unwrap_or(Tone::Normal);
            let item = item.spans([span(&priority.display()).tone(tone)]);
            if current.as_deref() == Some(priority.key.as_str()) {
                item.icon("check")
            } else {
                item
            }
        });
        let picker = Picker::menu().items(items);
        self.open_field_picker(key, EditField::Priority, picker);
    }

    fn type_picker(&mut self, key: &str, issue: &Issue, auth: &Auth) {
        let Some(queue) = issue.queue.as_ref().and_then(|queue| queue.key.clone()) else {
            return;
        };
        if !self.issue_types.contains_key(&queue) {
            match api::issue_types(auth, &queue) {
                Ok(types) => {
                    self.issue_types.insert(queue.clone(), types);
                }
                Err(err) => {
                    notify::warning(&tr("Couldn't read the queue's types"), Some(&err.message()));
                    return;
                }
            }
        }
        let current = issue.kind.as_ref().map(Named::key_or_id);
        let types = self.issue_types.get(&queue).cloned().unwrap_or_default();
        if types.is_empty() {
            notify::info(&tr("The queue offers no other types"));
            return;
        }
        let items = types.iter().map(|kind| {
            let key = kind.key_or_id();
            let item = picker::item(&format!("type:{key}"), &kind.display);
            if current.as_deref() == Some(key.as_str()) {
                item.icon("check")
            } else {
                item
            }
        });
        let picker = Picker::menu().items(items);
        self.open_field_picker(key, EditField::Type, picker);
    }

    fn deadline_picker(&mut self, key: &str, issue: &Issue) {
        let current = issue.deadline.as_deref().and_then(time::parse_date);
        let mut picker = date_menu(current);
        if current.is_some() {
            picker = picker.item(picker::item("clear", &tr("No Deadline")));
        }
        self.open_field_picker(key, EditField::Deadline, picker);
    }

    fn points_picker(&mut self, key: &str, issue: &Issue) {
        let mut picker = Picker::menu();
        for points in [1.0, 2.0, 3.0, 5.0, 8.0, 13.0] {
            let text = facts::points(points);
            let item = picker::item(&format!("points:{text}"), &text);
            picker = picker.item(if issue.story_points == Some(points) {
                item.icon("check")
            } else {
                item
            });
        }
        picker = picker.item(picker::item("other", &tr("Other…")));
        if issue.story_points.is_some() {
            picker = picker.item(picker::item("clear", &tr("No Story Points")));
        }
        self.open_field_picker(key, EditField::StoryPoints, picker);
    }

    fn sprint_picker(&mut self, key: &str, issue: &Issue, auth: &Auth) {
        let mut sprints: Vec<Sprint> = Vec::new();
        for board in &issue.boards {
            match api::sprints(auth, &id_text(&board.id)) {
                Ok(found) => {
                    // A finished sprint takes no tasks.
                    let finished = |sprint: &Sprint| {
                        sprint.archived || matches!(sprint.status.as_str(), "released" | "archived")
                    };
                    sprints.extend(found.into_iter().filter(|sprint| !finished(sprint)));
                }
                Err(err) => log::warn(&format!("Sprints: {}", err.message())),
            }
        }
        sprints.dedup_by(|a, b| id_text(&a.id) == id_text(&b.id));
        let current: Vec<String> = match &issue.sprint {
            Some(Value::Array(list)) => list.iter().map(|sprint| id_text(&sprint["id"])).collect(),
            Some(sprint) => vec![id_text(&sprint["id"])],
            None => Vec::new(),
        };
        if sprints.is_empty() && current.is_empty() {
            notify::info(&tr("The task's boards have no sprints to plan it into"));
            return;
        }
        let mut picker = Picker::menu();
        for sprint in &sprints {
            let id = id_text(&sprint.id);
            let item = picker::item(&format!("sprint:{id}"), &sprint.name)
                .detail(&sprint_state(&sprint.status));
            picker = picker.item(if current.contains(&id) { item.icon("check") } else { item });
        }
        if !current.is_empty() {
            picker = picker.item(picker::item("none", &tr("Out of the Sprint")));
        }
        self.open_field_picker(key, EditField::Sprint, picker);
    }

    fn components_picker(&mut self, key: &str, issue: &Issue, auth: &Auth) {
        let Some(queue) = issue.queue.as_ref().and_then(|queue| queue.key.clone()) else {
            return;
        };
        if !self.components.contains_key(&queue) {
            match api::components(auth, &queue) {
                Ok(components) => {
                    self.components.insert(queue.clone(), components);
                }
                Err(err) => {
                    notify::warning(
                        &tr("Couldn't read the queue's components"),
                        Some(&err.message()),
                    );
                    return;
                }
            }
        }
        let components: Vec<Component> = self.components.get(&queue).cloned().unwrap_or_default();
        if components.is_empty() {
            notify::info(&tr("The queue has no components"));
            return;
        }
        let chosen: Vec<String> = issue.components.iter().map(|c| id_text(&c.id)).collect();
        let items = components.iter().map(|component| {
            let id = id_text(&component.id);
            let item = picker::item(&format!("component:{id}"), &component.name);
            if chosen.contains(&id) {
                item.icon("check").detail(&tr("click to remove"))
            } else {
                item
            }
        });
        let picker = if components.len() > 8 {
            Picker::new(&tr("Find a component")).items(items)
        } else {
            Picker::menu().items(items)
        };
        self.open_field_picker(key, EditField::Components, picker);
    }

    /// A click on one of the other fields of the card: a picker under it or a form, by its type.
    fn edit_other_field(&mut self, key: &str, issue: &Issue, field: &str, auth: &Auth) {
        let metas = self.metas_of(issue);
        let Some(meta) = fields::find_meta(&metas, field) else {
            return;
        };
        let Some(kind) = fields::edit_kind(meta) else {
            return;
        };
        let name = meta.display();
        let current = issue.other.get(field).cloned().unwrap_or(Value::Null);
        let picker = match &kind {
            EditKind::User => {
                self.learn_users(auth);
                let users = self.users.as_deref().unwrap_or_default();
                let current = person_id(&current);
                people_picker(self.me.as_ref(), users, current.as_deref(), &tr("No One"))
            }
            EditKind::Date => {
                let current = current.as_str().and_then(time::parse_date);
                let picker = date_menu(current);
                if current.is_some() && !meta.schema.required {
                    picker.item(picker::item("clear", &tr("No Date")))
                } else {
                    picker
                }
            }
            EditKind::Choice(options) => {
                let current = crate::tracker::model::value_text(&current);
                let mut picker = Picker::menu();
                for option in options {
                    let item = picker::item(&format!("option:{option}"), option);
                    picker = picker.item(if *option == current { item.icon("check") } else { item });
                }
                if !current.is_empty() && !meta.schema.required {
                    picker = picker.item(picker::item("clear", &tr("None")));
                }
                picker
            }
            EditKind::Text | EditKind::Multiline | EditKind::Number { .. } => {
                let text = match &current {
                    Value::Number(number) => number.as_f64().map(facts::points).unwrap_or_default(),
                    other => crate::tracker::model::value_text(other),
                };
                let id = value_form(&name, &kind, &text, None).send();
                self.asked.insert(
                    id,
                    Asked::OtherField {
                        task: key.to_string(),
                        field: field.to_string(),
                        kind,
                    },
                );
                return;
            }
        };
        let id = picker.under(WINDOW, &format!("fields/{field}")).open();
        self.pickers.insert(
            id,
            PickerPurpose::OtherField {
                task: key.to_string(),
                field: field.to_string(),
            },
        );
    }

    /// What was chosen in the picker of one of the other fields.
    pub(super) fn other_field_picked(&mut self, task: &str, field: &str, choice: &str, auth: &Auth) {
        let value = match choice {
            "me" => match &self.me {
                Some(me) if !me.login.is_empty() => Value::String(me.login.clone()),
                Some(me) => Value::String(id_text(&me.uid)),
                None => return,
            },
            "none" | "clear" => Value::Null,
            "pick" => {
                let issue = self
                    .card
                    .as_ref()
                    .filter(|card| card.key == task)
                    .and_then(|card| card.issue.clone());
                let current = issue
                    .as_ref()
                    .and_then(|issue| issue.other.get(field)?.as_str().map(str::to_string))
                    .unwrap_or_default();
                let name = issue
                    .as_ref()
                    .and_then(|issue| {
                        let metas = self.metas_of(issue);
                        fields::find_meta(&metas, field).map(|meta| meta.display())
                    })
                    .unwrap_or_else(|| field.to_string());
                let id = value_form(&name, &EditKind::Date, &current, None).send();
                self.asked.insert(
                    id,
                    Asked::OtherField {
                        task: task.to_string(),
                        field: field.to_string(),
                        kind: EditKind::Date,
                    },
                );
                return;
            }
            choice => {
                if let Some(handle) = choice.strip_prefix("user:") {
                    Value::String(handle.to_string())
                } else if let Some(day) = choice.strip_prefix("day:") {
                    Value::String(day.to_string())
                } else if let Some(option) = choice.strip_prefix("option:") {
                    Value::String(option.to_string())
                } else {
                    return;
                }
            }
        };
        self.apply_change(task, &other_change(field, value), auth);
    }

    /// The answer to the form of one of the other fields: the value, or the form again with what
    /// was wrong.
    pub(super) fn other_field_answered(
        &mut self,
        task: String,
        field: String,
        kind: EditKind,
        values: Values,
        auth: &Auth,
    ) {
        let now = time::now();
        let today = super::facts::today(now, time::offset_at(now));
        let text = match kind {
            EditKind::Multiline => values.get("value").unwrap_or_default().trim_end().to_string(),
            _ => values.text("value"),
        };
        match parse_value(&kind, &text, today) {
            Ok(value) => self.apply_change(&task, &other_change(&field, value), auth),
            Err(error) => {
                let name = self
                    .card
                    .as_ref()
                    .and_then(|card| card.issue.as_ref())
                    .and_then(|issue| {
                        let metas = self.metas_of(issue);
                        fields::find_meta(&metas, &field).map(|meta| meta.display())
                    })
                    .unwrap_or_else(|| field.clone());
                let id = value_form(&name, &kind, &text, Some(&error)).send();
                self.asked.insert(id, Asked::OtherField { task, field, kind });
            }
        }
    }

    /// What was chosen in a property's picker.
    pub(super) fn field_picked(&mut self, task: &str, field: EditField, choice: &str, auth: &Auth) {
        let issue = self
            .card
            .as_ref()
            .filter(|card| card.key == task)
            .and_then(|card| card.issue.clone());
        let change = match (field, choice) {
            (EditField::Assignee, "me") => match &self.me {
                Some(me) if !me.login.is_empty() => assignee_change(Some(&me.login)),
                Some(me) => assignee_change(Some(&id_text(&me.uid))),
                None => return,
            },
            (EditField::Assignee, "none") => assignee_change(None),
            (EditField::Assignee, user) => match user.strip_prefix("user:") {
                Some(handle) => assignee_change(Some(handle)),
                None => return,
            },
            (EditField::Priority, priority) => match priority.strip_prefix("priority:") {
                Some(key) => priority_change(key),
                None => return,
            },
            (EditField::Type, kind) => match kind.strip_prefix("type:") {
                Some(key) => type_change(key),
                None => return,
            },
            (EditField::Deadline, "pick") => {
                let current = issue.and_then(|issue| issue.deadline).unwrap_or_default();
                let id = date_form(&current, None).send();
                self.asked.insert(id, Asked::Deadline(task.to_string()));
                return;
            }
            (EditField::Deadline, "clear") => deadline_change(None),
            (EditField::Deadline, day) => match day.strip_prefix("day:").and_then(time::parse_date)
            {
                Some(day) => deadline_change(Some(day)),
                None => return,
            },
            (EditField::StoryPoints, "other") => {
                let current = issue
                    .and_then(|issue| issue.story_points)
                    .map(facts::points)
                    .unwrap_or_default();
                let id = points_form(&current, None).send();
                self.asked.insert(id, Asked::StoryPoints(task.to_string()));
                return;
            }
            (EditField::StoryPoints, "clear") => story_points_change(None),
            (EditField::StoryPoints, points) => {
                match points.strip_prefix("points:").and_then(parse_points) {
                    Some(points) => story_points_change(Some(points)),
                    None => return,
                }
            }
            (EditField::Sprint, "none") => sprint_change(None),
            (EditField::Sprint, sprint) => match sprint.strip_prefix("sprint:") {
                Some(id) => sprint_change(Some(id)),
                None => return,
            },
            (EditField::Components, component) => {
                let Some(id) = component.strip_prefix("component:") else {
                    return;
                };
                let chosen = issue.is_some_and(|issue| {
                    issue.components.iter().any(|known| id_text(&known.id) == id)
                });
                component_change(id, !chosen)
            }
            (EditField::Tags, _) => return,
        };
        self.apply_change(task, &change, auth);
    }

    /// The answer to a property's form, or to Edit Task.
    pub(super) fn edit_form_answered(&mut self, asked: Asked, values: Values, auth: &Auth) {
        let now = time::now();
        let today = super::facts::today(now, time::offset_at(now));
        match asked {
            Asked::Deadline(key) => {
                let text = values.text("date");
                if text.is_empty() {
                    return self.apply_change(&key, &deadline_change(None), auth);
                }
                match parse_day(&text, today) {
                    Some(day) => self.apply_change(&key, &deadline_change(Some(day)), auth),
                    None => {
                        let error = tr("Write the date as 2026-10-20 or 20.10");
                        let id = date_form(&text, Some(&error)).send();
                        self.asked.insert(id, Asked::Deadline(key));
                    }
                }
            }
            Asked::StoryPoints(key) => {
                let text = values.text("points");
                if text.is_empty() {
                    return self.apply_change(&key, &story_points_change(None), auth);
                }
                match parse_points(&text) {
                    Some(points) => {
                        self.apply_change(&key, &story_points_change(Some(points)), auth)
                    }
                    None => {
                        let error = tr("Write a number: 3, 0.5");
                        let id = points_form(&text, Some(&error)).send();
                        self.asked.insert(id, Asked::StoryPoints(key));
                    }
                }
            }
            Asked::Tags(key) => {
                let tags = parse_tags(&values.text("tags"));
                self.apply_change(&key, &tags_change(&tags), auth);
            }
            Asked::EditTask(key) => {
                let Some(issue) = self
                    .card
                    .as_ref()
                    .filter(|card| card.key == key)
                    .and_then(|card| card.issue.clone())
                else {
                    return;
                };
                let summary = values.text("summary");
                let description = values.get("description").unwrap_or_default().to_string();
                if summary.is_empty() {
                    let id = task_form(&issue, Some(&summary), Some(&description))
                        .error(&tr("The task needs a title"))
                        .send();
                    self.asked.insert(id, Asked::EditTask(key));
                    return;
                }
                if let Some(change) = task_change(&issue, &summary, &description) {
                    self.apply_change(&key, &change, auth);
                }
            }
            Asked::OtherField { task, field, kind } => {
                self.other_field_answered(task, field, kind, values, auth)
            }
            Asked::CustomQuery | Asked::Close(_) | Asked::LogTime(_) => {}
        }
    }

    /// Edit Task: the title and the description in a form.
    pub(super) fn edit_task(&mut self) {
        let Some(card) = self.card.as_ref() else {
            return;
        };
        let Some(issue) = card.issue.as_ref() else {
            return;
        };
        let id = task_form(issue, None, None).send();
        self.asked.insert(id, Asked::EditTask(card.key.clone()));
    }

    /// Sends a change of the task and shows it as Tracker returns it; the board and the list hear
    /// of it. A refusal is told, and the card stays as it was.
    fn apply_change(&mut self, key: &str, change: &Value, auth: &Auth) {
        match api::update(auth, key, change) {
            Ok(issue) => {
                let mut history_shown = false;
                if let Some(card) = self.card.as_mut().filter(|card| card.key == key) {
                    card.issue = Some(issue);
                    card.history_loaded = false;
                    history_shown = card.tab == super::window::Tab::History;
                }
                if history_shown {
                    self.load_history(auth, false);
                }
                self.mark_changed(key);
            }
            Err(err) => {
                notify::warning(&trf("Couldn't change {0}", &[&key]), Some(&err.message()));
            }
        }
        self.render(auth);
    }
}

/// The quick choices of a day, from today (the current one checked), and Pick a Date…; a field
/// that can be emptied adds its own item.
fn date_menu(current: Option<Day>) -> Picker {
    let now = time::now();
    let today = super::facts::today(now, time::offset_at(now));
    let language = flux_plugin_api::host::i18n::language();
    let day_text = |day: Day| time::day_month(day.0, day.1, day.2, today.0, &language);
    let label = |choice: &str| match choice {
        "today" => tr("Today"),
        "tomorrow" => tr("Tomorrow"),
        "friday" => tr("End of the week"),
        "monday" => tr("Next week"),
        _ => tr("In two weeks"),
    };
    let mut picker = Picker::menu();
    for (choice, day) in deadline_choices(today) {
        let item = picker::item(&format!("day:{}", date_text(day)), &label(choice))
            .detail(&day_text(day));
        picker = picker.item(if current == Some(day) { item.icon("check") } else { item });
    }
    picker.item(picker::item("pick", &tr("Pick a Date…")))
}

/// A change of one of the other fields: `{"<its key in the task>": value}` (a local field's key is
/// `<queue's id>--<key>`, as the task has it); null empties it.
pub fn other_change(field: &str, value: Value) -> Value {
    let mut change = serde_json::Map::new();
    change.insert(field.to_string(), value);
    Value::Object(change)
}

/// A value typed for one of the other fields, as Tracker takes it; empty — null. A number or a day
/// that isn't one says how to write it.
pub fn parse_value(kind: &EditKind, text: &str, today: Day) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    match kind {
        EditKind::Number { whole } => {
            let wrong = || {
                if *whole {
                    tr("Write a whole number: 3")
                } else {
                    tr("Write a number: 3, 0.5")
                }
            };
            let number: f64 = text.trim().replace(',', ".").parse().map_err(|_| wrong())?;
            if !number.is_finite() || (*whole && number.fract() != 0.0) {
                return Err(wrong());
            }
            Ok(if number.fract() == 0.0 && number.abs() < 1e15 {
                json!(number as i64)
            } else {
                json!(number)
            })
        }
        EditKind::Date => parse_day(text, today)
            .map(|day| Value::String(date_text(day)))
            .ok_or_else(|| tr("Write the date as 2026-10-20 or 20.10")),
        _ => Ok(Value::String(text.to_string())),
    }
}

/// The form of a value of one's own for one of the other fields, by its type.
fn value_form(name: &str, kind: &EditKind, text: &str, error: Option<&str>) -> Form {
    let form = Form::new(name, &tr("Set"));
    let form = match kind {
        EditKind::Multiline => form.multiline("value", name, text),
        EditKind::Number { .. } => form
            .text("value", name, text)
            .placeholder("3")
            .note("value-note", &tr("Empty — no value")),
        EditKind::Date => form
            .text("value", name, text)
            .placeholder(&tr("2026-10-20 or 20.10"))
            .note("value-note", &tr("Empty — no date")),
        _ => form
            .text("value", name, text)
            .note("value-note", &tr("Empty — no value")),
    };
    match error {
        Some(error) => form.error(error),
        None => form,
    }
}

/// The form of a date of one's own.
fn date_form(text: &str, error: Option<&str>) -> Form {
    let form = Form::new(&tr("Deadline"), &tr("Set"))
        .text("date", &tr("Date"), text)
        .placeholder(&tr("2026-10-20 or 20.10"))
        .note("date-note", &tr("Empty — no deadline"));
    match error {
        Some(error) => form.error(error),
        None => form,
    }
}

/// The form of story points of one's own.
fn points_form(text: &str, error: Option<&str>) -> Form {
    let form = Form::new(&tr("Story Points"), &tr("Set"))
        .text("points", &tr("Story points"), text)
        .placeholder("3")
        .note("points-note", &tr("Empty — no story points"));
    match error {
        Some(error) => form.error(error),
        None => form,
    }
}

/// The form of the tags.
fn tags_form(text: &str, error: Option<&str>) -> Form {
    let form = Form::new(&tr("Tags"), &tr("Save"))
        .text("tags", &tr("Tags"), text)
        .placeholder(&tr("backend, frontend"))
        .note("tags-note", &tr("Separate the tags with commas; empty — no tags"));
    match error {
        Some(error) => form.error(error),
        None => form,
    }
}

/// Edit Task: the title and the description (Markdown).
fn task_form(issue: &Issue, summary: Option<&str>, description: Option<&str>) -> Form {
    Form::new(&trf("Edit {0}", &[&issue.key]), &tr("Save"))
        .text("summary", &tr("Title"), summary.unwrap_or(&issue.summary))
        .multiline(
            "description",
            &tr("Description"),
            description.unwrap_or(issue.description.as_deref().unwrap_or_default()),
        )
        .placeholder(&tr("Markdown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thursday, 15 October 2026.
    const TODAY: Day = (2026, 10, 15);

    #[test]
    fn changes_in_trackers_formats() {
        assert_eq!(assignee_change(Some("egor")), json!({"assignee": "egor"}));
        assert_eq!(assignee_change(None), json!({"assignee": null}));
        assert_eq!(priority_change("critical"), json!({"priority": "critical"}));
        assert_eq!(type_change("bug"), json!({"type": "bug"}));
        assert_eq!(deadline_change(Some((2026, 10, 20))), json!({"deadline": "2026-10-20"}));
        assert_eq!(deadline_change(None), json!({"deadline": null}));
        assert_eq!(story_points_change(Some(3.0)), json!({"storyPoints": 3.0}));
        assert_eq!(story_points_change(None), json!({"storyPoints": null}));
        assert_eq!(
            tags_change(&["a".into(), "b".into()]),
            json!({"tags": ["a", "b"]})
        );
        assert_eq!(component_change("11", true), json!({"components": {"add": ["11"]}}));
        assert_eq!(component_change("11", false), json!({"components": {"remove": ["11"]}}));
        assert_eq!(sprint_change(Some("672")), json!({"sprint": [{"id": "672"}]}));
        assert_eq!(sprint_change(None), json!({"sprint": []}));
    }

    #[test]
    fn edit_task_sends_only_what_changed() {
        let issue = Issue {
            key: "TRACK-1".into(),
            summary: "Title".into(),
            description: Some("Text\n".into()),
            ..Issue::default()
        };
        assert_eq!(task_change(&issue, "Title", "Text"), None);
        assert_eq!(
            task_change(&issue, " New title ", "Text"),
            Some(json!({"summary": "New title"}))
        );
        assert_eq!(
            task_change(&issue, "Title", "More text"),
            Some(json!({"description": "More text"}))
        );
        // An empty title isn't sent: the form asks again.
        assert_eq!(task_change(&issue, "  ", "Text"), None);
    }

    #[test]
    fn deadline_quick_choices() {
        // Wednesday: Friday two days on, next Monday five, two weeks.
        let wednesday = (2026, 10, 14);
        assert_eq!(weekday(wednesday), 2);
        assert_eq!(
            deadline_choices(wednesday),
            vec![
                ("today", (2026, 10, 14)),
                ("tomorrow", (2026, 10, 15)),
                ("friday", (2026, 10, 16)),
                ("monday", (2026, 10, 19)),
                ("two-weeks", (2026, 10, 28)),
            ]
        );
        // Thursday: Friday is tomorrow — not twice.
        assert_eq!(weekday(TODAY), 3);
        let thursday: Vec<&str> = deadline_choices(TODAY).iter().map(|(key, _)| *key).collect();
        assert_eq!(thursday, vec!["today", "tomorrow", "monday", "two-weeks"]);
        // Sunday: next Monday is tomorrow.
        let sunday = (2026, 10, 18);
        assert_eq!(weekday(sunday), 6);
        assert_eq!(
            deadline_choices(sunday),
            vec![
                ("today", (2026, 10, 18)),
                ("tomorrow", (2026, 10, 19)),
                ("two-weeks", (2026, 11, 1)),
            ]
        );
        // Across a year.
        assert_eq!(deadline_choices((2026, 12, 31))[1], ("tomorrow", (2027, 1, 1)));
    }

    #[test]
    fn days_as_people_write_them() {
        assert_eq!(parse_day("2026-10-20", TODAY), Some((2026, 10, 20)));
        assert_eq!(parse_day("20.10", TODAY), Some((2026, 10, 20)));
        assert_eq!(parse_day("20/10", TODAY), Some((2026, 10, 20)));
        assert_eq!(parse_day("20.10.2027", TODAY), Some((2027, 10, 20)));
        assert_eq!(parse_day("1.3.27", TODAY), Some((2027, 3, 1)));
        // A day that has passed this year is next year's.
        assert_eq!(parse_day("10.01", TODAY), Some((2027, 1, 10)));
        assert_eq!(parse_day("15.10", TODAY), Some((2026, 10, 15)));
        assert_eq!(parse_day("31.06", TODAY), None);
        assert_eq!(parse_day("2026-02-30", TODAY), None);
        assert_eq!(parse_day("soon", TODAY), None);
        assert_eq!(parse_day("", TODAY), None);
    }

    #[test]
    fn points_and_tags_as_people_write_them() {
        assert_eq!(parse_points("3"), Some(3.0));
        assert_eq!(parse_points("1,5"), Some(1.5));
        assert_eq!(parse_points(" 0.5 "), Some(0.5));
        assert_eq!(parse_points("-1"), None);
        assert_eq!(parse_points("lots"), None);
        assert_eq!(facts::points(3.0), "3");
        assert_eq!(facts::points(0.5), "0.5");
        assert_eq!(
            parse_tags("backend, bff;backend\n qa ,,"),
            vec!["backend".to_string(), "bff".to_string(), "qa".to_string()]
        );
        assert!(parse_tags("  ").is_empty());
    }

    #[test]
    fn other_fields_values_as_tracker_takes_them() {
        let number = EditKind::Number { whole: false };
        let whole = EditKind::Number { whole: true };
        assert_eq!(parse_value(&number, "2,5", TODAY), Ok(json!(2.5)));
        assert_eq!(parse_value(&whole, "120", TODAY), Ok(json!(120)));
        assert!(parse_value(&whole, "2.5", TODAY).is_err());
        assert!(parse_value(&number, "lots", TODAY).is_err());
        assert_eq!(parse_value(&EditKind::Date, "20.10", TODAY), Ok(json!("2026-10-20")));
        assert!(parse_value(&EditKind::Date, "31.06", TODAY).is_err());
        assert_eq!(parse_value(&EditKind::Text, "stage", TODAY), Ok(json!("stage")));
        // Empty empties the field.
        assert_eq!(parse_value(&whole, "  ", TODAY), Ok(Value::Null));
        assert_eq!(
            other_change("64f1c0a2e4b0a1b2c3d4e5f6--team", json!("Платежи")),
            json!({"64f1c0a2e4b0a1b2c3d4e5f6--team": "Платежи"})
        );
    }

    #[test]
    fn properties_by_id() {
        for field in [
            EditField::Assignee,
            EditField::Priority,
            EditField::Type,
            EditField::Deadline,
            EditField::StoryPoints,
            EditField::Sprint,
            EditField::Tags,
            EditField::Components,
        ] {
            assert_eq!(EditField::of(field.id()), Some(field));
        }
        assert_eq!(EditField::of("author"), None);
    }

    #[test]
    fn people_are_found_by_name_or_login() {
        let person = |uid: u64, login: &str, display: &str| User {
            uid: json!(uid),
            login: login.into(),
            display: display.into(),
            ..User::default()
        };
        let me = Myself {
            uid: json!(1),
            login: "egor".into(),
            display: "Егор Агеев".into(),
            ..Myself::default()
        };
        let users = [person(1, "egor", "Егор Агеев"), person(2, "anna.petrova", "Анна Петрова")];
        let picker = people_picker(Some(&me), &users, Some("2"), "No assignee");
        let items = &picker.spec().items;
        // You first, then No assignee, then the others — you aren't among them twice.
        let keys: Vec<&str> = items.iter().map(|item| item.key.as_str()).collect();
        assert_eq!(keys, ["me", "none", "user:anna.petrova"]);
        // Flux's search matches the label: the name and the login are both in it.
        let label: String = items[2].label.iter().map(|span| span.text.as_str()).collect();
        assert_eq!(label, "Анна Петрова  anna.petrova");
        let checked: Vec<bool> = items.iter().map(|item| item.icon.is_some()).collect();
        assert_eq!(checked, [false, false, true]);
        let unassigned = people_picker(Some(&me), &users, None, "No assignee");
        assert_eq!(unassigned.spec().items[1].icon.as_deref(), Some("check"));
    }

    #[test]
    fn a_user_is_named_by_login() {
        let user = User {
            uid: json!(1120000000016772u64),
            login: "egor".into(),
            ..User::default()
        };
        assert_eq!(user.handle(), "egor");
        let user = User {
            uid: json!(1120000000016772u64),
            ..User::default()
        };
        assert_eq!(user.handle(), "1120000000016772");
    }
}
