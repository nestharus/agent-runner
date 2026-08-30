//! Runner-private automatic-wake process environment vocabulary.
//!
//! ## Declared roles
//!
//! Roles: accessor.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutoWakeEnvironmentVariable(&'static str);

impl AutoWakeEnvironmentVariable {
    pub const MARKER: Self = Self("OULIPOLY_AUTO_WAKE");
    pub const SESSION_ID: Self = Self("OULIPOLY_AUTO_WAKE_SESSION_ID");
    pub const CLAIM_TOKEN: Self = Self("OULIPOLY_AUTO_WAKE_TOKEN");
    pub const COUNT: Self = Self("OULIPOLY_AUTO_WAKE_COUNT");
    pub const RETRY_BASE_MILLISECONDS: Self = Self("OULIPOLY_AUTO_WAKE_RETRY_BASE_MS");
    #[cfg(feature = "test-support")]
    pub const TEST_SENTINEL: Self = Self("OULIPOLY_AUTO_WAKE_TEST_SENTINEL");

    #[cfg(not(feature = "test-support"))]
    pub const ALL: [Self; 5] = [
        Self::MARKER,
        Self::SESSION_ID,
        Self::CLAIM_TOKEN,
        Self::COUNT,
        Self::RETRY_BASE_MILLISECONDS,
    ];

    #[cfg(feature = "test-support")]
    pub const ALL: [Self; 6] = [
        Self::MARKER,
        Self::SESSION_ID,
        Self::CLAIM_TOKEN,
        Self::COUNT,
        Self::RETRY_BASE_MILLISECONDS,
        Self::TEST_SENTINEL,
    ];

    pub const fn name(self) -> &'static str {
        self.0
    }
}
