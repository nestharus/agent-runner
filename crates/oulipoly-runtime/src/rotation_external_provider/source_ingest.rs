//! Settle the source's existing canonical projection before taking a rotation snapshot.
//! Competing workers retain their leases; this caller may advance only an available
//! source stream. Unrelated session ingestion remains independent.

use super::{ExternalRotationError, ExternalRotationIdentity, error_formatter};
use crate::provider_registry::ProviderRegistryHandle;
use crate::services::MigrationServiceRequest;
use crate::session_provider::{
    SessionTurnIngestDriverRequest, SessionTurnIngestQuantumOutcome,
    run_session_turn_ingest_quantum_for_key,
};
use chrono::{DateTime, Utc};
use oulipoly_provider::client::CancellationToken;
use oulipoly_state::{
    SessionTurnIngestStream, SessionTurnIngestStreamKey, SessionTurnStreamProjection,
};
use std::time::{Duration, Instant};

// The existing worker bounds each provider page at 30 seconds. The barrier
// admits no new page after this deadline and never waits indefinitely for a lease.
const BARRIER_DEADLINE: Duration = Duration::from_secs(90);
const LEASE_OBSERVATION_INTERVAL: Duration = Duration::from_millis(100);

pub(super) fn settle_source_ingestion(
    registry_handle: &ProviderRegistryHandle,
    identity: &ExternalRotationIdentity,
    request: &MigrationServiceRequest<'_>,
) -> Result<(), ExternalRotationError> {
    if request
        .state
        .canonical_session_turn_ingest_freshness(
            &identity.source_provider,
            &identity.source_session_id,
        )
        .map_err(blocked)?
        .tracked_streams
        == 0
    {
        return Ok(());
    }
    let registry = registry_handle.current();
    let endpoint = registry
        .preflight_account(&identity.source_provider)
        .map_err(|error| blocked(error.to_string()))?;
    if !endpoint.capabilities().capabilities.session_turn_pages_v1 {
        return Err(blocked(
            "tracked source no longer advertises canonical paging",
        ));
    }
    let key = SessionTurnIngestStreamKey {
        provider_name: identity.source_provider.clone(),
        provider_instance_id: format!("{}-instance", endpoint.capabilities().provider_id),
        settings_id: endpoint
            .settings_id()
            .map_err(|error| blocked(error.to_string()))?
            .to_string(),
        session_id: identity.source_session_id.clone(),
        projection: SessionTurnStreamProjection::CanonicalIngest,
    };
    let started = Instant::now();
    let cancellation = CancellationToken::new();
    let lease_owner = format!(
        "rotation-source-ingest-{}-{}",
        std::process::id(),
        request.resolved.chain_id
    );
    loop {
        let Some(stream) = request
            .state
            .session_turn_ingest_stream(&key)
            .map_err(blocked)?
        else {
            return Err(blocked(
                "tracked source identity has no matching canonical stream",
            ));
        };
        let action = barrier_action(&stream, Utc::now()).map_err(blocked)?;
        if action == BarrierAction::Complete {
            if request
                .state
                .canonical_session_turn_ingest_freshness(
                    &identity.source_provider,
                    &identity.source_session_id,
                )
                .map_err(blocked)?
                .is_caught_up()
            {
                return Ok(());
            }
            return Err(blocked(
                "another tracked source identity still has unsettled ingestion",
            ));
        }
        let Some(remaining) = BARRIER_DEADLINE.checked_sub(started.elapsed()) else {
            return Err(blocked(
                "timed out waiting for the source canonical projection",
            ));
        };
        if action == BarrierAction::WaitForLease {
            std::thread::park_timeout(LEASE_OBSERVATION_INTERVAL.min(remaining));
            continue;
        }
        match run_session_turn_ingest_quantum_for_key(
            SessionTurnIngestDriverRequest {
                state: request.state,
                registry: registry.as_ref(),
                lease_owner: &lease_owner,
                effective_cwd: Some(request.effective_cwd),
                cancellation: &cancellation,
                now: Utc::now(),
            },
            &key,
        )
        .map_err(|error| blocked(error.to_string()))?
        {
            SessionTurnIngestQuantumOutcome::Applied { .. } => (),
            SessionTurnIngestQuantumOutcome::Idle => {
                // Another worker can acquire the source after our status read.
                // Observe its checkpoint/lease again rather than stealing it.
                std::thread::park_timeout(LEASE_OBSERVATION_INTERVAL.min(remaining));
            }
            SessionTurnIngestQuantumOutcome::RetryScheduled { error, .. }
            | SessionTurnIngestQuantumOutcome::Unsupported { error, .. }
            | SessionTurnIngestQuantumOutcome::Quarantined { error, .. } => {
                return Err(blocked(error));
            }
        }
    }
}

fn blocked(reason: impl std::fmt::Display) -> ExternalRotationError {
    error_formatter::host_apply_conflict(format!(
        "source canonical ingestion is not settled: {reason}"
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum BarrierAction {
    Complete,
    Advance,
    WaitForLease,
}

fn barrier_action(
    stream: &SessionTurnIngestStream,
    now: DateTime<Utc>,
) -> Result<BarrierAction, String> {
    match stream.status.as_str() {
        "caught_up" if stream.lease_owner.is_none() => Ok(BarrierAction::Complete),
        "ready" | "active" => {
            if stream.lease_owner.is_none() {
                return if stream.status == "ready" {
                    Ok(BarrierAction::Advance)
                } else {
                    Err("active source stream has no lease owner".into())
                };
            }
            let expiry = stream
                .lease_expires_at
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .ok_or_else(|| "source ingestion lease has no valid expiry".to_string())?;
            Ok(if expiry > now {
                BarrierAction::WaitForLease
            } else {
                BarrierAction::Advance
            })
        }
        status => Err(format!(
            "source stream status {status}: {}",
            stream
                .last_error
                .as_deref()
                .unwrap_or("no retry is performed during rotation")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stream(status: &str) -> SessionTurnIngestStream {
        SessionTurnIngestStream {
            key: SessionTurnIngestStreamKey {
                provider_name: "source".into(),
                provider_instance_id: "fixture-instance".into(),
                settings_id: "source-settings".into(),
                session_id: "session".into(),
                projection: SessionTurnStreamProjection::CanonicalIngest,
            },
            checkpoint_generation: 0,
            after_token: None,
            snapshot_id: None,
            next_page_token: None,
            expected_page_index: 0,
            expected_turn_sequence: 0,
            status: status.into(),
            committed_page_count: 0,
            committed_turn_count: 0,
            retry_count: 0,
            lease_owner: None,
            lease_expires_at: None,
            last_error: None,
        }
    }
    #[test]
    fn source_barrier_waits_for_active_worker_then_observes_caught_up() {
        let now = Utc::now();
        let mut source = stream("active");
        source.lease_owner = Some("background-worker".into());
        source.lease_expires_at = Some((now + chrono::Duration::seconds(75)).to_rfc3339());
        assert_eq!(
            barrier_action(&source, now).unwrap(),
            BarrierAction::WaitForLease
        );
        source.status = "caught_up".into();
        source.lease_owner = None;
        source.lease_expires_at = None;
        assert_eq!(
            barrier_action(&source, now).unwrap(),
            BarrierAction::Complete
        );
    }
    #[test]
    fn source_barrier_advances_ready_pages_and_reclaims_only_expired_leases() {
        let now = Utc::now();
        let mut source = stream("ready");
        assert_eq!(
            barrier_action(&source, now).unwrap(),
            BarrierAction::Advance
        );
        source.status = "active".into();
        source.lease_owner = Some("expired-worker".into());
        source.lease_expires_at = Some((now - chrono::Duration::seconds(1)).to_rfc3339());
        assert_eq!(
            barrier_action(&source, now).unwrap(),
            BarrierAction::Advance
        );
        source.lease_expires_at = None;
        assert!(barrier_action(&source, now).is_err());
    }
    #[test]
    fn source_barrier_preserves_failure_dispositions() {
        for status in ["retry_wait", "unsupported", "quarantined"] {
            let mut source = stream(status);
            source.last_error = Some("fixture-stop".into());
            let error = barrier_action(&source, Utc::now()).unwrap_err();
            assert!(error.contains(status));
            assert!(error.contains("fixture-stop"));
        }
    }
}
