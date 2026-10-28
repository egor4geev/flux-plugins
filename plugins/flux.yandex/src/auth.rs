//! The account: who is signed in to Yandex, how, and in which organization.
//!
//! - One way at a time: Flux's own Yandex ID app in the browser (the main one), or — under Other
//!   Ways — a pasted OAuth token, an IAM token of Yandex Cloud, the organization's own app (in the
//!   browser, or with the code Yandex shows). A new sign-in replaces the previous one's tokens
//!   ([`signin`]).
//! - After any sign-in the plugin finds the organization by itself — Yandex 360's list of the
//!   user's organizations: one is chosen, several are offered, none asks for the ID with directions
//!   ([`orgs`]) — and checks the connection (Tracker's `/myself`).
//! - The token's lifetime: `expires_in` and `refresh_token` are kept; near the end the plugin
//!   refreshes the token, and when it can't, or Yandex rejects the token, a sticky notification
//!   offers to sign in again.
//! - The settings page: a line about the account and one about the organization, with their
//!   buttons; the buttons' `argument` says which flow (Other Ways, Cancel, Change Organization).
//!
//! Flux's secrets keep the tokens (`account.token`, `account.refresh`; the old settings' secrets
//! `oauth-token`, `iam-token` — the same keys); the plugin's storage keeps how the account signed
//! in, when its token expires and who it is (`session`), and the organization (`organization`).

mod orgs;
mod signin;

use std::cell::Cell;
use std::time::Duration;

use flux_plugin_api::dialog::Values;
use flux_plugin_api::host::commands;
use flux_plugin_api::notify::Notice;
use flux_plugin_api::settings::Tone;
use flux_plugin_api::{
    CommandContext, CommandSource, Event, log, secrets, server, storage, system, timers,
};
use flux_plugin_api::{settings as page, tr, trf};
use serde::{Deserialize, Serialize};

pub use orgs::Organization;

use crate::client::ClientError;
use crate::settings::{ACCOUNT_STATUS, ORGANIZATION_STATUS};
use crate::tracker::api;

pub const SIGN_IN: &str = "sign-in";
pub const SIGN_OUT: &str = "sign-out";
pub const CHECK_CONNECTION: &str = "check-connection";

/// What a settings button or a notification's action carries to `sign-in`.
const OTHER_WAYS: &str = "other";
const CANCEL: &str = "cancel";
/// Sign in again the way the account signed in last time.
const AGAIN: &str = "again";
/// … and to `check-connection`: Change Organization.
const CHANGE_ORGANIZATION: &str = "organization";

/// The secrets' keys: the sign-in's tokens, the old settings' secrets.
const ACCOUNT_TOKEN: &str = "account.token";
const ACCOUNT_REFRESH: &str = "account.refresh";
/// Who signed in, as versions before the session kept it.
const ACCOUNT_INFO: &str = "account.info";
const OAUTH_TOKEN: &str = "oauth-token";
const IAM_TOKEN: &str = "iam-token";
/// The plugin's storage: [`Session`].
const SESSION: &str = "session";

/// How often the token's lifetime is looked at while Flux runs.
const LIFETIME_CHECK: Duration = Duration::from_secs(6 * 60 * 60);
/// A token is refreshed this long before it expires (Yandex advises every few months)…
const REFRESH_BEFORE: i64 = 30 * 24 * 60 * 60;
/// …and the user is told to sign in again this long before, when it can't be.
const WARN_BEFORE: i64 = 7 * 24 * 60 * 60;
/// An IAM token lives 12 hours.
const IAM_LIFETIME: i64 = 12 * 60 * 60;

/// How requests to Yandex authorize: `Authorization: OAuth <token>` or `Bearer <IAM token>`.
#[derive(Clone, Debug, PartialEq)]
pub enum Credentials {
    OAuth(String),
    Iam(String),
}

impl Credentials {
    /// The `Authorization` header's value.
    pub fn header(&self) -> String {
        match self {
            Credentials::OAuth(token) => format!("OAuth {token}"),
            Credentials::Iam(token) => format!("Bearer {token}"),
        }
    }
}

/// Who signed in, as `login.yandex.ru/info` (or Tracker's `/myself`) says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub login: String,
    pub display: String,
    pub email: String,
}

impl Account {
    /// "Егор Агеев (egor.ageev)", or the login alone.
    fn who(&self) -> String {
        if self.display.is_empty() || self.display == self.login {
            self.login.clone()
        } else if self.login.is_empty() {
            self.display.clone()
        } else {
            format!("{} ({})", self.display, self.login)
        }
    }

    fn is_known(&self) -> bool {
        !self.login.is_empty() || !self.display.is_empty()
    }
}

/// How the account signed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Way {
    /// Flux's app, in the browser.
    #[default]
    Flux,
    /// The organization's own app "for user authorization", in the browser.
    App,
    /// The organization's own app "for API access or debugging": the code from Yandex's page.
    Code,
    /// A pasted OAuth token.
    Token,
    /// An IAM token of Yandex Cloud.
    Iam,
}

/// What the plugin knows of the sign-in besides its tokens (the plugin's storage).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Session {
    way: Way,
    /// The app the token came from: refreshing needs it. Empty — unknown.
    client_id: String,
    /// When the token stops working, in seconds since 1970; 0 — unknown.
    expires_at: i64,
    /// Who it is; none until Yandex or Tracker said.
    account: Option<Account>,
}

/// What an open dialog asks.
enum Question {
    /// Other Ways to Sign In: the buttons of the ways.
    OtherWays,
    /// A pasted OAuth token.
    TokenForm,
    /// An IAM token.
    IamForm,
    /// The organization's own app: its ClientID and kind.
    AppForm,
    /// The code Yandex's page shows (a debug app).
    CodeForm,
    /// Several organizations found: a button each.
    ChooseOrganization(Vec<Organization>),
    /// Many found: a dropdown.
    ChooseOrganizationForm(Vec<Organization>),
    /// None found: where the ID is.
    MissingOrganization,
    /// The ID and the type, by hand.
    OrganizationForm,
}

pub struct Auth {
    /// The token of a sign-in in the browser or with a code.
    token: Option<String>,
    /// A pasted OAuth token, an IAM token.
    pasted: Option<String>,
    iam: Option<String>,
    session: Session,
    organization: Option<Organization>,
    /// The last check of the connection since the sign-in or the launch: none — not checked; an
    /// error — what Tracker said.
    checked: Option<Result<(), ClientError>>,
    /// A sign-in waiting for the browser.
    pending: Option<signin::Pending>,
    /// A sign-in with a code waiting for the user to paste it.
    code: Option<signin::CodeSignIn>,
    /// The open dialogs and forms.
    asked: Vec<(u64, Question)>,
    /// Stops the server a moment after the browser got its page.
    stop_timer: Option<u64>,
    lifetime_timer: Option<u64>,
    /// A refresh failed this launch: not tried again until the next.
    refresh_failed: bool,
    /// The user was told this launch that the token expires, or that Yandex rejected it.
    expiry_told: bool,
    rejected_told: Cell<bool>,
    changed: bool,
}

impl Auth {
    pub fn new() -> Self {
        Auth {
            token: None,
            pasted: None,
            iam: None,
            session: Session::default(),
            organization: None,
            checked: None,
            pending: None,
            code: None,
            asked: Vec::new(),
            stop_timer: None,
            lifetime_timer: None,
            refresh_failed: false,
            expiry_told: false,
            rejected_told: Cell::new(false),
            changed: false,
        }
    }

    pub fn activate(&mut self) {
        self.token = secret(ACCOUNT_TOKEN);
        self.pasted = secret(OAUTH_TOKEN);
        self.iam = secret(IAM_TOKEN);
        self.session = storage::get(SESSION).unwrap_or_default();
        if self.session.account.is_none() && self.token.is_some() {
            // Signed in by a version before the session: who it is was in the secrets.
            self.session.account = secrets::get(ACCOUNT_INFO)
                .and_then(|info| serde_json::from_str::<Account>(&info).ok())
                .filter(Account::is_known);
        }
        self.organization = orgs::stored();
        self.lifetime_timer = Some(timers::every(LIFETIME_CHECK));
        self.changed = true;
        self.show();
        self.look_at_lifetime();
        if self.credentials().is_some() && self.organization.is_none() {
            // Signed in, but the organization isn't known (the settings of versions before kept
            // it): one found is taken at once; otherwise a notification offers to choose.
            self.find_organization_quietly();
        }
    }

    pub fn deactivate(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.close();
            server::stop();
        }
        if let Some(timer) = self.lifetime_timer.take() {
            timers::cancel(timer);
        }
    }

    /// The page's values changed: nothing of the account is among them any more.
    pub fn settings_changed(&mut self) {
        self.show();
    }

    pub fn run_command(&mut self, command: &str, context: &CommandContext) {
        let from_settings = context.source == CommandSource::Settings;
        match (command, context.argument.as_deref()) {
            (SIGN_IN, Some(OTHER_WAYS)) => self.ask_other_ways(from_settings),
            (SIGN_IN, Some(CANCEL)) => self.cancel_sign_in(),
            (SIGN_IN, Some(AGAIN)) => self.sign_in_again(from_settings),
            (SIGN_IN, _) => self.sign_in_with_flux(),
            (SIGN_OUT, _) => self.sign_out(),
            (CHECK_CONNECTION, Some(CHANGE_ORGANIZATION)) => self.change_organization(),
            (CHECK_CONNECTION, _) => self.check_connection(),
            _ => {}
        }
    }

    /// The event is the account's: the plugin's server (the browser's return), its timers, its
    /// dialogs.
    pub fn claims(&self, event: &Event) -> bool {
        match event {
            Event::ServerRequest(_) => true,
            Event::Timer(id) => {
                self.pending
                    .as_ref()
                    .is_some_and(|pending| pending.owns(*id))
                    || self.stop_timer == Some(*id)
                    || self.lifetime_timer == Some(*id)
            }
            Event::DialogAnswered((id, _))
            | Event::TextAnswered((id, _))
            | Event::FormAnswered((id, _)) => self.asked.iter().any(|(asked, _)| asked == id),
            _ => false,
        }
    }

    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::ServerRequest(request) => self.callback(request),
            Event::Timer(id) if self.stop_timer == Some(id) => {
                self.stop_timer = None;
                if self.pending.is_none() {
                    server::stop();
                }
            }
            Event::Timer(id) if self.lifetime_timer == Some(id) => self.look_at_lifetime(),
            Event::Timer(id) => self.pending_timer(id),
            Event::DialogAnswered((id, button)) => {
                if let Some(question) = self.take_question(id) {
                    self.dialog_answered(question, button);
                }
            }
            Event::FormAnswered((id, values)) => {
                if let Some(question) = self.take_question(id) {
                    self.form_answered(question, values.map(Values::from));
                }
            }
            _ => {}
        }
    }

    /// The credentials of requests: an IAM token, a pasted OAuth token, the sign-in's token (a new
    /// sign-in removes the others; the order is for those kept by versions before).
    pub fn credentials(&self) -> Option<Credentials> {
        if let Some(iam) = &self.iam {
            return Some(Credentials::Iam(iam.clone()));
        }
        self.pasted
            .clone()
            .or_else(|| self.token.clone())
            .map(Credentials::OAuth)
    }

    /// The chosen organization.
    pub fn organization(&self) -> Option<Organization> {
        self.organization.clone()
    }

    /// The headers of a request to Tracker: authorization and the organization; none without
    /// credentials.
    pub fn headers(&self) -> Option<Vec<(String, String)>> {
        let credentials = self.credentials()?;
        let mut headers = vec![("Authorization".to_string(), credentials.header())];
        if let Some(organization) = &self.organization {
            headers.push(organization.header());
        }
        Some(headers)
    }

    /// The sign-in, the token or the organization changed since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Yandex answered 401 to a request with the credentials: once a launch, a sticky
    /// notification offers to sign in again (the client calls it).
    pub fn token_rejected(&self) {
        if self.credentials().is_none() || self.rejected_told.replace(true) {
            return;
        }
        Notice::warning(&tr("Yandex didn't accept the sign-in"))
            .body(&tr("Sign in again to keep Tracker working."))
            .action_with(&tr("Sign In…"), SIGN_IN, AGAIN)
            .sticky()
            .send();
    }

    // --- The page ---

    /// The account's and the organization's lines, and which buttons work.
    fn show(&self) {
        let signed_in = self.credentials().is_some();
        commands::set_enabled(SIGN_OUT, signed_in);
        commands::set_enabled(CHECK_CONNECTION, signed_in);
        if self.pending.is_none() {
            match self.account_line() {
                Some((text, tone)) => page::status(ACCOUNT_STATUS, &text, tone),
                None => page::clear_status(ACCOUNT_STATUS),
            }
        }
        match (&self.organization, signed_in) {
            (Some(organization), _) => page::status(
                ORGANIZATION_STATUS,
                &trf("Organization: {0}", &[&organization.describe()]),
                Tone::Normal,
            ),
            (None, true) => page::status(
                ORGANIZATION_STATUS,
                &tr("Choose the organization: Change Organization…"),
                Tone::Warning,
            ),
            (None, false) => page::clear_status(ORGANIZATION_STATUS),
        }
    }

    /// The account's line; none — the manifest's "Not signed in".
    fn account_line(&self) -> Option<(String, Tone)> {
        let credentials = self.credentials()?;
        let who = self
            .session
            .account
            .as_ref()
            .filter(|account| account.is_known())
            .map(Account::who);
        let signed_in = match (&who, &credentials) {
            (Some(who), _) => trf("You're signed in as {0}", &[who]),
            (None, Credentials::Iam(_)) => tr("You're signed in with an IAM token"),
            (None, Credentials::OAuth(_)) => tr("You're signed in with an OAuth token"),
        };
        let now = system::now();
        let expired = self.session.expires_at > 0 && now >= self.session.expires_at;
        Some(match &self.checked {
            _ if expired && self.session.way == Way::Iam => {
                (tr("The IAM token has expired: sign in again"), Tone::Error)
            }
            _ if expired => (tr("The sign-in has expired: sign in again"), Tone::Error),
            Some(Err(ClientError::Unauthorized)) => (
                tr("Yandex didn't accept the sign-in: sign in again"),
                Tone::Error,
            ),
            Some(Err(err)) => (
                trf(
                    "{0} · Tracker didn't let you in: {1}",
                    &[&signed_in, &err.message()],
                ),
                Tone::Error,
            ),
            _ if self.session.way == Way::Iam && self.session.expires_at > 0 => {
                let until = system::local_time(self.session.expires_at).hm();
                (trf("{0} · until {1}", &[&signed_in, &until]), Tone::Success)
            }
            _ => (signed_in, Tone::Success),
        })
    }

    // --- Signing in and out ---

    /// A sign-in of any way is over: its tokens replace every other's, then who it is, the
    /// organization and the check.
    fn signed_in(
        &mut self,
        way: Way,
        tokens: signin::Tokens,
        client_id: &str,
        account: Option<Account>,
    ) {
        let now = system::now();
        let keep = |key: &str, value: Option<&str>| {
            let result = match value {
                Some(value) => secrets::set(key, value),
                None => secrets::remove(key),
            };
            if let Err(err) = result {
                log::warn(&format!("Couldn't keep \"{key}\" in Flux's secrets: {err}"));
            }
        };
        let (token, pasted, iam) = match way {
            Way::Flux | Way::App | Way::Code => (Some(tokens.access.clone()), None, None),
            Way::Token => (None, Some(tokens.access.clone()), None),
            Way::Iam => (None, None, Some(tokens.access.clone())),
        };
        keep(ACCOUNT_TOKEN, token.as_deref());
        keep(
            ACCOUNT_REFRESH,
            tokens.refresh.as_deref().filter(|_| token.is_some()),
        );
        keep(OAUTH_TOKEN, pasted.as_deref());
        keep(IAM_TOKEN, iam.as_deref());
        if secrets::get(ACCOUNT_INFO).is_some() {
            keep(ACCOUNT_INFO, None);
        }
        let expires_at = match (way, tokens.expires_in) {
            (Way::Iam, _) => now + IAM_LIFETIME,
            (_, Some(seconds)) if seconds > 0 => now + seconds,
            _ => 0,
        };
        self.token = token;
        self.pasted = pasted;
        self.iam = iam;
        self.session = Session {
            way,
            client_id: client_id.to_string(),
            expires_at,
            account,
        };
        storage::set(SESSION, &self.session);
        self.checked = None;
        self.refresh_failed = false;
        self.expiry_told = false;
        self.rejected_told.set(false);
        self.changed = true;
        self.show();
        if let Some(who) = self.session.account.as_ref().map(Account::who) {
            flux_plugin_api::notify::success(&trf("Signed in to Yandex as {0}", &[&who]));
        }
        self.find_organization(false);
    }

    fn sign_out(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.close();
            server::stop();
        }
        for key in [
            ACCOUNT_TOKEN,
            ACCOUNT_REFRESH,
            ACCOUNT_INFO,
            OAUTH_TOKEN,
            IAM_TOKEN,
        ] {
            if secrets::get(key).is_some() {
                let _ = secrets::remove(key);
            }
        }
        storage::remove(SESSION);
        let was = self.credentials().is_some();
        self.token = None;
        self.pasted = None;
        self.iam = None;
        self.session = Session::default();
        self.checked = None;
        // The organization stays: a sign-in again finds it in the list, or asks.
        self.show();
        if was {
            self.changed = true;
        }
    }

    /// The notification's Sign In: the way the account signed in last time.
    fn sign_in_again(&mut self, from_settings: bool) {
        match self.session.way {
            Way::Flux => self.sign_in_with_flux(),
            Way::App => match signin::own_app() {
                Some(app) => self.start_browser(&app.client_id, Way::App),
                None => self.ask_other_ways(from_settings),
            },
            Way::Code => match signin::own_app() {
                Some(app) => self.start_code(&app.client_id),
                None => self.ask_other_ways(from_settings),
            },
            Way::Token => self.ask_token(false),
            Way::Iam => self.ask_token(true),
        }
    }

    // --- The token's lifetime ---

    /// Near its end the token is refreshed; when it can't be, the user is told to sign in again.
    fn look_at_lifetime(&mut self) {
        let expires_at = self.session.expires_at;
        if expires_at == 0 || self.credentials().is_none() {
            return;
        }
        let left = expires_at - system::now();
        if self.session.way == Way::Iam {
            if left <= 0 && !self.expiry_told {
                self.expiry_told = true;
                Notice::warning(&tr("The IAM token has expired"))
                    .body(&tr(
                        "Get a new one with yc iam create-token and sign in again.",
                    ))
                    .action_with(&tr("Sign In…"), SIGN_IN, AGAIN)
                    .sticky()
                    .send();
                self.show();
            }
            return;
        }
        if left < REFRESH_BEFORE && !self.refresh_failed {
            let refresh = secret(ACCOUNT_REFRESH);
            match refresh.map(|refresh| signin::refresh(&refresh, &self.session.client_id)) {
                Some(Ok(tokens)) => {
                    log::info("The Yandex token was refreshed");
                    self.refreshed(tokens);
                    return;
                }
                Some(Err(err)) => log::info(&format!("The Yandex token wasn't refreshed: {err}")),
                None => {}
            }
            self.refresh_failed = true;
        }
        if left < WARN_BEFORE && !self.expiry_told {
            self.expiry_told = true;
            let title = if left <= 0 {
                tr("The Yandex sign-in has expired")
            } else {
                let days = (left / (24 * 60 * 60)).max(1);
                trf("The Yandex sign-in expires in {0} d", &[&days])
            };
            Notice::warning(&title)
                .body(&tr("Sign in again to keep Tracker working."))
                .action_with(&tr("Sign In…"), SIGN_IN, AGAIN)
                .sticky()
                .send();
            self.show();
        }
    }

    fn refreshed(&mut self, tokens: signin::Tokens) {
        if let Err(err) = secrets::set(ACCOUNT_TOKEN, &tokens.access) {
            log::warn(&format!("Couldn't keep the token in Flux's secrets: {err}"));
        }
        if let Some(refresh) = &tokens.refresh {
            let _ = secrets::set(ACCOUNT_REFRESH, refresh);
        }
        if let Some(seconds) = tokens.expires_in.filter(|seconds| *seconds > 0) {
            self.session.expires_at = system::now() + seconds;
        }
        storage::set(SESSION, &self.session);
        if self.token.as_deref() != Some(tokens.access.as_str()) {
            self.token = Some(tokens.access);
            self.changed = true;
        }
    }

    // --- The connection ---

    /// Check Connection: who the token is in Tracker, with the organization.
    fn check_connection(&mut self) {
        if self.credentials().is_none() {
            self.show();
            return;
        }
        if self.organization.is_none() {
            self.find_organization(false);
            return;
        }
        let _ = self.check();
    }

    /// Tracker's `/myself` with the credentials and the organization; the account's line says how
    /// it went. Who it is comes from there when Yandex ID didn't say (a pasted token, IAM).
    fn check(&mut self) -> Result<(), ClientError> {
        page::busy(ACCOUNT_STATUS, &tr("Checking the connection…"));
        let result = api::myself(self).map(|me| {
            let unknown = !self.session.account.as_ref().is_some_and(Account::is_known);
            if unknown && (!me.login.is_empty() || !me.display.is_empty()) {
                self.session.account = Some(Account {
                    login: me.login,
                    display: me.display,
                    email: me.email,
                });
                storage::set(SESSION, &self.session);
            }
        });
        if result == Err(ClientError::Unauthorized) {
            self.token_rejected();
        }
        self.checked = Some(result.clone());
        self.show();
        result
    }
}

/// A secret, trimmed; none when empty.
fn secret(key: &str) -> Option<String> {
    secrets::get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_headers() {
        assert_eq!(Credentials::OAuth("y0_x".into()).header(), "OAuth y0_x");
        assert_eq!(Credentials::Iam("t1.x".into()).header(), "Bearer t1.x");
    }

    #[test]
    fn who_the_account_is() {
        let account = |login: &str, display: &str| Account {
            login: login.into(),
            display: display.into(),
            email: String::new(),
        };
        assert_eq!(account("egor", "Егор Агеев").who(), "Егор Агеев (egor)");
        assert_eq!(account("egor", "egor").who(), "egor");
        assert_eq!(account("egor", "").who(), "egor");
        assert_eq!(account("", "Егор").who(), "Егор");
        assert!(!account("", "").is_known());
    }

    #[test]
    fn sessions_of_versions_before_read() {
        let session: Session = serde_json::from_str("{}").unwrap();
        assert_eq!(session, Session::default());
        let session: Session =
            serde_json::from_str(r#"{"way":"iam","expires_at":5,"account":null}"#).unwrap();
        assert_eq!(session.way, Way::Iam);
        assert_eq!(session.expires_at, 5);
    }
}
