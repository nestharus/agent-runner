//! ## Declared roles
//!
//! `validator`
//!
//! Snapshot query bounds used to keep every refresh finite.

const DEFAULT_INVOCATION_SUBTREE_DEPTH: usize = 64;
const DEFAULT_MAX_INVOCATION_NODES: usize = 200;
const DEFAULT_MAILBOX_CAP: usize = 100;
const DEFAULT_AGENT_BASH_SCAN_DIRS: usize = 100;
const DEFAULT_LOG_TAIL_BYTES: usize = 4 * 1024;
const DEFAULT_TRANSCRIPT_TAIL_BYTES: usize = 16 * 1024;
const DEFAULT_WAKE_CLAIM_STALE_AFTER_SECONDS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotLimits {
    pub include_terminal: bool,
    pub invocation_subtree_depth: usize,
    pub max_invocation_nodes: usize,
    pub mailbox_cap: usize,
    pub agent_bash_scan_dirs: usize,
    pub log_tail_bytes: usize,
    pub transcript_tail_bytes: usize,
    pub wake_claim_stale_after_seconds: i64,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            include_terminal: false,
            invocation_subtree_depth: DEFAULT_INVOCATION_SUBTREE_DEPTH,
            max_invocation_nodes: DEFAULT_MAX_INVOCATION_NODES,
            mailbox_cap: DEFAULT_MAILBOX_CAP,
            agent_bash_scan_dirs: DEFAULT_AGENT_BASH_SCAN_DIRS,
            log_tail_bytes: DEFAULT_LOG_TAIL_BYTES,
            transcript_tail_bytes: DEFAULT_TRANSCRIPT_TAIL_BYTES,
            wake_claim_stale_after_seconds: DEFAULT_WAKE_CLAIM_STALE_AFTER_SECONDS,
        }
    }
}
