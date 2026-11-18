//! FluxGram's app key with Telegram — the one place for it.
//!
//! A Telegram client signs in with its application's `api_id` and `api_hash` (my.telegram.org →
//! API development tools). FluxGram's own come from its author; a build without them (0 and empty)
//! says in the sign-in dialog that FluxGram isn't set up instead of failing.
//!
//! `--features test-dc` builds a FluxGram for Telegram's test servers (`use_test_dc`) with TDLib's
//! example credentials, for development: those are never used with the production servers.

/// The application's `api_id` (my.telegram.org, FluxGram's author, 2026-11-18).
#[cfg(not(feature = "test-dc"))]
pub const API_ID: i32 = 32013619;

/// The application's `api_hash`.
#[cfg(not(feature = "test-dc"))]
pub const API_HASH: &str = "d78bf756256c7864b3e38611ae75147b";

/// Telegram's production servers.
#[cfg(not(feature = "test-dc"))]
pub const TEST_DC: bool = false;

/// TDLib's example `api_id` (td/example): Telegram's test servers only.
#[cfg(feature = "test-dc")]
pub const API_ID: i32 = 94575;

/// TDLib's example `api_hash`: Telegram's test servers only.
#[cfg(feature = "test-dc")]
pub const API_HASH: &str = "a3406de8d171bb422bb6ddf3bbd800e2";

/// Telegram's test servers: their own accounts (`99966XYYYY`), apart from the real ones.
#[cfg(feature = "test-dc")]
pub const TEST_DC: bool = true;

/// FluxGram has its app key.
// The constants are what the author edits: the check is meant to be on them.
#[allow(clippy::const_is_empty)]
pub fn set_up() -> bool {
    API_ID != 0 && !API_HASH.is_empty()
}
