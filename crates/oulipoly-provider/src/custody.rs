//! Direct-operation custody. Receipts are emitted by the retained Child owner,
//! never reconstructed from diagnostics or a PID supplied by a caller.
//! Roles: accessor, validator, orchestration.
use crate::generated::ProcessStatus;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ProviderOperation {
    Describe,
    Policy,
    Launch,
    TerminalClassify,
    Other(String),
}
impl ProviderOperation {
    pub fn from_subcommand(value: &str) -> Self {
        match value {
            "describe" => Self::Describe,
            "policy.evaluate" => Self::Policy,
            "launch" => Self::Launch,
            "terminal.classify" => Self::TerminalClassify,
            other => Self::Other(other.into()),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessIdentity {
    pub os_pid: i64,
    pub os_boot_id: String,
    pub os_pid_starttime_ticks: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct ActorSettlementReceipt {
    pub attempt_id: Uuid,
    pub operation: ProviderOperation,
    pub spawned: bool,
    pub exact_process_identity: Option<ProcessIdentity>,
    pub process_status: Option<ProcessStatus>,
    pub process_tree_terminated: bool,
    pub leader_reaped: bool,
    pub force_killed: bool,
    pub host_cancellation_requested: bool,
    pub operation_finished: bool,
    pub uncertain: bool,
}
impl ActorSettlementReceipt {
    pub fn effect_incapable(&self) -> bool {
        self.operation_finished
            && !self.uncertain
            && if self.spawned {
                self.exact_process_identity.is_some()
                    && self.process_status.is_some()
                    && self.process_tree_terminated
                    && self.leader_reaped
            } else {
                self.exact_process_identity.is_none() && self.process_status.is_none()
            }
    }
}
#[derive(Debug, Clone)]
pub struct AttemptActorCustody {
    attempt_id: Uuid,
    records: Arc<Mutex<Vec<OperationCustody>>>,
    requests: Arc<Mutex<Vec<GeneratedRequestIdentity>>>,
}
#[derive(Debug, Clone)]
pub struct OperationCustody(pub(crate) Arc<Mutex<ActorSettlementReceipt>>);
pub(crate) struct OperationGuard(pub OperationCustody);
impl Drop for OperationGuard {
    fn drop(&mut self) {
        let mut receipt = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        receipt.operation_finished = true;
        receipt.uncertain |= std::thread::panicking();
    }
}
impl AttemptActorCustody {
    pub fn new(attempt_id: Uuid) -> Self {
        Self {
            attempt_id,
            records: Arc::default(),
            requests: Arc::default(),
        }
    }
    pub(crate) fn begin(&self, subcommand: &str) -> OperationGuard {
        let record = OperationCustody(Arc::new(Mutex::new(ActorSettlementReceipt {
            attempt_id: self.attempt_id,
            operation: ProviderOperation::from_subcommand(subcommand),
            spawned: false,
            exact_process_identity: None,
            process_status: None,
            process_tree_terminated: false,
            leader_reaped: false,
            force_killed: false,
            host_cancellation_requested: false,
            operation_finished: false,
            uncertain: false,
        })));
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(record.clone());
        OperationGuard(record)
    }
    /// Called only by the operation owner when its branch did no process-capable work.
    pub fn record_not_invoked(&self, subcommand: &str) {
        drop(self.begin(subcommand));
    }
    pub fn receipts(&self) -> Vec<ActorSettlementReceipt> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|r| r.0.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .collect()
    }
}

/// Host-generated correlation and the verbatim wire ID are distinct values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GeneratedRequestIdentity {
    pub operation: ProviderOperation,
    pub correlation: Uuid,
    pub wire_request_id: String,
}
impl GeneratedRequestIdentity {
    pub fn new(operation: ProviderOperation, prefix: &str) -> Self {
        let correlation = Uuid::new_v4();
        Self {
            operation,
            correlation,
            wire_request_id: format!("{prefix}{correlation}"),
        }
    }
}
impl AttemptActorCustody {
    pub fn record_request(&self, request: GeneratedRequestIdentity) {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(request);
    }
    pub fn requests(&self) -> Vec<GeneratedRequestIdentity> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
