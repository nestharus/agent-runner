//! Marker verification config access and env parsing.
//!
//! ## Declared roles
//! accessor, parser, orchestration
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/marker_verification/config.rs
//!     role: adapter
//!     Translates:
//!       - process environment marker-verification config contract (`OULIPOLY_MARKER_VERIFICATION_COOLDOWN_SECS`, `OULIPOLY_MARKER_RELEASE_SLACK_SECS`)
//! ```

/// Per-provider refresh cooldown. While a successful `--usage` is younger
/// than this we trust the cached windows and skip the script call. 60s
/// matches the `usage-refresh-locks` dir convention and is well under the
/// 5h smallest rolling window.
pub const DEFAULT_MARKER_VERIFICATION_COOLDOWN_SECS: i64 = 60;

/// Slack ahead of `next_available_at`. Once the marker is within this many
/// seconds of its stated release we treat it as already expired.
pub const DEFAULT_MARKER_RELEASE_SLACK_SECS: i64 = 60;

const COOLDOWN_ENV_VAR: &str = "OULIPOLY_MARKER_VERIFICATION_COOLDOWN_SECS";
const SLACK_ENV_VAR: &str = "OULIPOLY_MARKER_RELEASE_SLACK_SECS";

pub(super) fn cooldown_secs() -> i64 {
    env_secs_or_default(COOLDOWN_ENV_VAR, DEFAULT_MARKER_VERIFICATION_COOLDOWN_SECS)
}

/// Effective release slack — `OULIPOLY_MARKER_RELEASE_SLACK_SECS` if set,
/// otherwise `DEFAULT_MARKER_RELEASE_SLACK_SECS`. Public so the balancer's
/// cached-only `provider_is_quota_exhausted` predicate respects the same
/// env override as the verify path.
pub fn release_slack_secs() -> i64 {
    env_secs_or_default(SLACK_ENV_VAR, DEFAULT_MARKER_RELEASE_SLACK_SECS)
}

fn env_secs_or_default(name: &str, default: i64) -> i64 {
    env_secs(name).unwrap_or(default)
}

fn env_secs(name: &str) -> Option<i64> {
    env_value(name).and_then(|value| parse_secs(&value))
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn parse_secs(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok()
}
