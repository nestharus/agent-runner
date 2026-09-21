//! Maintained registry and progress-aware policy primitives for production
//! runtime caps.
//!
//! The registry is data, not control flow: call sites retain domain-specific
//! behavior while their stable ownership and classification remain queryable.
//! `runtime_cap_registry` tests bind each entry to its source declaration,
//! initializer, and a named production use scope. Every other numeric
//! declaration must carry an explicit, source-bound non-cap exclusion.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;

const REGISTRY_JSON: &str = include_str!("../../../runtime-caps.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCapClass {
    StructuralSafetyBound,
    ExternalProtocolDeadline,
    ResourceGuard,
    PollingCadence,
    TestOnlyPatience,
    ProvisionalStopgap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCapUse {
    pub source: String,
    pub scope: String,
    pub kind: RuntimeCapUseKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCapUseKind {
    /// The named declaration is referenced by the branch or API invocation
    /// that applies the cap.
    DirectControl,
    /// The declaration initializes typed runtime configuration. The checker
    /// proves this source edge, but deliberately does not claim whole-program
    /// dataflow through the configured field.
    ConfigurationSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCap {
    pub id: String,
    pub owner: String,
    pub class: RuntimeCapClass,
    pub protected_resource: String,
    pub exhaustion_behavior: String,
    pub observability: String,
    pub configurability: String,
    pub rationale: String,
    pub default_value: String,
    pub source: String,
    pub declaration_scope: String,
    pub symbol: String,
    pub controlling_use: RuntimeCapUse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryDocument {
    schema_version: u32,
    caps: Vec<RuntimeCap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValidationError(pub String);

impl std::fmt::Display for RegistryValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for RegistryValidationError {}

/// Returns the checked, stable registry used by operator queries and tests.
pub fn registry() -> &'static [RuntimeCap] {
    static REGISTRY: OnceLock<Vec<RuntimeCap>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let mut document: RegistryDocument = serde_json::from_str(REGISTRY_JSON)
                .expect("embedded runtime-cap registry must be valid JSON");
            assert_eq!(
                document.schema_version, 2,
                "unsupported cap registry schema"
            );
            validate_registry(&document.caps).expect("embedded runtime-cap registry is invalid");
            document.caps.sort_by(|left, right| left.id.cmp(&right.id));
            document.caps
        })
        .as_slice()
}

pub fn find(id: &str) -> Option<&'static RuntimeCap> {
    registry().iter().find(|cap| cap.id == id)
}

/// Operator-readable stable JSON. This deliberately returns registry data only;
/// AGE-378 may attach longitudinal measurements later.
pub fn registry_json() -> &'static str {
    REGISTRY_JSON
}

pub fn validate_registry(caps: &[RuntimeCap]) -> Result<(), RegistryValidationError> {
    let mut ids = std::collections::BTreeSet::new();
    let mut sites = std::collections::BTreeSet::new();
    for cap in caps {
        if !valid_stable_id(&cap.id) {
            return Err(RegistryValidationError(format!(
                "invalid stable cap id: {:?}",
                cap.id
            )));
        }
        if !ids.insert(cap.id.as_str()) {
            return Err(RegistryValidationError(format!(
                "duplicate cap id: {}",
                cap.id
            )));
        }
        for (field, value) in [
            ("owner", cap.owner.as_str()),
            ("protected_resource", cap.protected_resource.as_str()),
            ("exhaustion_behavior", cap.exhaustion_behavior.as_str()),
            ("observability", cap.observability.as_str()),
            ("configurability", cap.configurability.as_str()),
            ("rationale", cap.rationale.as_str()),
            ("default_value", cap.default_value.as_str()),
            ("source", cap.source.as_str()),
            ("declaration_scope", cap.declaration_scope.as_str()),
            ("symbol", cap.symbol.as_str()),
            (
                "controlling_use.source",
                cap.controlling_use.source.as_str(),
            ),
            ("controlling_use.scope", cap.controlling_use.scope.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(RegistryValidationError(format!(
                    "cap {} has empty {field}",
                    cap.id
                )));
            }
        }
        if cap.source.starts_with('/') || cap.source.contains("..") {
            return Err(RegistryValidationError(format!(
                "cap {} source must be workspace-relative: {}",
                cap.id, cap.source
            )));
        }
        if cap.controlling_use.source.starts_with('/') || cap.controlling_use.source.contains("..")
        {
            return Err(RegistryValidationError(format!(
                "cap {} controlling-use source must be workspace-relative: {}",
                cap.id, cap.controlling_use.source
            )));
        }
        for (field, value) in [
            ("rationale", cap.rationale.as_str()),
            ("default_value", cap.default_value.as_str()),
            ("protected_resource", cap.protected_resource.as_str()),
            ("exhaustion_behavior", cap.exhaustion_behavior.as_str()),
            ("observability", cap.observability.as_str()),
            ("configurability", cap.configurability.as_str()),
        ] {
            if value.contains("cannot be changed without registry review")
                || value.starts_with("source expression ")
                || value == "bounded traversal, retry, or state-machine invariant"
                || value == "bounded memory or payload retention"
                || value == "bounded process resources and work queues"
                || value == "scheduler and observation cadence"
                || value == "external peer or protocol liveness"
                || value
                    == "typed error/result or bounded owning-operation diagnostic before terminal/degraded action"
                || value
                    == "fixed source default unless the owning API or documented environment variable supplies an override"
                || value == "PENDING"
                || value.contains("initializer and production reference are registry-checked")
                || value.contains("rejects, truncates, or")
                || value.contains("typed result, bounded output, or")
                || value.contains("takes its existing bounded rejection")
                || value.contains("expires, reclaims, suppresses, or")
                || value.contains("existing typed")
                || value.contains("source-defined")
                || value.contains("keeps one operation finite")
                || value.contains("prevents the caller and worker slot")
                || value.contains("typed result/remaining-work")
                || value.contains("bounded item/traversal")
                || value.contains("in-memory queue occupancy")
            {
                return Err(RegistryValidationError(format!(
                    "cap {} retains non-specific generated {field}: {value}",
                    cap.id
                )));
            }
        }
        let site = (
            cap.source.as_str(),
            cap.declaration_scope.as_str(),
            cap.symbol.as_str(),
        );
        if !sites.insert(site) {
            return Err(RegistryValidationError(format!(
                "duplicate cap site: {}#{}#{}",
                cap.source, cap.declaration_scope, cap.symbol
            )));
        }
    }
    Ok(())
}

fn valid_stable_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
        && !id.starts_with(['.', '-', '_'])
        && !id.ends_with(['.', '-', '_'])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLiveness {
    Alive,
    Exited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressWaitOutcome {
    Pending,
    Complete,
    Cancelled,
    PeerExited,
    Stalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressWaitDecision {
    pub outcome: ProgressWaitOutcome,
    pub poll_after: Option<Duration>,
}

/// Pure, clock-injected state machine that separates poll cadence from the
/// terminal stale-progress policy. Only a changed progress token renews the
/// stale deadline; arbitrary loop activity does not.
#[derive(Debug, Clone)]
pub struct ProgressWait {
    stale_after: Duration,
    poll_interval: Duration,
    last_progress: u64,
    last_progress_at: Duration,
}

impl ProgressWait {
    pub fn new(
        stale_after: Duration,
        poll_interval: Duration,
        now: Duration,
        initial_progress: u64,
    ) -> Self {
        assert!(!poll_interval.is_zero(), "poll cadence must be non-zero");
        Self {
            stale_after,
            poll_interval,
            last_progress: initial_progress,
            last_progress_at: now,
        }
    }

    pub fn observe(
        &mut self,
        now: Duration,
        progress: u64,
        cancelled: bool,
        peer: PeerLiveness,
        complete: bool,
    ) -> ProgressWaitDecision {
        if cancelled {
            return decision(ProgressWaitOutcome::Cancelled, None);
        }
        if complete {
            return decision(ProgressWaitOutcome::Complete, None);
        }
        if peer == PeerLiveness::Exited {
            return decision(ProgressWaitOutcome::PeerExited, None);
        }
        if progress != self.last_progress {
            self.last_progress = progress;
            self.last_progress_at = now;
        }
        let idle = now.saturating_sub(self.last_progress_at);
        if idle >= self.stale_after {
            return decision(ProgressWaitOutcome::Stalled, None);
        }
        decision(
            ProgressWaitOutcome::Pending,
            Some(self.poll_interval.min(self.stale_after - idle)),
        )
    }

    pub fn stale_after(&self) -> Duration {
        self.stale_after
    }

    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }
}

fn decision(outcome: ProgressWaitOutcome, poll_after: Option<Duration>) -> ProgressWaitDecision {
    ProgressWaitDecision {
        outcome,
        poll_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: u64) -> Duration {
        Duration::from_secs(value)
    }

    #[test]
    fn defined_progress_renews_liveness_beyond_old_wall_clock() {
        let mut wait = ProgressWait::new(seconds(5), Duration::from_millis(25), Duration::ZERO, 0);
        for second in 1..=12 {
            let decision = wait.observe(seconds(second), second, false, PeerLiveness::Alive, false);
            assert_eq!(decision.outcome, ProgressWaitOutcome::Pending);
        }
        assert_eq!(
            wait.observe(seconds(13), 12, false, PeerLiveness::Alive, true)
                .outcome,
            ProgressWaitOutcome::Complete
        );
    }

    #[test]
    fn cancellation_is_authoritative_even_while_progressing() {
        let mut wait = ProgressWait::new(seconds(5), Duration::from_millis(25), Duration::ZERO, 0);
        assert_eq!(
            wait.observe(seconds(6), 1, true, PeerLiveness::Alive, true)
                .outcome,
            ProgressWaitOutcome::Cancelled
        );
    }

    #[test]
    fn dead_peer_and_stale_progress_are_terminal() {
        let mut dead = ProgressWait::new(seconds(5), Duration::from_millis(25), Duration::ZERO, 0);
        assert_eq!(
            dead.observe(seconds(1), 0, false, PeerLiveness::Exited, false)
                .outcome,
            ProgressWaitOutcome::PeerExited
        );
        let mut stalled =
            ProgressWait::new(seconds(5), Duration::from_millis(25), Duration::ZERO, 0);
        assert_eq!(
            stalled
                .observe(seconds(5), 0, false, PeerLiveness::Alive, false)
                .outcome,
            ProgressWaitOutcome::Stalled
        );
    }

    #[test]
    fn zero_stale_progress_policy_is_an_immediate_typed_stall() {
        let mut wait =
            ProgressWait::new(Duration::ZERO, Duration::from_millis(25), Duration::ZERO, 0);
        assert_eq!(
            wait.observe(Duration::ZERO, 0, false, PeerLiveness::Alive, false)
                .outcome,
            ProgressWaitOutcome::Stalled
        );
    }

    #[test]
    fn poll_cadence_is_bounded_but_never_a_terminal_deadline() {
        let cadence = Duration::from_millis(25);
        let mut wait = ProgressWait::new(seconds(5), cadence, Duration::ZERO, 0);
        for tick in 1..100 {
            let now = Duration::from_millis(tick * 10);
            let decision = wait.observe(now, tick, false, PeerLiveness::Alive, false);
            assert_eq!(decision.outcome, ProgressWaitOutcome::Pending);
            assert!(decision.poll_after.is_some_and(|delay| delay <= cadence));
        }
    }
}
