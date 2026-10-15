//! The keys of the manifest's settings, and reading them.

use flux_plugin_api::setting;

pub const ACCOUNT_STATUS: &str = "account-status";
pub const OAUTH_TOKEN: &str = "oauth-token";
pub const IAM_TOKEN: &str = "iam-token";
pub const ORG_ID: &str = "org-id";
pub const ORG_TYPE: &str = "org-type";
pub const CONNECTION_STATUS: &str = "connection-status";
pub const MY_QUERY: &str = "my-query";
pub const BRANCH_TEMPLATE: &str = "branch-template";
pub const COMMIT_TEMPLATE: &str = "commit-template";
pub const POLL_MINUTES: &str = "poll-minutes";
pub const NOTIFY_ASSIGNED: &str = "notify-assigned";
pub const NOTIFY_SUMMONED: &str = "notify-summoned";
pub const NOTIFY_STATUS: &str = "notify-status";
pub const OFFER_WORKLOG: &str = "offer-worklog";
pub const ACTIONS: &str = "actions";
pub const WORK_STATUS: &str = "work-status";
pub const WORK_BRANCH: &str = "work-branch";
pub const WORK_CONTEXT: &str = "work-context";
pub const WORK_TIMER: &str = "work-timer";
pub const CALENDAR_ENABLED: &str = "calendar-enabled";
pub const REMIND_MINUTES: &str = "remind-minutes";
pub const HIDDEN_CALENDARS: &str = "hidden-calendars";
pub const CALDAV_LOGIN: &str = "caldav-login";
pub const CALDAV_PASSWORD: &str = "caldav-password";
pub const CLIENT_ID: &str = "client-id";

/// A string setting, trimmed; none when empty.
pub fn text(key: &str) -> Option<String> {
    setting::<String>(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A switch.
pub fn on(key: &str) -> bool {
    setting::<bool>(key).unwrap_or(false)
}

/// A number.
pub fn number(key: &str) -> i64 {
    setting::<i64>(key).unwrap_or(0)
}
