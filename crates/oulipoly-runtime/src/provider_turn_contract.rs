//! Provider-turn batch bound consumed by the retained ordinary resume path.
//!
//! ## Declared roles
//!
//! `validator`

/// Maximum mailbox rows admitted to one provider turn.
pub const MAILBOX_BATCH_MAX_ROWS: usize = 20;
