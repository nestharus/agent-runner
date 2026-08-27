//! Runner-private automatic-wake process environment vocabulary.
//!
//! ## Declared roles
//!
//! Roles: accessor, entity.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutoWakeEnvironmentVariable(&'static str);

impl AutoWakeEnvironmentVariable {
    pub const ALL: [Self; 5] = [
        Self("OULIPOLY_AUTO_WAKE"),
        Self("OULIPOLY_AUTO_WAKE_SESSION_ID"),
        Self("OULIPOLY_AUTO_WAKE_TOKEN"),
        Self("OULIPOLY_AUTO_WAKE_COUNT"),
        Self("OULIPOLY_AUTO_WAKE_RETRY_BASE_MS"),
    ];

    // Producer handles derive from ALL; the private field prevents unregistered construction.
    pub const MARKER: Self = Self::ALL[0];
    pub const SESSION_ID: Self = Self::ALL[1];
    pub const CLAIM_TOKEN: Self = Self::ALL[2];
    pub const COUNT: Self = Self::ALL[3];
    pub const RETRY_BASE_MILLISECONDS: Self = Self::ALL[4];

    pub const fn name(self) -> &'static str {
        self.0
    }
}
