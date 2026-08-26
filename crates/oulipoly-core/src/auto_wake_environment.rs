//! Runner-private automatic-wake process environment vocabulary.
//!
//! ## Declared roles
//!
//! Role: accessor.

pub const AUTO_WAKE_ENV: &str = "OULIPOLY_AUTO_WAKE";
pub const AUTO_WAKE_SESSION_ID_ENV: &str = "OULIPOLY_AUTO_WAKE_SESSION_ID";
pub const AUTO_WAKE_TOKEN_ENV: &str = "OULIPOLY_AUTO_WAKE_TOKEN";
pub const AUTO_WAKE_COUNT_ENV: &str = "OULIPOLY_AUTO_WAKE_COUNT";
pub const AUTO_WAKE_RETRY_BASE_MS_ENV: &str = "OULIPOLY_AUTO_WAKE_RETRY_BASE_MS";

pub const RUNNER_PRIVATE_AUTO_WAKE_ENV_NAMES: [&str; 5] = [
    AUTO_WAKE_ENV,
    AUTO_WAKE_SESSION_ID_ENV,
    AUTO_WAKE_TOKEN_ENV,
    AUTO_WAKE_COUNT_ENV,
    AUTO_WAKE_RETRY_BASE_MS_ENV,
];
