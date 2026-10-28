//! The organization Tracker works in: Yandex 360 (`X-Org-ID`) or Yandex Cloud
//! (`X-Cloud-Org-ID`).
//!
//! After a sign-in the plugin asks Yandex 360 for the user's organizations (`GET
//! https://api360.yandex.net/directory/v1/org`, the right `directory:read_organization`): the one
//! chosen before stays if it's there; a single one is taken; several are offered, a button each
//! (a dialog shows over Settings, a picker wouldn't); none — no right, a Yandex Cloud organization,
//! an IAM token — a dialog says where the ID is (Tracker → Administration → Organizations) and asks
//! for it. Then the connection is checked. The choice is kept in the plugin's storage.

use std::time::Duration;

use flux_plugin_api::dialog::{Ask, Form, Values};
use flux_plugin_api::notify::Notice;
use flux_plugin_api::{http, storage, system};
use flux_plugin_api::{settings as page, tr, trf};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Auth, CHANGE_ORGANIZATION, CHECK_CONNECTION, Credentials, Question};
use crate::client::ClientError;
use crate::settings::ORGANIZATION_STATUS;

/// Yandex 360's list of the user's organizations.
const ORGANIZATIONS: &str = "https://api360.yandex.net/directory/v1/org?pageSize=100";
/// Tracker's page of the organizations: their IDs.
const TRACKER_ORGANIZATIONS: &str = "https://tracker.yandex.ru/admin/orgs";
/// The plugin's storage: the chosen organization.
const ORGANIZATION: &str = "organization";
/// More organizations than this are offered in a dropdown, not as buttons.
const BUTTONS: usize = 3;
/// A button's name is cut to this many characters.
const BUTTON_CHARS: usize = 22;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[serde(rename = "360")]
    Yandex360,
    Cloud,
}

/// The organization: its ID, kind and name (empty when typed by hand).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Organization {
    pub id: String,
    pub kind: Kind,
    #[serde(default)]
    pub name: String,
}

impl Organization {
    /// From an ID and its type (`auto` — by the ID: only digits are Yandex 360's).
    pub fn of(id: &str, kind: &str) -> Option<Organization> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        let kind = match kind {
            "360" => Kind::Yandex360,
            "cloud" => Kind::Cloud,
            _ if id.chars().all(|c| c.is_ascii_digit()) => Kind::Yandex360,
            _ => Kind::Cloud,
        };
        Some(Organization {
            id: id.to_string(),
            kind,
            name: String::new(),
        })
    }

    pub fn header(&self) -> (String, String) {
        let name = match self.kind {
            Kind::Yandex360 => "X-Org-ID",
            Kind::Cloud => "X-Cloud-Org-ID",
        };
        (name.to_string(), self.id.clone())
    }

    /// "Рога и копыта · Yandex 360 · 123456".
    pub fn describe(&self) -> String {
        let kind = match self.kind {
            Kind::Yandex360 => tr("Yandex 360"),
            Kind::Cloud => tr("Yandex Cloud"),
        };
        if self.name.is_empty() {
            format!("{kind} · {}", self.id)
        } else {
            format!("{} · {kind} · {}", self.name, self.id)
        }
    }

    /// A button's label: the name, cut.
    fn label(&self) -> String {
        let name = if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        };
        cut(name, BUTTON_CHARS)
    }
}

/// The organization chosen before.
pub(super) fn stored() -> Option<Organization> {
    storage::get::<Organization>(ORGANIZATION).filter(|organization| !organization.id.is_empty())
}

/// The user's organizations in Yandex 360; none when the token may not read them (no right, a
/// Yandex Cloud account) or Yandex 360 didn't answer.
fn list(token: &str) -> Option<Vec<Organization>> {
    let response = http::get(ORGANIZATIONS)
        .header("Authorization", &format!("OAuth {token}"))
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(20))
        .fetch()
        .ok()?;
    if !response.is_success() {
        flux_plugin_api::log::info(&format!(
            "Yandex 360 didn't list the organizations: {}",
            response.status
        ));
        return None;
    }
    Some(parse(&response.text()))
}

/// `{"organizations": [{"id": 123, "name": "…"}, …]}`: the ID a number or a string.
pub(super) fn parse(text: &str) -> Vec<Organization> {
    let json: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    json["organizations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|organization| {
            let id = match &organization["id"] {
                Value::Number(number) => number.to_string(),
                Value::String(text) => text.trim().to_string(),
                _ => return None,
            };
            if id.is_empty() {
                return None;
            }
            Some(Organization {
                id,
                kind: Kind::Yandex360,
                name: organization["name"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            })
        })
        .collect()
}

/// The first `chars` characters, "…" when cut.
fn cut(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(chars - 1).collect();
    format!("{}…", kept.trim_end())
}

impl Auth {
    /// After a sign-in (`change` — Change Organization: always offered): the organization from
    /// Yandex 360's list, or asked for.
    pub(super) fn find_organization(&mut self, change: bool) {
        let Some(credentials) = self.credentials() else {
            return;
        };
        page::busy(ORGANIZATION_STATUS, &tr("Looking for your organization…"));
        let found = match &credentials {
            Credentials::OAuth(token) => list(token).unwrap_or_default(),
            // Yandex Cloud's organizations aren't in Yandex 360.
            Credentials::Iam(_) => Vec::new(),
        };
        let current = self.organization.clone();
        if !change {
            if let Some(current) = &current
                && let Some(listed) = found.iter().find(|listed| listed.id == current.id)
            {
                // The one chosen before: its name may be new.
                return self.choose(listed.clone());
            }
            if let [only] = found.as_slice() {
                return self.choose(only.clone());
            }
            if found.is_empty() && current.is_some() {
                self.show();
                let _ = self.check();
                return;
            }
        }
        self.show();
        if found.is_empty() {
            self.ask_missing_organization(matches!(credentials, Credentials::Iam(_)));
        } else {
            self.ask_organization(found);
        }
    }

    /// At launch, signed in without a known organization: one found is taken without a word;
    /// otherwise a notification offers to choose (no dialogs at launch).
    pub(super) fn find_organization_quietly(&mut self) {
        if let Some(Credentials::OAuth(token)) = self.credentials()
            && let Some(found) = list(&token)
            && let [only] = found.as_slice()
        {
            return self.choose(only.clone());
        }
        Notice::warning(&tr("Choose your organization"))
            .body(&tr(
                "Tracker works within an organization of Yandex 360 or Yandex Cloud.",
            ))
            .action_with(&tr("Choose…"), CHECK_CONNECTION, CHANGE_ORGANIZATION)
            .sticky()
            .send();
    }

    /// Change Organization…
    pub(super) fn change_organization(&mut self) {
        if self.credentials().is_none() {
            self.show();
            return;
        }
        self.find_organization(true);
    }

    /// The organization is chosen: kept, and the connection checked with it.
    fn choose(&mut self, organization: Organization) {
        if self.organization.as_ref() != Some(&organization) {
            if self.organization.as_ref().map(|old| &old.id) != Some(&organization.id) {
                self.changed = true;
            }
            storage::set(ORGANIZATION, &organization);
            self.organization = Some(organization);
        }
        self.show();
        let _ = self.check();
    }

    /// Several organizations: a button each (a dropdown when there are many).
    fn ask_organization(&mut self, found: Vec<Organization>) {
        if found.len() > BUTTONS {
            let options: Vec<(String, String)> = found
                .iter()
                .map(|organization| (organization.id.clone(), organization.describe()))
                .chain([("other".to_string(), tr("Another one: enter its ID…"))])
                .collect();
            let options: Vec<(&str, &str)> = options
                .iter()
                .map(|(value, title)| (value.as_str(), title.as_str()))
                .collect();
            let selected = self
                .organization
                .as_ref()
                .map(|current| current.id.as_str());
            let id = Form::new(&tr("Choose the Organization"), &tr("Choose"))
                .message(&tr(
                    "Your account is in several organizations. Which one should Tracker work in?",
                ))
                .choice("organization", &tr("Organization"), &options, selected)
                .send();
            self.asked
                .push((id, Question::ChooseOrganizationForm(found)));
            return;
        }
        let mut message =
            tr("Your account is in several organizations. Which one should Tracker work in?");
        for organization in &found {
            message.push_str("\n\n");
            message.push_str(&organization.describe());
        }
        let mut ask = Ask::info(&tr("Choose the Organization")).message(&message);
        for (index, organization) in found.iter().enumerate() {
            ask = if index == 0 {
                ask.primary(&organization.label())
            } else {
                ask.normal(&organization.label())
            };
        }
        let id = ask.normal(&tr("Another…")).cancel(&tr("Cancel")).send();
        self.asked.push((id, Question::ChooseOrganization(found)));
    }

    /// None found: where the ID is, and a way to it.
    fn ask_missing_organization(&mut self, cloud: bool) {
        let mut message = if cloud {
            tr(
                "Yandex Cloud doesn't tell Flux your organization. Its ID is in Tracker: Administration → Organizations.",
            )
        } else {
            tr(
                "Flux couldn't find your organization by itself. Its ID is in Tracker: Administration → Organizations.",
            )
        };
        message.push_str("\n\n");
        message.push_str(&tr(
            "Only digits — an organization of Yandex 360; letters and digits — of Yandex Cloud.",
        ));
        let id = Ask::info(&tr("Which Organization?"))
            .message(&message)
            .normal(&tr("Open Tracker"))
            .primary(&tr("Enter the ID…"))
            .cancel(&tr("Cancel"))
            .send();
        self.asked.push((id, Question::MissingOrganization));
    }

    /// The ID and the type by hand; `error` — what was wrong with the last answer.
    fn ask_organization_id(&mut self, typed: Option<(&str, &str)>, error: Option<&str>) {
        let (id, kind) = typed.unwrap_or_else(|| match &self.organization {
            Some(current) => (
                current.id.as_str(),
                match current.kind {
                    Kind::Yandex360 => "360",
                    Kind::Cloud => "cloud",
                },
            ),
            None => ("", "auto"),
        });
        let (id, kind) = (id.to_string(), kind.to_string());
        let auto = tr("By the ID: only digits — Yandex 360");
        let yandex_360 = tr("Yandex 360");
        let cloud = tr("Yandex Cloud");
        let mut form = Form::new(&tr("Organization"), &tr("Save"))
            .message(&tr(
                "Tracker → Administration → Organizations shows the ID.",
            ))
            .text("id", &tr("Organization ID"), &id)
            .code()
            .placeholder("123456")
            .choice(
                "kind",
                &tr("Organization type"),
                &[("auto", &auto), ("360", &yandex_360), ("cloud", &cloud)],
                Some(&kind),
            );
        if let Some(error) = error {
            form = form.error(error);
        }
        let question = form.send();
        self.asked.push((question, Question::OrganizationForm));
    }

    /// The ID typed: checked with Tracker before it is kept; asked again when Tracker says no.
    fn organization_given(&mut self, values: Option<Values>) {
        let Some(values) = values else {
            self.show();
            return;
        };
        let (id, kind) = (values.text("id"), values.text("kind"));
        let Some(organization) = Organization::of(&id, &kind) else {
            return self
                .ask_organization_id(Some((&id, &kind)), Some(&tr("Enter the organization's ID")));
        };
        let before = self.organization.replace(organization.clone());
        match self.check() {
            Err(
                err @ (ClientError::Forbidden(_) | ClientError::NotFound | ClientError::Other(_)),
            ) => {
                self.organization = before;
                self.show();
                let words = trf(
                    "Tracker didn't let you in with this organization: {0}",
                    &[&err.message()],
                );
                self.ask_organization_id(Some((&id, &kind)), Some(&words));
            }
            _ => {
                self.organization = before;
                self.choose(organization);
            }
        }
    }

    // --- The answers ---

    pub(super) fn take_question(&mut self, id: u64) -> Option<Question> {
        let index = self.asked.iter().position(|(asked, _)| *asked == id)?;
        Some(self.asked.remove(index).1)
    }

    pub(super) fn dialog_answered(&mut self, question: Question, button: Option<u32>) {
        match question {
            Question::OtherWays => self.other_way_chosen(button),
            Question::ChooseOrganization(found) => {
                let button = button.map(|button| button as usize);
                match button {
                    Some(index) if index < found.len() => self.choose(found[index].clone()),
                    Some(index) if index == found.len() => self.ask_organization_id(None, None),
                    _ => self.show(),
                }
            }
            Question::MissingOrganization => match button {
                Some(0) => {
                    let _ = system::open_url(TRACKER_ORGANIZATIONS);
                    self.ask_organization_id(None, None);
                }
                Some(1) => self.ask_organization_id(None, None),
                _ => self.show(),
            },
            _ => {}
        }
    }

    pub(super) fn form_answered(&mut self, question: Question, values: Option<Values>) {
        match question {
            Question::TokenForm => self.token_given(false, values),
            Question::IamForm => self.token_given(true, values),
            Question::AppForm => self.own_app_given(values),
            Question::CodeForm => self.code_given(values),
            Question::OrganizationForm => self.organization_given(values),
            Question::ChooseOrganizationForm(found) => {
                let chosen = values
                    .as_ref()
                    .and_then(|values| values.get("organization"));
                match chosen {
                    Some("other") => self.ask_organization_id(None, None),
                    Some(id) => {
                        match found.into_iter().find(|organization| organization.id == id) {
                            Some(organization) => self.choose(organization),
                            None => self.show(),
                        }
                    }
                    None => self.show(),
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn organizations_by_id_and_type() {
        let of = |id: &str, kind: &str| Organization::of(id, kind).map(|o| (o.id, o.kind));
        assert_eq!(
            of("123456", "auto"),
            Some(("123456".into(), Kind::Yandex360))
        );
        assert_eq!(of("bpf3abc", "auto"), Some(("bpf3abc".into(), Kind::Cloud)));
        assert_eq!(of("123456", "cloud"), Some(("123456".into(), Kind::Cloud)));
        assert_eq!(of(" 42 ", "360"), Some(("42".into(), Kind::Yandex360)));
        assert_eq!(of("  ", "auto"), None);
        assert_eq!(
            Organization::of("42", "360").unwrap().header(),
            ("X-Org-ID".to_string(), "42".to_string())
        );
        assert_eq!(
            Organization::of("bpf", "auto").unwrap().header(),
            ("X-Cloud-Org-ID".to_string(), "bpf".to_string())
        );
    }

    #[test]
    fn yandex_360_lists_them() {
        let found = parse(
            r#"{"organizations":[{"id":123456,"name":" Рога и копыта ","email":"a@b.ru"},
                {"id":"654321","name":"Копыта"},{"id":null,"name":"no id"},{"name":"none"}],
                "nextPageToken":""}"#,
        );
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].id.as_str(), found[0].name.as_str()),
            ("123456", "Рога и копыта")
        );
        assert_eq!(found[1].id, "654321");
        assert!(
            found
                .iter()
                .all(|organization| organization.kind == Kind::Yandex360)
        );
        assert!(parse(r#"{"organizations":[]}"#).is_empty());
        assert!(parse(r#"{"code":7,"message":"forbidden"}"#).is_empty());
        assert!(parse("<html>").is_empty());
    }

    #[test]
    fn kept_as_json() {
        let organization = Organization {
            id: "123456".into(),
            kind: Kind::Yandex360,
            name: "Рога и копыта".into(),
        };
        let json = serde_json::to_string(&organization).unwrap();
        assert!(json.contains("\"kind\":\"360\""));
        assert_eq!(
            serde_json::from_str::<Organization>(&json).unwrap(),
            organization
        );
        let typed: Organization = serde_json::from_str(r#"{"id":"bpf","kind":"cloud"}"#).unwrap();
        assert_eq!((typed.kind, typed.name.as_str()), (Kind::Cloud, ""));
    }

    #[test]
    fn long_names_are_cut_for_buttons() {
        assert_eq!(cut("Рога и копыта", 22), "Рога и копыта");
        let cut_name = cut("Общество с ограниченной ответственностью", 22);
        assert_eq!(cut_name.chars().count(), 22);
        assert!(cut_name.ends_with('…'));
    }
}
