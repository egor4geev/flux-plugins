//! The ways to sign in.
//!
//! - **Sign In with Yandex**: Flux's own Yandex ID app ([`BUILTIN_CLIENT_ID`], a public client "for
//!   user authorization") — the code with PKCE (S256, no secret) through the browser, back to the
//!   plugin's server on `http://127.0.0.1:<port>/callback` (Yandex checks the port: the first free
//!   of [`PORTS`], all three registered with the app), then `POST https://oauth.yandex.ru/token`
//!   with the verifier. The rights are asked for explicitly: Tracker's and who the user is in
//!   `scope`, the organizations' list in `optional_scope` (the user may decline it; then the
//!   organization is asked for by hand). While the browser is out: the page's busy line and a
//!   notification with Cancel; after a while, a hint at the other ways.
//! - **Other ways** (a dialog explaining each): a pasted OAuth token; an IAM token of Yandex Cloud
//!   (12 hours); the organization's own app — "for user authorization" goes the browser's way with
//!   its ClientID (no `scope`: its token gets the app's rights), "for API access or debugging"
//!   returns only to `https://oauth.yandex.ru/verification_code`, which shows a code: the user
//!   pastes it, and it is exchanged with the verifier.
//!
//! Forms can't show over Settings (Flux shows them when Settings closes): a dialog started from a
//! settings button says so.

use std::time::Duration;

use flux_plugin_api::bindings::flux::plugin::server::ServerRequest;
use flux_plugin_api::dialog::{Ask, Form, Values};
use flux_plugin_api::host::notifications::{self, Progress};
use flux_plugin_api::notify::Notice;
use flux_plugin_api::settings::Tone;
use flux_plugin_api::{http, server, storage, system, timers};
use flux_plugin_api::{settings as page, tr, trf};
use serde::{Deserialize, Serialize};

use super::{Account, Auth, CANCEL, OTHER_WAYS, Question, SIGN_IN, Way};
use crate::pkce::Pkce;
use crate::settings::ACCOUNT_STATUS;

/// Flux's Yandex ID app: a public client "for user authorization" (no secret: PKCE), its redirect
/// URIs `http://127.0.0.1:{52580,52581,52582}/callback`.
pub(super) const BUILTIN_CLIENT_ID: &str = "3051cf93a31648eb8f612bea8aca28e4";
/// The rights Flux's app asks for: Tracker, who the user is…
pub(super) const SCOPE: &str = "tracker:read tracker:write login:info login:email";
/// …and, if the user allows, the organizations they are in (Yandex 360).
pub(super) const OPTIONAL_SCOPE: &str = "directory:read_organization";
/// The ports Yandex returns to: registered with the app as `http://127.0.0.1:<port>/callback`.
pub const PORTS: [u16; 3] = [52580, 52581, 52582];
const AUTHORIZE: &str = "https://oauth.yandex.ru/authorize";
const TOKEN: &str = "https://oauth.yandex.ru/token";
const INFO: &str = "https://login.yandex.ru/info?format=json";
const TIMEOUT: Duration = Duration::from_secs(20);
/// A sign-in the browser doesn't finish is abandoned…
const SIGN_IN_WAIT: Duration = Duration::from_secs(5 * 60);
/// …and after a while the other ways are suggested.
const HINT_AFTER: Duration = Duration::from_secs(20);
/// The organization's own app (the plugin's storage): its ClientID and kind.
const OWN_APP: &str = "own-app";

/// A sign-in waiting for the browser.
pub(super) struct Pending {
    pkce: Pkce,
    client_id: String,
    redirect: String,
    way: Way,
    /// The sign-in is abandoned.
    timer: u64,
    /// The other ways are suggested.
    hint: Option<u64>,
    /// The notification with Cancel.
    notice: u64,
}

impl Pending {
    pub(super) fn owns(&self, timer: u64) -> bool {
        self.timer == timer || self.hint == Some(timer)
    }

    /// Its timers and its notification go.
    pub(super) fn close(&self) {
        timers::cancel(self.timer);
        if let Some(hint) = self.hint {
            timers::cancel(hint);
        }
        notifications::remove(self.notice);
    }
}

/// A sign-in with a code (a debug app) waiting for the code.
pub(super) struct CodeSignIn {
    pkce: Pkce,
    client_id: String,
}

/// What `/token` gave.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    pub expires_in: Option<i64>,
}

impl Tokens {
    /// A token pasted by the user: its lifetime unknown.
    fn pasted(token: &str) -> Tokens {
        Tokens {
            access: token.to_string(),
            refresh: None,
            expires_in: None,
        }
    }
}

/// The organization's own app.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct OwnApp {
    pub client_id: String,
    /// "browser" or "code".
    pub kind: String,
}

/// The organization's own app, as the user gave it last.
pub(super) fn own_app() -> Option<OwnApp> {
    storage::get::<OwnApp>(OWN_APP).filter(|app| !app.client_id.is_empty())
}

impl Auth {
    /// Sign In with Yandex: Flux's app in the browser.
    pub(super) fn sign_in_with_flux(&mut self) {
        self.start_browser(BUILTIN_CLIENT_ID, Way::Flux);
    }

    /// Opens Yandex ID's page in the browser and waits for it to come back to `/callback`.
    pub(super) fn start_browser(&mut self, client_id: &str, way: Way) {
        let Some(pkce) = Pkce::new() else {
            page::status(
                ACCOUNT_STATUS,
                &tr("No randomness for the sign-in"),
                Tone::Error,
            );
            return;
        };
        if let Some(pending) = self.pending.take() {
            pending.close();
            server::stop();
        }
        let Some(port) = PORTS.iter().find_map(|port| server::listen_on(*port).ok()) else {
            let message = trf(
                "The ports {0} are busy: close what uses them and try again",
                &[&"52580–52582"],
            );
            page::status(ACCOUNT_STATUS, &message, Tone::Error);
            return;
        };
        let redirect = server::url(port, "/callback");
        let url = authorize_url(client_id, &pkce, Some(&redirect), way == Way::Flux);
        if let Err(err) = system::open_url(&url) {
            server::stop();
            page::status(ACCOUNT_STATUS, &err, Tone::Error);
            return;
        }
        let notice = waiting_notice(false).send();
        notifications::set_progress(notice, Some(Progress::Indeterminate));
        self.pending = Some(Pending {
            pkce,
            client_id: client_id.to_string(),
            redirect,
            way,
            timer: timers::after(SIGN_IN_WAIT),
            hint: Some(timers::after(HINT_AFTER)),
            notice,
        });
        page::busy(ACCOUNT_STATUS, &tr("Finish the sign-in in the browser…"));
    }

    /// The notification's Cancel.
    pub(super) fn cancel_sign_in(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.close();
            server::stop();
        }
        self.show();
    }

    /// A timer of the sign-in in the browser: the hint, or the end of the wait.
    pub(super) fn pending_timer(&mut self, timer: u64) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if pending.hint == Some(timer) {
            pending.hint = None;
            waiting_notice(true).update(pending.notice);
            notifications::set_progress(pending.notice, Some(Progress::Indeterminate));
            page::busy(
                ACCOUNT_STATUS,
                &tr("Finish the sign-in in the browser… Doesn't work? Try Other Ways…"),
            );
            return;
        }
        // The browser didn't come back.
        if let Some(pending) = self.pending.take() {
            pending.close();
        }
        server::stop();
        page::status(ACCOUNT_STATUS, &tr("The sign-in timed out"), Tone::Warning);
    }

    /// The browser came back to `/callback`.
    pub(super) fn callback(&mut self, request: ServerRequest) {
        if request.route() != "/callback" {
            server::not_found(request.id);
            return;
        }
        let Some(pending) = self.pending.take() else {
            respond(
                request.id,
                false,
                &tr("This sign-in is over: start it again in Flux."),
            );
            return;
        };
        pending.close();
        let state_fits = request.query("state").as_deref() == Some(pending.pkce.state.as_str());
        let finish = |auth: &mut Auth, page_ok: bool, page_text: &str| {
            respond(request.id, page_ok, page_text);
            // The browser gets its page first; the server stops a moment later.
            auth.stop_timer = Some(timers::after(Duration::from_secs(1)));
        };
        if let Some(error) = request.query("error") {
            let words = request.query("error_description").unwrap_or(error);
            finish(
                self,
                false,
                &trf("Yandex didn't sign you in: {0}", &[&words]),
            );
            page::status(
                ACCOUNT_STATUS,
                &tr("The sign-in was cancelled"),
                Tone::Warning,
            );
            return;
        }
        let Some(code) = request.query("code").filter(|_| state_fits) else {
            finish(
                self,
                false,
                &tr("The answer doesn't belong to this sign-in."),
            );
            page::status(ACCOUNT_STATUS, &tr("The sign-in failed"), Tone::Error);
            return;
        };
        match exchange(
            &pending.client_id,
            &pending.pkce,
            &code,
            Some(&pending.redirect),
        ) {
            Ok(tokens) => {
                let account = account(&tokens.access);
                let who = account
                    .clone()
                    .map(|account| {
                        if account.display.is_empty() {
                            account.login
                        } else {
                            account.display
                        }
                    })
                    .unwrap_or_default();
                let words = if who.is_empty() {
                    tr("You're signed in to Flux. You can close this tab.")
                } else {
                    trf(
                        "You're signed in to Flux as {0}. You can close this tab.",
                        &[&who],
                    )
                };
                finish(self, true, &words);
                self.signed_in(pending.way, tokens, &pending.client_id, account);
            }
            Err(err) => {
                finish(self, false, &err);
                page::status(ACCOUNT_STATUS, &err, Tone::Error);
            }
        }
    }

    // --- Other ways ---

    /// Other Ways to Sign In: what each is, in plain words, a button each (Esc cancels: a fourth
    /// button doesn't fit the dialog in Russian). A dialog shows over Settings; the forms after it
    /// wait for Settings to close, and the dialog says so.
    pub(super) fn ask_other_ways(&mut self, from_settings: bool) {
        let mut message = [
            tr("OAuth token — if you already have one: a Yandex ID token with the rights tracker:read and tracker:write."),
            tr("IAM token — for a federated account of Yandex Cloud: yc iam create-token gives one, and it lives 12 hours."),
            tr("Own app — if your organization allows only its own Yandex ID app: you'll need its ClientID."),
        ]
        .join("\n\n");
        if from_settings {
            message.push_str("\n\n");
            message.push_str(&tr(
                "Flux asks for the token or the ClientID when you close Settings.",
            ));
        }
        let id = Ask::info(&tr("Other Ways to Sign In"))
            .message(&message)
            .normal(&tr("OAuth Token…"))
            .normal(&tr("IAM Token…"))
            .normal(&tr("Own App…"))
            .send();
        self.asked.push((id, Question::OtherWays));
    }

    /// A pasted OAuth token, or an IAM token.
    pub(super) fn ask_token(&mut self, iam: bool) {
        self.ask_token_again(iam, None);
    }

    fn ask_token_again(&mut self, iam: bool, error: Option<&str>) {
        let (title, message, placeholder) = if iam {
            (
                tr("Sign In with an IAM Token"),
                tr(
                    "For a federated account of Yandex Cloud. Get a token with yc iam create-token: it lives 12 hours, then Flux asks for a new one.",
                ),
                "t1.…",
            )
        } else {
            (
                tr("Sign In with an OAuth Token"),
                tr(
                    "A Yandex ID token with the rights tracker:read and tracker:write. Flux keeps it and finds your organization by itself.",
                ),
                "y0_…",
            )
        };
        let mut form = Form::new(&title, &tr("Sign In"))
            .message(&message)
            .text("token", &tr("Token"), "")
            .code()
            .placeholder(placeholder);
        if let Some(error) = error {
            form = form.error(error);
        }
        let id = form.send();
        let question = if iam {
            Question::IamForm
        } else {
            Question::TokenForm
        };
        self.asked.push((id, question));
    }

    /// The organization's own app: its ClientID and kind.
    fn ask_own_app(&mut self, error: Option<&str>) {
        let app = own_app().unwrap_or_default();
        let browser = tr("For user authorization: sign in in the browser");
        let code = tr("For API access or debugging: sign in with a code");
        let mut form = Form::new(&tr("Sign In with Your Organization's App"), &tr("Continue"))
            .message(&tr("The Yandex ID app your organization registered: its page on oauth.yandex.ru shows the ClientID and what it is for."))
            .text("client-id", &tr("ClientID"), &app.client_id)
            .code()
            .placeholder(&tr("The app's ClientID"))
            .choice(
                "kind",
                &tr("The app is"),
                &[("browser", &browser), ("code", &code)],
                Some(if app.kind == "code" { "code" } else { "browser" }),
            )
            .note(
                "redirects",
                &tr("To sign in in the browser, the app needs the redirect URIs http://127.0.0.1:52580/callback, :52581/callback and :52582/callback."),
            );
        if let Some(error) = error {
            form = form.error(error);
        }
        let id = form.send();
        self.asked.push((id, Question::AppForm));
    }

    /// A debug app: Yandex's page shows a code after the user allows access; the user pastes it.
    pub(super) fn start_code(&mut self, client_id: &str) {
        let Some(pkce) = Pkce::new() else {
            page::status(
                ACCOUNT_STATUS,
                &tr("No randomness for the sign-in"),
                Tone::Error,
            );
            return;
        };
        let url = authorize_url(client_id, &pkce, None, false);
        if let Err(err) = system::open_url(&url) {
            page::status(ACCOUNT_STATUS, &err, Tone::Error);
            return;
        }
        self.code = Some(CodeSignIn {
            pkce,
            client_id: client_id.to_string(),
        });
        self.ask_code(None);
    }

    fn ask_code(&mut self, error: Option<&str>) {
        let mut form = Form::new(&tr("Paste the Code"), &tr("Sign In"))
            .message(&tr("Once you allow access in the browser, Yandex shows a confirmation code: copy it here."))
            .text("code", &tr("Code"), "")
            .code()
            .placeholder("1234567");
        if let Some(error) = error {
            form = form.error(error);
        }
        let id = form.send();
        self.asked.push((id, Question::CodeForm));
    }

    /// The answer to Other Ways: which way.
    pub(super) fn other_way_chosen(&mut self, button: Option<u32>) {
        match button {
            Some(0) => self.ask_token(false),
            Some(1) => self.ask_token(true),
            Some(2) => self.ask_own_app(None),
            _ => {}
        }
    }

    /// A token form's answer: the token replaces the account's.
    pub(super) fn token_given(&mut self, iam: bool, values: Option<Values>) {
        let Some(values) = values else {
            return;
        };
        // A token copied with its header or quotes still works.
        let token = values
            .text("token")
            .trim_start_matches("OAuth ")
            .trim_start_matches("Bearer ")
            .trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace())
            .to_string();
        if token.is_empty() || token.contains(char::is_whitespace) {
            self.ask_token_again(iam, Some(&tr("Paste the token: one line without spaces")));
            return;
        }
        let (way, account) = if iam {
            (Way::Iam, None)
        } else {
            (Way::Token, account(&token))
        };
        self.signed_in(way, Tokens::pasted(&token), "", account);
    }

    /// The own app's form: the browser's way, or the code's.
    pub(super) fn own_app_given(&mut self, values: Option<Values>) {
        let Some(values) = values else {
            return;
        };
        let client_id = values.text("client-id");
        if client_id.is_empty() || client_id.contains(char::is_whitespace) {
            self.ask_own_app(Some(&tr("Paste the app's ClientID")));
            return;
        }
        let kind = values.get("kind").unwrap_or("browser").to_string();
        storage::set(
            OWN_APP,
            &OwnApp {
                client_id: client_id.clone(),
                kind: kind.clone(),
            },
        );
        if kind == "code" {
            self.start_code(&client_id);
        } else {
            self.start_browser(&client_id, Way::App);
        }
    }

    /// The code's form: exchanged with the verifier; a wrong code — asked again.
    pub(super) fn code_given(&mut self, values: Option<Values>) {
        let Some(values) = values else {
            self.code = None;
            return;
        };
        let Some(sign_in) = self.code.take() else {
            return;
        };
        let code = values.text("code");
        if code.is_empty() {
            self.code = Some(sign_in);
            self.ask_code(Some(&tr("Paste the code from Yandex's page")));
            return;
        }
        match exchange(&sign_in.client_id, &sign_in.pkce, &code, None) {
            Ok(tokens) => {
                let account = account(&tokens.access);
                self.signed_in(Way::Code, tokens, &sign_in.client_id, account);
            }
            Err(err) => {
                self.code = Some(sign_in);
                self.ask_code(Some(&err));
            }
        }
    }
}

/// The waiting notification: Cancel; with the hint — the other ways too.
fn waiting_notice(hint: bool) -> Notice {
    let notice = Notice::info(&tr("Finish the sign-in in the browser"));
    if hint {
        notice
            .body(&tr("Doesn't work? Try another way to sign in."))
            .action_with(&tr("Other Ways…"), SIGN_IN, OTHER_WAYS)
            .action_with(&tr("Cancel"), SIGN_IN, CANCEL)
    } else {
        notice
            .body(&tr("Flux opened Yandex's sign-in page: allow access there, and the browser brings you back."))
            .action_with(&tr("Cancel"), SIGN_IN, CANCEL)
    }
}

/// Yandex ID's page: the code with PKCE; `redirect` — the plugin's callback (none: the app's own,
/// a debug app's verification code page); Flux's app asks for its rights explicitly.
pub(super) fn authorize_url(
    client_id: &str,
    pkce: &Pkce,
    redirect: Option<&str>,
    builtin: bool,
) -> String {
    let mut query = vec![("response_type", "code"), ("client_id", client_id)];
    if let Some(redirect) = redirect {
        query.push(("redirect_uri", redirect));
    }
    query.extend([
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", pkce.state.as_str()),
    ]);
    if builtin {
        query.push(("scope", SCOPE));
        query.push(("optional_scope", OPTIONAL_SCOPE));
    }
    http::with_query(AUTHORIZE, &query)
}

/// The code for the tokens: `/token` with the verifier (no secret).
fn exchange(
    client_id: &str,
    pkce: &Pkce,
    code: &str,
    redirect: Option<&str>,
) -> Result<Tokens, String> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("code_verifier", pkce.verifier.as_str()),
    ];
    if let Some(redirect) = redirect {
        form.push(("redirect_uri", redirect));
    }
    token_request(&form)
}

/// A new access token for the refresh token, without a secret; Yandex may refuse it.
pub(super) fn refresh(refresh_token: &str, client_id: &str) -> Result<Tokens, String> {
    if client_id.is_empty() {
        return Err("the app of the token is unknown".into());
    }
    token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ])
}

fn token_request(form: &[(&str, &str)]) -> Result<Tokens, String> {
    let response = http::post(TOKEN)
        .form(form)
        .timeout(TIMEOUT)
        .fetch()
        .map_err(|err| err.message())?;
    parse_tokens(&response.text()).map_err(|words| {
        trf(
            "Yandex didn't give a token: {0}",
            &[&words.unwrap_or_else(|| response.status.to_string())],
        )
    })
}

/// `/token`'s answer: the tokens, or Yandex's words of why not (none — no words).
pub(super) fn parse_tokens(text: &str) -> Result<Tokens, Option<String>> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Answer {
        access_token: String,
        refresh_token: String,
        expires_in: Option<i64>,
        error: Option<String>,
        error_description: Option<String>,
    }
    let answer: Answer = serde_json::from_str(text).map_err(|_| None)?;
    if answer.access_token.is_empty() {
        return Err(answer.error_description.or(answer.error));
    }
    Ok(Tokens {
        access: answer.access_token,
        refresh: Some(answer.refresh_token).filter(|token| !token.is_empty()),
        expires_in: answer.expires_in,
    })
}

/// Who the token is, as Yandex ID says (`login:info`, `login:email`); none when it doesn't.
pub(super) fn account(token: &str) -> Option<Account> {
    let response = http::get(INFO)
        .header("Authorization", &format!("OAuth {token}"))
        .timeout(TIMEOUT)
        .fetch()
        .ok()
        .filter(|response| response.is_success())?;
    parse_account(&response.text())
}

pub(super) fn parse_account(text: &str) -> Option<Account> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Info {
        login: String,
        display_name: String,
        real_name: String,
        default_email: String,
    }
    let info: Info = serde_json::from_str(text).ok()?;
    let display = if info.real_name.is_empty() {
        info.display_name
    } else {
        info.real_name
    };
    let account = Account {
        email: info.default_email,
        login: info.login,
        display,
    };
    account.is_known().then_some(account)
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

    fn pkce() -> Pkce {
        Pkce {
            verifier: "v".into(),
            challenge: "c".into(),
            state: "s".into(),
        }
    }

    #[test]
    fn flux_asks_for_its_rights() {
        let url = authorize_url(
            BUILTIN_CLIENT_ID,
            &pkce(),
            Some("http://127.0.0.1:52580/callback"),
            true,
        );
        assert!(url.starts_with("https://oauth.yandex.ru/authorize?response_type=code&client_id=3051cf93a31648eb8f612bea8aca28e4&"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A52580%2Fcallback"));
        assert!(url.contains("code_challenge=c&code_challenge_method=S256&state=s"));
        assert!(
            url.contains("&scope=tracker%3Aread%20tracker%3Awrite%20login%3Ainfo%20login%3Aemail")
                || url.contains("&scope=tracker%3Aread+tracker%3Awrite+login%3Ainfo+login%3Aemail")
        );
        assert!(url.contains("&optional_scope=directory%3Aread_organization"));
        assert!(!url.contains("calendar"));
    }

    #[test]
    fn an_own_app_gets_its_own_rights() {
        // A debug app: no redirect (its own, the verification code page), no scope.
        let url = authorize_url("abc", &pkce(), None, false);
        assert!(!url.contains("redirect_uri"));
        assert!(!url.contains("scope"));
        assert!(url.contains("client_id=abc"));
    }

    #[test]
    fn tokens_and_refusals() {
        let tokens = parse_tokens(
            r#"{"access_token":"y0_a","refresh_token":"1:r","expires_in":31536000,"token_type":"bearer"}"#,
        )
        .unwrap();
        assert_eq!(tokens.access, "y0_a");
        assert_eq!(tokens.refresh.as_deref(), Some("1:r"));
        assert_eq!(tokens.expires_in, Some(31536000));
        let tokens = parse_tokens(r#"{"access_token":"y0_b"}"#).unwrap();
        assert_eq!((tokens.refresh, tokens.expires_in), (None, None));
        assert_eq!(
            parse_tokens(r#"{"error":"invalid_grant","error_description":"Code has expired"}"#),
            Err(Some("Code has expired".into()))
        );
        assert_eq!(
            parse_tokens(r#"{"error":"invalid_client"}"#),
            Err(Some("invalid_client".into()))
        );
        assert_eq!(parse_tokens("<html>"), Err(None));
    }

    #[test]
    fn who_yandex_id_says() {
        let account = parse_account(
            r#"{"login":"egor","display_name":"egor","real_name":"Егор Агеев","default_email":"e@x.ru"}"#,
        )
        .unwrap();
        assert_eq!(
            (account.login.as_str(), account.display.as_str()),
            ("egor", "Егор Агеев")
        );
        assert_eq!(account.email, "e@x.ru");
        assert_eq!(parse_account(r#"{"id":"1"}"#), None);
        assert_eq!(parse_account("nope"), None);
    }

    #[test]
    fn escapes_the_page() {
        assert_eq!(escape("<b>&\"</b>"), "&lt;b&gt;&amp;&quot;&lt;/b&gt;");
    }
}
