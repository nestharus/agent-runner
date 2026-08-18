//! Session-local mailbox ingress for the resident supervisor.

use std::collections::HashSet;
use std::fmt;
#[cfg(unix)]
use std::path::Path;

use oulipoly_state::mailbox::{
    MAILBOX_INGRESS_EXPIRED_ERROR, MAILBOX_PAYLOAD_VERIFICATION_FAILED_ERROR, MailboxDb, MailboxRow,
};
use oulipoly_state::{ExternalIngress, SessionLifecycleRepository, StateDb, SupervisorFence};
use serde::{Deserialize, Serialize};

use crate::session_supervisor::{
    SessionNotification, SessionSupervisor, SupervisorError, SupervisorPhase,
};

const PERSISTED_MAILBOX_INGRESS_SCHEMA_VERSION: u64 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct PersistedMailboxIngressV1 {
    schema_version: u64,
    mailbox_row: MailboxRow,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeadlessResumePoke {
    pub session_id: String,
    pub supervisor_generation: i64,
    pub lease_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngressDrain {
    pub accepted_sequences: Vec<i64>,
    pub recovered_sequences: Vec<i64>,
    pub paused: bool,
    pub queue_saturated: bool,
}

#[derive(Debug)]
pub enum SessionIngressError {
    InvalidBatchLimit,
    StalePoke,
    Mailbox(String),
    #[cfg(unix)]
    ControlTransport(crate::executor::cli::pty_broker::PtyControlClientError),
    State(oulipoly_state::SessionLifecycleError),
    Decode(String),
    UnsupportedPersistedPayloadVersion(u64),
    Deserialize(serde_json::Error),
    Serialize(serde_json::Error),
    ControlPayload(serde_json::Error),
    Supervisor(SupervisorError),
}

impl fmt::Display for SessionIngressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBatchLimit => formatter.write_str("mailbox ingress batch limit is zero"),
            Self::StalePoke => formatter.write_str("headless resume poke is stale or mis-targeted"),
            Self::Mailbox(error) => write!(formatter, "mailbox ingress: {error}"),
            #[cfg(unix)]
            Self::ControlTransport(error) => {
                write!(formatter, "headless control transport: {}", error.message)
            }
            Self::State(error) => write!(formatter, "durable ingress: {error}"),
            Self::Decode(error) => write!(formatter, "mailbox ingress decode: {error}"),
            Self::UnsupportedPersistedPayloadVersion(version) => write!(
                formatter,
                "mailbox ingress payload schema version {version} is unsupported"
            ),
            Self::Deserialize(error) => {
                write!(formatter, "mailbox ingress deserialization: {error}")
            }
            Self::Serialize(error) => write!(formatter, "mailbox ingress serialization: {error}"),
            Self::ControlPayload(error) => write!(formatter, "headless control payload: {error}"),
            Self::Supervisor(error) => write!(formatter, "resident owner: {error}"),
        }
    }
}

impl std::error::Error for SessionIngressError {}

impl From<oulipoly_state::SessionLifecycleError> for SessionIngressError {
    fn from(value: oulipoly_state::SessionLifecycleError) -> Self {
        Self::State(value)
    }
}

pub struct SessionMailboxIngress<Input, Map> {
    session_id: String,
    chain_id: Option<String>,
    owner_fence: SupervisorFence,
    batch_limit: usize,
    mailbox: MailboxDb,
    lifecycle: StateDb,
    map_notification: Map,
    recovered: bool,
    recovery_cursor: i64,
    hydrated_delivery_ids: HashSet<String>,
    _input: std::marker::PhantomData<fn() -> Input>,
}

impl<Input, Map> SessionMailboxIngress<Input, Map>
where
    Input: Clone + Send + 'static,
    Map: FnMut(&str, &MailboxRow) -> Result<SessionNotification<Input>, String>,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: impl Into<String>,
        chain_id: Option<String>,
        owner_fence: SupervisorFence,
        batch_limit: usize,
        mailbox: MailboxDb,
        lifecycle: StateDb,
        map_notification: Map,
    ) -> Result<Self, SessionIngressError> {
        if batch_limit == 0 {
            return Err(SessionIngressError::InvalidBatchLimit);
        }
        Ok(Self {
            session_id: session_id.into(),
            chain_id,
            owner_fence,
            batch_limit,
            mailbox,
            lifecycle,
            map_notification,
            recovered: false,
            recovery_cursor: 0,
            hydrated_delivery_ids: HashSet::new(),
            _input: std::marker::PhantomData,
        })
    }

    pub fn fallback_read<Output: Send + 'static>(
        &mut self,
        owner: &SessionSupervisor<Input, Output>,
        accepted_at: i64,
    ) -> Result<IngressDrain, SessionIngressError> {
        self.drain(owner, accepted_at)
    }

    pub fn handle_poke<Output: Send + 'static>(
        &mut self,
        poke: &HeadlessResumePoke,
        owner: &SessionSupervisor<Input, Output>,
        accepted_at: i64,
    ) -> Result<IngressDrain, SessionIngressError> {
        if poke.session_id != self.session_id
            || poke.supervisor_generation != self.owner_fence.generation
            || poke.lease_token != self.owner_fence.token
        {
            return Err(SessionIngressError::StalePoke);
        }
        self.drain(owner, accepted_at)
    }

    pub fn handle_control_payload<Output: Send + 'static>(
        &mut self,
        payload: &str,
        owner: &SessionSupervisor<Input, Output>,
        accepted_at: i64,
    ) -> Result<IngressDrain, SessionIngressError> {
        let poke = serde_json::from_str(payload).map_err(SessionIngressError::ControlPayload)?;
        self.handle_poke(&poke, owner, accepted_at)
    }

    fn drain<Output: Send + 'static>(
        &mut self,
        owner: &SessionSupervisor<Input, Output>,
        accepted_at: i64,
    ) -> Result<IngressDrain, SessionIngressError> {
        let snapshot = owner.status().map_err(SessionIngressError::Supervisor)?;
        if snapshot.session_id != self.session_id || snapshot.fence != self.owner_fence {
            return Err(SessionIngressError::StalePoke);
        }
        if self
            .mailbox
            .notifications_paused(&self.session_id)
            .map_err(SessionIngressError::Mailbox)?
        {
            return Ok(IngressDrain {
                accepted_sequences: Vec::new(),
                recovered_sequences: Vec::new(),
                paused: true,
                queue_saturated: false,
            });
        }

        let mut drain = IngressDrain {
            accepted_sequences: Vec::new(),
            recovered_sequences: Vec::new(),
            paused: false,
            queue_saturated: false,
        };
        if !self.recovered {
            self.recovered = self.recover_accepted_pending(owner, accepted_at, &mut drain)?;
            if !self.recovered {
                return Ok(drain);
            }
            self.hydrated_delivery_ids = HashSet::new();
        }
        if snapshot.phase == SupervisorPhase::Draining {
            return Ok(drain);
        }

        let cursor = self.lifecycle.external_ingress_cursor(&self.session_id)?;
        let rows = self
            .mailbox
            .list_pending_for_delivery_after(
                &self.session_id,
                self.chain_id.as_deref(),
                cursor,
                self.batch_limit,
            )
            .map_err(SessionIngressError::Mailbox)?;
        for row in rows {
            if self.mailbox.verify_mailbox_row_payload(&row).is_err() {
                self.mailbox
                    .mark_delivery_failed(
                        &self.session_id,
                        self.chain_id.as_deref(),
                        &[row.seq],
                        MAILBOX_PAYLOAD_VERIFICATION_FAILED_ERROR,
                    )
                    .map_err(SessionIngressError::Mailbox)?;
                continue;
            }
            let notification = (self.map_notification)(&self.session_id, &row)
                .map_err(SessionIngressError::Decode)?;
            let ingress = mailbox_external_ingress(&self.session_id, &row)?;
            match owner.notify_external(ingress, notification, accepted_at) {
                Ok(_) => drain.accepted_sequences.push(row.seq),
                Err(SupervisorError::QueueFull) => {
                    drain.queue_saturated = true;
                    break;
                }
                Err(SupervisorError::DuplicateOrStaleSequence) => {}
                Err(SupervisorError::Expired) => {
                    self.mailbox
                        .mark_delivery_failed(
                            &self.session_id,
                            self.chain_id.as_deref(),
                            &[row.seq],
                            MAILBOX_INGRESS_EXPIRED_ERROR,
                        )
                        .map_err(SessionIngressError::Mailbox)?;
                }
                Err(error) => {
                    self.recovered = false;
                    return Err(SessionIngressError::Supervisor(error));
                }
            }
        }
        Ok(drain)
    }

    fn recover_accepted_pending<Output: Send + 'static>(
        &mut self,
        owner: &SessionSupervisor<Input, Output>,
        accepted_at: i64,
        drain: &mut IngressDrain,
    ) -> Result<bool, SessionIngressError> {
        let rows = self.lifecycle.accepted_pending_external_ingress(
            &self.session_id,
            self.recovery_cursor,
            self.batch_limit,
        )?;
        let complete = rows.len() < self.batch_limit;
        for ingress in rows {
            if self.hydrated_delivery_ids.contains(&ingress.ingress_id) {
                self.recovery_cursor = ingress.sequence;
                continue;
            }
            let row = deserialize_persisted_mailbox_ingress(&ingress.payload)?;
            let notification = (self.map_notification)(&self.session_id, &row)
                .map_err(SessionIngressError::Decode)?;
            match owner.notify_external(ingress.clone(), notification, accepted_at) {
                Ok(_) => drain.recovered_sequences.push(ingress.sequence),
                Err(SupervisorError::QueueFull) => {
                    drain.queue_saturated = true;
                    return Ok(false);
                }
                Err(SupervisorError::DuplicateOrStaleSequence) => {}
                Err(error) => return Err(SessionIngressError::Supervisor(error)),
            }
            self.recovery_cursor = ingress.sequence;
            self.hydrated_delivery_ids.insert(ingress.ingress_id);
        }
        Ok(complete)
    }
}

#[cfg(unix)]
pub fn send_headless_resume_poke(
    path: impl AsRef<Path>,
    poke: &HeadlessResumePoke,
) -> Result<crate::executor::cli::pty_broker::PtyControlResponse, SessionIngressError> {
    let payload = serde_json::to_string(poke).map_err(SessionIngressError::Serialize)?;
    crate::executor::cli::pty_broker::send_control_operation(
        path,
        crate::executor::cli::pty_broker::LiveSessionControlOperation::HeadlessResume(&payload),
    )
    .map_err(SessionIngressError::ControlTransport)
}

fn mailbox_external_ingress(
    session_id: &str,
    row: &MailboxRow,
) -> Result<ExternalIngress, SessionIngressError> {
    Ok(ExternalIngress {
        session_id: session_id.to_owned(),
        sequence: row.seq,
        ingress_id: format!("mailbox:{session_id}:{}", row.seq),
        payload: serde_json::to_string(&PersistedMailboxIngressV1 {
            schema_version: PERSISTED_MAILBOX_INGRESS_SCHEMA_VERSION,
            mailbox_row: row.clone(),
        })
        .map_err(SessionIngressError::Serialize)?,
    })
}

fn deserialize_persisted_mailbox_ingress(payload: &str) -> Result<MailboxRow, SessionIngressError> {
    let value: serde_json::Value =
        serde_json::from_str(payload).map_err(SessionIngressError::Deserialize)?;
    let Some(schema_version) = value.get("schema_version") else {
        return serde_json::from_value(value).map_err(SessionIngressError::Deserialize);
    };
    let schema_version = schema_version.as_u64().ok_or_else(|| {
        SessionIngressError::Decode(
            "persisted payload schema_version must be a non-negative integer".to_owned(),
        )
    })?;
    if schema_version != PERSISTED_MAILBOX_INGRESS_SCHEMA_VERSION {
        return Err(SessionIngressError::UnsupportedPersistedPayloadVersion(
            schema_version,
        ));
    }
    serde_json::from_value::<PersistedMailboxIngressV1>(value)
        .map(|persisted| persisted.mailbox_row)
        .map_err(SessionIngressError::Deserialize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox_row() -> MailboxRow {
        MailboxRow {
            seq: 7,
            session_id: "session-a".to_owned(),
            kind: "agent_bash_complete".to_owned(),
            handle: "ab-test".to_owned(),
            payload_json: r#"{"handle":"ab-test"}"#.to_owned(),
            enqueued_at: "2026-08-09T00:00:00Z".to_owned(),
            delivered_at: None,
            delivered_by_invocation_uuid: None,
            delivery_attempts: 0,
            delivery_error: None,
            owner_invocation_uuid: Some("owner-a".to_owned()),
            matched_os_pid: Some(42),
            matched_os_boot_id: Some("boot-a".to_owned()),
            matched_os_pid_starttime_ticks: Some(420),
            matched_chain_index: Some(2),
            state_dir: "/tmp/state".to_owned(),
            meta_path: "/tmp/meta".to_owned(),
            log_path: "/tmp/log".to_owned(),
            rc_path: "/tmp/rc".to_owned(),
            rc: 0,
            payload_file_path: Some("/tmp/payload".to_owned()),
            payload_sha256: Some("sha256".to_owned()),
            payload_byte_len: Some(24),
            payload_retention_policy: Some("until_terminal_disposition".to_owned()),
            payload_compacted_at: None,
            submission_token: None,
            target_kind: Some("session".to_owned()),
            target_id: Some("session-a".to_owned()),
        }
    }

    #[test]
    fn versioned_persisted_ingress_payload_round_trips() {
        let row = mailbox_row();
        let payload = mailbox_external_ingress("session-a", &row).unwrap().payload;
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(value["schema_version"], 1);
        assert_eq!(
            deserialize_persisted_mailbox_ingress(&payload).unwrap(),
            row
        );
    }

    #[test]
    fn legacy_raw_mailbox_row_payload_recovers_at_the_persistence_seam() {
        let row = mailbox_row();
        let payload = serde_json::to_string(&row).unwrap();

        assert_eq!(
            deserialize_persisted_mailbox_ingress(&payload).unwrap(),
            row
        );
    }

    #[test]
    fn unsupported_future_persisted_ingress_payload_version_is_rejected() {
        let payload = serde_json::json!({
            "schema_version": PERSISTED_MAILBOX_INGRESS_SCHEMA_VERSION + 1,
            "mailbox_row": mailbox_row(),
        })
        .to_string();

        assert!(matches!(
            deserialize_persisted_mailbox_ingress(&payload),
            Err(SessionIngressError::UnsupportedPersistedPayloadVersion(2))
        ));
    }
}
