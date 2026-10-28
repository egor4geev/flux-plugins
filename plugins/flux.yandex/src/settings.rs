//! The keys of the manifest's settings, and reading them. The account's tokens aren't settings
//! any more: [`crate::auth`] keeps them (the same secret keys as the old `oauth-token` and
//! `iam-token` secrets), with the organization in the plugin's storage.

use flux_plugin_api::setting;

pub const ACCOUNT_STATUS: &str = "account-status";
pub const ORGANIZATION_STATUS: &str = "organization-status";
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
