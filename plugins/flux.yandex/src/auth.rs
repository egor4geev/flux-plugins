//! The sign-in: Yandex ID's OAuth — the code with PKCE (S256, no secret) through the browser, back
//! to the plugin's server on `http://127.0.0.1:<port>/callback` (Yandex checks the port: the first
//! free of [`PORTS`], all three registered with the app), then `POST https://oauth.yandex.ru/token`;
//! who signed in from `login.yandex.ru/info`; the token and the account in the keychain (`secrets`).
//! Instead of the sign-in — a pasted OAuth token (`oauth-token`) or an IAM token for a federated
//! account of Yandex Cloud (`iam-token`): settings of the secret kind, the same keychain. The
//! organization: `org-id`, `org-type` (by the ID: only digits — Yandex 360, `X-Org-ID`; else Yandex
//! Cloud, `X-Cloud-Org-ID`). The settings page's lines: who is signed in, the connection's check.

use std::time::Duration;

use flux_plugin_api::bindings::flux::plugin::server::ServerRequest;
use flux_plugin_api::settings::Tone;
use flux_plugin_api::{CommandContext, Event, http, log, notify, secrets, server, system, timers};
use flux_plugin_api::{settings as page, tr, trf};
use serde::{Deserialize, Serialize};

use crate::pkce::Pkce;
use crate::settings::{
    ACCOUNT_STATUS, CLIENT_ID, CONNECTION_STATUS, IAM_TOKEN, OAUTH_TOKEN, ORG_ID, ORG_TYPE, text,
};
use crate::tracker::api;

pub const SIGN_IN: &str = "sign-in";
pub const SIGN_OUT: &str = "sign-out";
pub const CHECK_CONNECTION: &str = "check-connection";

/// The ports Yandex returns to: registered with the app as `http://127.0.0.1:<port>/callback`.
pub const PORTS: [u16; 3] = [52580, 52581, 52582];
/// Flux's Yandex ID app; the `client-id` setting replaces it. Empty until the app is registered.
const BUILTIN_CLIENT_ID: &str = "";
const AUTHORIZE: &str = "https://oauth.yandex.ru/authorize";
const TOKEN: &str = "https://oauth.yandex.ru/token";
const INFO: &str = "https://login.yandex.ru/info?format=json";
/// A sign-in the browser doesn't finish is abandoned.
const SIGN_IN_WAIT: Duration = Duration::from_secs(5 * 60);

/// The keychain's keys of the signed-in account.
const ACCOUNT_TOKEN: &str = "account.token";
const ACCOUNT_INFO: &str = "account.info";

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

/// The organization's header: `X-Org-ID` (Yandex 360) or `X-Cloud-Org-ID` (Yandex Cloud).
#[derive(Clone, Debug, PartialEq)]
pub enum Organization {
    Yandex360(String),
    Cloud(String),
}

impl Organization {
    /// From the settings: the ID and its type (`auto` — by the ID: only digits are Yandex 360's).
    pub fn of(id: &str, kind: &str) -> Option<Organization> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        let yandex_360 = match kind {
            "360" => true,
            "cloud" => false,
            _ => id.chars().all(|c| c.is_ascii_digit()),
        };
        Some(if yandex_360 {
            Organization::Yandex360(id.to_string())
        } else {
            Organization::Cloud(id.to_string())
        })
    }

    pub fn header(&self) -> (String, String) {
        match self {
            Organization::Yandex360(id) => ("X-Org-ID".into(), id.clone()),
            Organization::Cloud(id) => ("X-Cloud-Org-ID".into(), id.clone()),
        }
    }

    /// "Yandex 360 · 123456".
    pub fn describe(&self) -> String {
        match self {
            Organization::Yandex360(id) => format!("{} · {id}", tr("Yandex 360")),
            Organization::Cloud(id) => format!("{} · {id}", tr("Yandex Cloud")),
        }
    }
}

/// Who signed in, as `login.yandex.ru/info` says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub login: String,
    pub display: String,
    pub email: String,
}

/// A sign-in waiting for the browser.
struct Pending {
    pkce: Pkce,
    client_id: String,
    redirect: String,
    timer: u64,
}

pub struct Auth {
    /// The signed-in account's token and who it is.
    token: Option<String>,
    account: Option<Account>,
    /// The tokens of the settings.
    pasted: Option<String>,
    iam: Option<String>,
    organization: Option<Organization>,
    pending: Option<Pending>,
    /// Stops the server a moment after the browser got its page.
    stop_timer: Option<u64>,
    changed: bool,
}

impl Auth {
    pub fn new() -> Self {
        Auth {
            token: None,
            account: None,
            pasted: None,
            iam: None,
            organization: None,
            pending: None,
            stop_timer: None,
            changed: false,
        }
    }

    pub fn activate(&mut self) {
        self.token = secrets::get(ACCOUNT_TOKEN).filter(|token| !token.is_empty());
        self.account = self.token.as_ref().and_then(|_| {
            secrets::get(ACCOUNT_INFO).and_then(|info| serde_json::from_str(&info).ok())
        });
        self.read_settings();
        self.show_account();
        self.changed = true;
    }

    pub fn deactivate(&mut self) {
        if self.pending.take().is_some() {
            server::stop();
        }
    }

    pub fn settings_changed(&mut self) {
        let before = (self.credentials(), self.organization.clone());
        self.read_settings();
        if (self.credentials(), self.organization.clone()) != before {
            self.changed = true;
            page::clear_status(CONNECTION_STATUS);
        }
        self.show_account();
    }

    fn read_settings(&mut self) {
        let secret = |key: &str| {
            secrets::get(key)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        self.pasted = secret(OAUTH_TOKEN);
        self.iam = secret(IAM_TOKEN);
        self.organization = Organization::of(
            &text(ORG_ID).unwrap_or_default(),
            &text(ORG_TYPE).unwrap_or_default(),
        );
    }

    pub fn run_command(&mut self, command: &str, context: &CommandContext) {
        let _ = context;
        match command {
            SIGN_IN => self.sign_in(),
            SIGN_OUT => self.sign_out(),
            CHECK_CONNECTION => self.check_connection(),
            _ => {}
        }
    }

    /// The event is about the sign-in: the plugin's server (the callback), the sign-in's timers.
    pub fn claims(&self, event: &Event) -> bool {
        match event {
            Event::ServerRequest(_) => true,
            Event::Timer(id) => {
                self.pending.as_ref().is_some_and(|pending| pending.timer == *id)
                    || self.stop_timer == Some(*id)
            }
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
            Event::Timer(_) => {
                // The browser didn't come back.
                self.pending = None;
                server::stop();
                page::status(ACCOUNT_STATUS, &tr("The sign-in timed out"), Tone::Warning);
            }
            _ => {}
        }
    }

    /// The credentials of requests: an IAM token, a pasted OAuth token, the sign-in's token.
    pub fn credentials(&self) -> Option<Credentials> {
        if let Some(iam) = &self.iam {
            return Some(Credentials::Iam(iam.clone()));
        }
        self.pasted
            .clone()
            .or_else(|| self.token.clone())
            .map(Credentials::OAuth)
    }

    /// The organization of the settings.
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

    /// The signed-in account's email (`login@domain` for CalDAV), when known.
    pub fn email(&self) -> Option<String> {
        self.account
            .as_ref()
            .map(|account| account.email.clone())
            .filter(|email| !email.is_empty())
    }

    /// The sign-in, the token or the organization changed since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// The account's line on the settings page.
    fn show_account(&self) {
        if self.pending.is_some() {
            return;
        }
        if self.iam.is_some() {
            page::status(ACCOUNT_STATUS, &tr("An IAM token is used"), Tone::Normal);
        } else if self.pasted.is_some() {
            page::status(ACCOUNT_STATUS, &tr("A pasted OAuth token is used"), Tone::Normal);
        } else if let (Some(_), Some(account)) = (&self.token, &self.account) {
            let who = if account.display.is_empty() || account.display == account.login {
                account.login.clone()
            } else {
                format!("{} ({})", account.display, account.login)
            };
            page::status(ACCOUNT_STATUS, &trf("Signed in as {0}", &[&who]), Tone::Success);
        } else {
            page::clear_status(ACCOUNT_STATUS);
        }
    }

    fn sign_in(&mut self) {
        let client_id = text(CLIENT_ID).unwrap_or_else(|| BUILTIN_CLIENT_ID.to_string());
        if client_id.is_empty() {
            let message = tr("Set the ClientID of a Yandex ID app in the plugin's settings (Advanced)");
            page::status(ACCOUNT_STATUS, &message, Tone::Warning);
            notify::warning(&tr("Can't sign in to Yandex"), Some(&message));
            return;
        }
        let Some(pkce) = Pkce::new() else {
            page::status(ACCOUNT_STATUS, &tr("No randomness for the sign-in"), Tone::Error);
            return;
        };
        if let Some(pending) = self.pending.take() {
            timers::cancel(pending.timer);
        }
        let Some(port) = PORTS.iter().find_map(|port| server::listen_on(*port).ok()) else {
            let message = trf("The ports {0} are busy: close what uses them and try again", &[
                &"52580–52582",
            ]);
            page::status(ACCOUNT_STATUS, &message, Tone::Error);
            return;
        };
        let redirect = server::url(port, "/callback");
        let url = http::with_query(
            AUTHORIZE,
            &[
                ("response_type", "code"),
                ("client_id", &client_id),
                ("redirect_uri", &redirect),
                ("code_challenge", &pkce.challenge),
                ("code_challenge_method", "S256"),
                ("state", &pkce.state),
            ],
        );
        if let Err(err) = system::open_url(&url) {
            server::stop();
            page::status(ACCOUNT_STATUS, &err, Tone::Error);
            return;
        }
        let timer = timers::after(SIGN_IN_WAIT);
        self.pending = Some(Pending {
            pkce,
            client_id,
            redirect,
            timer,
        });
        page::busy(ACCOUNT_STATUS, &tr("Finish the sign-in in the browser…"));
    }

    /// The browser came back to `/callback`.
    fn callback(&mut self, request: ServerRequest) {
        if request.route() != "/callback" {
            server::not_found(request.id);
            return;
        }
        let Some(pending) = self.pending.take() else {
            respond(request.id, false, &tr("This sign-in is over: start it again in Flux."));
            return;
        };
        timers::cancel(pending.timer);
        let state_fits = request.query("state").as_deref() == Some(pending.pkce.state.as_str());
        let finish = |auth: &mut Auth, page_ok: bool, page_text: &str| {
            respond(request.id, page_ok, page_text);
            // The browser gets its page first; the server stops a moment later.
            auth.stop_timer = Some(timers::after(Duration::from_secs(1)));
        };
        if let Some(error) = request.query("error") {
            let words = request.query("error_description").unwrap_or(error);
            finish(self, false, &trf("Yandex didn't sign you in: {0}", &[&words]));
            page::status(ACCOUNT_STATUS, &tr("The sign-in was cancelled"), Tone::Warning);
            return;
        }
        let Some(code) = request.query("code").filter(|_| state_fits) else {
            finish(self, false, &tr("The answer doesn't belong to this sign-in."));
            page::status(ACCOUNT_STATUS, &tr("The sign-in failed"), Tone::Error);
            return;
        };
        match exchange(&pending, &code) {
            Ok((token, account)) => {
                let saved = secrets::set(ACCOUNT_TOKEN, &token).and_then(|()| {
                    secrets::set(ACCOUNT_INFO, &serde_json::to_string(&account).unwrap_or_default())
                });
                if let Err(err) = saved {
                    log::warn(&format!("Couldn't keep the token in the keychain: {err}"));
                }
                let who = if account.display.is_empty() {
                    account.login.clone()
                } else {
                    account.display.clone()
                };
                finish(self, true, &trf("You're signed in to Flux as {0}. You can close this tab.", &[&who]));
                self.token = Some(token);
                self.account = Some(account);
                self.changed = true;
                self.show_account();
                notify::success(&trf("Signed in to Yandex as {0}", &[&who]));
            }
            Err(err) => {
                finish(self, false, &err);
                page::status(ACCOUNT_STATUS, &err, Tone::Error);
            }
        }
    }

    fn sign_out(&mut self) {
        if let Some(pending) = self.pending.take() {
            timers::cancel(pending.timer);
            server::stop();
        }
        let _ = secrets::remove(ACCOUNT_TOKEN);
        let _ = secrets::remove(ACCOUNT_INFO);
        let was = self.token.take().is_some();
        self.account = None;
        self.show_account();
        page::clear_status(CONNECTION_STATUS);
        if was {
            self.changed = true;
        }
    }

    /// Check Connection: who the token is in Tracker, and the organization.
    fn check_connection(&mut self) {
        if self.credentials().is_none() {
            page::status(CONNECTION_STATUS, &tr("Sign in first"), Tone::Warning);
            return;
        }
        let Some(organization) = self.organization.clone() else {
            page::status(CONNECTION_STATUS, &tr("Set the organization's ID"), Tone::Warning);
            return;
        };
        page::busy(CONNECTION_STATUS, &tr("Checking…"));
        match api::myself(self) {
            Ok(me) => {
                let who = if me.display.is_empty() { me.login } else { me.display };
                page::status(
                    CONNECTION_STATUS,
                    &trf("Connected as {0} · {1}", &[&who, &organization.describe()]),
                    Tone::Success,
                );
            }
            Err(err) => page::status(CONNECTION_STATUS, &err.message(), Tone::Error),
        }
    }
}

/// The code for the token (`/token` with the verifier), then who it is (`/info`).
fn exchange(pending: &Pending, code: &str) -> Result<(String, Account), String> {
    let response = http::post(TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", &pending.client_id),
            ("code_verifier", &pending.pkce.verifier),
            ("redirect_uri", &pending.redirect),
        ])
        .timeout(Duration::from_secs(20))
        .fetch()
        .map_err(|err| err.message())?;
    #[derive(Deserialize)]
    struct Token {
        access_token: Option<String>,
        error: Option<String>,
        error_description: Option<String>,
    }
    let token: Token = response
        .json()
        .map_err(|_| trf("Yandex answered {0}", &[&response.status.to_string()]))?;
    let Some(access) = token.access_token.filter(|token| !token.is_empty()) else {
        let words = token
            .error_description
            .or(token.error)
            .unwrap_or_else(|| response.status.to_string());
        return Err(trf("Yandex didn't give a token: {0}", &[&words]));
    };
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Info {
        login: String,
        display_name: String,
        real_name: String,
        default_email: String,
    }
    let info: Info = http::get(INFO)
        .header("Authorization", &format!("OAuth {access}"))
        .timeout(Duration::from_secs(20))
        .fetch()
        .ok()
        .filter(|response| response.is_success())
        .and_then(|response| response.json().ok())
        .unwrap_or_default();
    let display = if info.real_name.is_empty() {
        info.display_name
    } else {
        info.real_name
    };
    let email = if info.default_email.is_empty() && !info.login.is_empty() {
        format!("{}@yandex.ru", info.login)
    } else {
        info.default_email
    };
    Ok((
        access,
        Account {
            login: info.login,
            display,
            email,
        },
    ))
}

/// The page the browser shows after the sign-in.
fn respond(request: u64, ok: bool, message: &str) {
    let title = if ok {
        tr("Signed in")
    } else {
        tr("The sign-in didn't work")
    };
    let color = if ok { "#2f9e5b" } else { "#c2453b" };
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Flux</title></head>\
         <body style=\"font:15px -apple-system,BlinkMacSystemFont,sans-serif;display:flex;\
         align-items:center;justify-content:center;height:90vh;margin:0;color:#333\">\
         <div style=\"text-align:center;max-width:460px\"><h2 style=\"color:{color}\">{}</h2>\
         <p>{}</p></div></body></html>",
        escape(&title),
        escape(message)
    );
    server::respond_html(request, if ok { 200 } else { 400 }, &html);
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn organizations_by_id_and_type() {
        assert_eq!(
            Organization::of("123456", "auto"),
            Some(Organization::Yandex360("123456".into()))
        );
        assert_eq!(
            Organization::of("bpf3abc", "auto"),
            Some(Organization::Cloud("bpf3abc".into()))
        );
        assert_eq!(
            Organization::of("123456", "cloud"),
            Some(Organization::Cloud("123456".into()))
        );
        assert_eq!(Organization::of("  ", "auto"), None);
        assert_eq!(
            Organization::of("42", "360").unwrap().header(),
            ("X-Org-ID".to_string(), "42".to_string())
        );
    }

    #[test]
    fn credentials_headers() {
        assert_eq!(Credentials::OAuth("y0_x".into()).header(), "OAuth y0_x");
        assert_eq!(Credentials::Iam("t1.x".into()).header(), "Bearer t1.x");
    }

    #[test]
    fn escapes_the_page() {
        assert_eq!(escape("<b>&\"</b>"), "&lt;b&gt;&amp;&quot;&lt;/b&gt;");
    }
}
