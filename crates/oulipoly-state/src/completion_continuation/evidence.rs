use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionIdentity {
    pub protocol: String,
    pub domain_id: String,
    pub source_id: String,
    pub handle: String,
    pub registration_id: String,
    pub registration_digest: String,
}
impl AdmittedSourceBinding {
    pub fn identity(&self) -> Result<CompletionIdentity, String> {
        let source = self.registration()?;
        Ok(CompletionIdentity {
            protocol: source.protocol,
            domain_id: source.domain_id,
            source_id: source.source_id,
            handle: source.handle,
            registration_id: source.registration_id,
            registration_digest: self.registration_digest.clone(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceOutcome {
    #[serde(flatten)]
    pub identity: CompletionIdentity,
    pub completion_revision: u64,
    pub kind: String,
    pub observer: SourceProcessIdentity,
    pub root_wait_status: Option<i32>,
    pub output_closed: bool,
    pub original_tree_drained: bool,
    pub cancellation_id: Option<String>,
    pub launch_fence_revision: u64,
    pub ready_sentinel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionSnapshot {
    #[serde(flatten)]
    pub identity: CompletionIdentity,
    pub completion_revision: u64,
    pub outcome_sha256: String,
    pub outcome_byte_len: u64,
    pub rc: i32,
    pub status: String,
    pub output: String,
}

#[derive(Debug)]
pub struct VerifiedCompletion {
    pub snapshot: CompletionSnapshot,
    pub outcome: SourceOutcome,
    pub snapshot_sha256: String,
    pub outcome_sha256: String,
}

impl VerifiedCompletion {
    pub fn from_bytes(
        binding: &AdmittedSourceBinding,
        snapshot_bytes: &[u8],
        outcome_bytes: &[u8],
    ) -> Result<Self, String> {
        if snapshot_bytes.len() > 16 * 1024 * 1024 || outcome_bytes.len() > MAX_REGISTRATION_BYTES {
            return Err("completion evidence exceeds bounded length".into());
        }
        let snapshot: CompletionSnapshot =
            serde_json::from_slice(snapshot_bytes).map_err(|e| e.to_string())?;
        let outcome: SourceOutcome =
            serde_json::from_slice(outcome_bytes).map_err(|e| e.to_string())?;
        let identity = binding.identity()?;
        let source = binding.registration()?;
        let outcome_sha256 = sha256(outcome_bytes);
        if snapshot.identity != identity
            || outcome.identity != identity
            || snapshot.completion_revision == 0
            || snapshot.completion_revision != outcome.completion_revision
            || snapshot.outcome_sha256 != outcome_sha256
            || snapshot.outcome_byte_len != outcome_bytes.len() as u64
        {
            return Err("completion source/snapshot/outcome identity conflict".into());
        }
        let exit = outcome.root_wait_status.and_then(terminal_wait_rc) == Some(snapshot.rc)
            && outcome.output_closed;
        let valid = match outcome.kind.as_str() {
            "exit_root" => {
                source.completion_kind == "exit" && source.completion_scope == "root" && exit
            }
            "exit_tree" => {
                source.completion_kind == "exit"
                    && source.completion_scope == "tree"
                    && exit
                    && outcome.original_tree_drained
            }
            "cancelled" => {
                outcome
                    .cancellation_id
                    .as_ref()
                    .is_some_and(|id| !id.is_empty())
                    && outcome.output_closed
                    && outcome.original_tree_drained
            }
            "never_launched" => {
                outcome.launch_fence_revision > 0
                    && outcome.root_wait_status.is_none()
                    && outcome.output_closed
                    && outcome.original_tree_drained
                    && snapshot.rc != 0
            }
            "ceased_status_unknown" => {
                outcome.root_wait_status.is_none()
                    && outcome.output_closed
                    && outcome.original_tree_drained
                    && snapshot.rc != 0
            }
            "ready" => {
                source.completion_kind == "ready"
                    && outcome
                        .ready_sentinel
                        .as_ref()
                        .is_some_and(|s| !s.is_empty())
            }
            _ => false,
        };
        if !valid
            || outcome.observer.pid <= 0
            || outcome.observer.boot_id.is_empty()
            || outcome.observer.starttime_ticks < 0
        {
            return Err("completion outcome lacks required original-source evidence".into());
        }
        Ok(Self {
            snapshot,
            outcome,
            snapshot_sha256: sha256(snapshot_bytes),
            outcome_sha256,
        })
    }
}

/// Linux waitpid status, not a diagnostic shell rc or stopped/continued event.
fn terminal_wait_rc(status: i32) -> Option<i32> {
    if !(0..=65535).contains(&status) {
        return None;
    }
    let signal = status & 0x7f;
    if signal == 0 {
        Some((status >> 8) & 0xff)
    } else if signal < 0x7f {
        Some(128 + signal)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (AdmittedSourceBinding, serde_json::Value) {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/age360-paired-wire.json"))
                .unwrap();
        let binding = AdmittedSourceBinding::new(
            "test-admission",
            fixture["registration_bytes_utf8"]
                .as_str()
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        (binding, fixture)
    }
    #[test]
    fn canonical_source_evidence_is_accepted_and_diagnostic_rc_is_not_wait_status() {
        let (binding, f) = fixture();
        let snapshot = f["snapshot_bytes_utf8"].as_str().unwrap().as_bytes();
        let outcome = f["outcome_bytes_utf8"].as_str().unwrap().as_bytes();
        VerifiedCompletion::from_bytes(&binding, snapshot, outcome).unwrap();
        let mut changed: serde_json::Value = serde_json::from_slice(snapshot).unwrap();
        changed["rc"] = 70.into();
        assert!(
            VerifiedCompletion::from_bytes(
                &binding,
                &serde_json::to_vec(&changed).unwrap(),
                outcome
            )
            .is_err()
        );
    }
    #[test]
    fn stopped_or_continued_wait_status_does_not_prove_exit() {
        assert_eq!(terminal_wait_rc(0), Some(0));
        assert_eq!(terminal_wait_rc(70 << 8), Some(70));
        assert_eq!(terminal_wait_rc(9), Some(137));
        assert_eq!(terminal_wait_rc(0x137f), None);
        assert_eq!(terminal_wait_rc(0xffff), None);
        assert_eq!(terminal_wait_rc(-1), None);
    }
}
