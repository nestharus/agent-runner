//! ## Declared roles
//!
//! `validator`
//!
//! Snapshot query bounds used to keep every refresh finite.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotLimits {
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
            invocation_subtree_depth: 64,
            max_invocation_nodes: 200,
            mailbox_cap: 100,
            agent_bash_scan_dirs: 100,
            log_tail_bytes: 4 * 1024,
            transcript_tail_bytes: 16 * 1024,
            wake_claim_stale_after_seconds: 600,
        }
    }
}
