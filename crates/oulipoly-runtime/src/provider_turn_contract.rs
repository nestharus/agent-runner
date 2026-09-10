//! Shared provider-turn boundaries consumed by both the current resume path and
//! the target resident-supervisor adapter.
//!
//! ## Declared roles
//!
//! `validator`

/// Maximum mailbox rows admitted to one provider turn.
pub const MAILBOX_BATCH_MAX_ROWS: usize = 20;
