//! ## Declared roles
//!
//! `accessor`, `orchestration`
//!
//! Read-only observability snapshots for interactive session monitoring.

mod agent_bash;
mod dto;
mod invocation;
mod limits;
mod liveness;
mod mailbox;
mod provider_inspect_transcript_source;
mod service;
mod state_access;
mod transcript_source;
mod visibility;

pub use dto::{
    CancelRef, InspectRef, LivenessStatus, MailboxState, MonitorDiagnostic,
    MonitorDiagnosticSeverity, MonitorNode, MonitorNodeId, MonitorNodeKind, MonitorProcessIdentity,
    MonitorSnapshot, MonitorStatus, MonitorSummary, WakeState,
};
pub use limits::SnapshotLimits;
pub use service::{
    ObservabilityRoot, ObservabilitySnapshotPort, ProductionObservabilitySnapshotService,
};
