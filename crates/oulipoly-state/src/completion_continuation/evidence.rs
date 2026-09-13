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

pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputArtifact {
    pub representation: String,
    pub relative: String,
    pub sha256: String,
    pub byte_len: u64,
    pub encoding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CompletionOutput {
    Inline(String),
    Artifact(OutputArtifact),
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
    pub output: CompletionOutput,
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
        let evidence = Self::parse_bytes(binding, snapshot_bytes, outcome_bytes)?;
        if matches!(evidence.snapshot.output, CompletionOutput::Artifact(_)) {
            return Err("artifact output requires actual full source verification".into());
        }
        Ok(evidence)
    }

    pub fn from_source_files(binding: &AdmittedSourceBinding) -> Result<Self, String> {
        let source = binding.registration()?;
        let directory = Path::new(&source.handle_dir);
        let snapshot = read_source_file(directory, &source.snapshot_relative, 16 * 1024 * 1024)?;
        let outcome =
            read_source_file(directory, &source.outcome_relative, MAX_REGISTRATION_BYTES)?;
        let evidence = Self::parse_bytes(binding, &snapshot, &outcome)?;
        if let CompletionOutput::Artifact(artifact) = &evidence.snapshot.output {
            artifact.copy_verified(directory, &mut std::io::sink())?;
        }
        Ok(evidence)
    }

    fn parse_bytes(
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
                exit_mode(&source, &outcome) && source.completion_scope == "root" && exit
            }
            "exit_tree" => {
                exit_mode(&source, &outcome)
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

fn exit_mode(source: &SourceRegistration, outcome: &SourceOutcome) -> bool {
    source.completion_kind == "exit"
        || (source.completion_kind == "ready" && outcome.ready_sentinel.is_none())
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

impl OutputArtifact {
    /// Complete raw body verification/transfer uses one fixed-size buffer, not
    /// a body-sized JSON/string allocation. Exact EOF and digest are required.
    pub fn copy_verified(
        &self,
        directory: &Path,
        output: &mut impl std::io::Write,
    ) -> Result<(), String> {
        use std::io::Read;
        if self.representation != "retained-output-v1"
            || self.relative != "completion-output-v2.bin"
            || self.encoding != "utf8-lossy"
            || !is_sha256(&self.sha256)
            || self.byte_len > MAX_OUTPUT_BYTES as u64
        {
            return Err("unsupported completion output artifact".into());
        }
        let mut input = open_source_file(directory, &self.relative, MAX_OUTPUT_BYTES)?;
        if input.metadata().map_err(|e| e.to_string())?.len() != self.byte_len {
            return Err("output artifact length conflict".into());
        }
        let mut hasher = Sha256::new();
        let mut bytes = [0; 65536];
        let mut total = 0u64;
        loop {
            let count = input.read(&mut bytes).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > self.byte_len {
                return Err("output artifact grew".into());
            }
            hasher.update(&bytes[..count]);
            output
                .write_all(&bytes[..count])
                .map_err(|e| e.to_string())?;
        }
        if total != self.byte_len || format!("{:x}", hasher.finalize()) != self.sha256 {
            return Err("output artifact digest/length conflict".into());
        }
        Ok(())
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
    fn ready_registration_accepts_genuine_early_exit_but_not_false_readiness_or_wait() {
        let (_, f) = fixture();
        for scope in ["root", "tree"] {
            for rc in [0, 37, 137] {
                let mut registration: serde_json::Value =
                    serde_json::from_str(f["registration_bytes_utf8"].as_str().unwrap()).unwrap();
                registration["completion_kind"] = "ready".into();
                registration["completion_scope"] = scope.into();
                let binding = AdmittedSourceBinding::new(
                    "test-admission",
                    &serde_json::to_vec(&registration).unwrap(),
                )
                .unwrap();
                let mut outcome: serde_json::Value =
                    serde_json::from_str(f["outcome_bytes_utf8"].as_str().unwrap()).unwrap();
                let mut snapshot: serde_json::Value =
                    serde_json::from_str(f["snapshot_bytes_utf8"].as_str().unwrap()).unwrap();
                outcome["registration_digest"] = binding.registration_digest().into();
                snapshot["registration_digest"] = binding.registration_digest().into();
                outcome["kind"] = format!("exit_{scope}").into();
                outcome["root_wait_status"] = if rc == 137 { 9 } else { rc << 8 }.into();
                outcome["original_tree_drained"] = (scope == "tree").into();
                snapshot["rc"] = rc.into();
                let check = |outcome: &serde_json::Value, mut snapshot: serde_json::Value| {
                    let bytes = serde_json::to_vec(outcome).unwrap();
                    snapshot["outcome_sha256"] = sha256(&bytes).into();
                    snapshot["outcome_byte_len"] = bytes.len().into();
                    VerifiedCompletion::from_bytes(
                        &binding,
                        &serde_json::to_vec(&snapshot).unwrap(),
                        &bytes,
                    )
                };
                check(&outcome, snapshot.clone()).unwrap();
                outcome["ready_sentinel"] = "NOT OBSERVED".into();
                assert!(check(&outcome, snapshot.clone()).is_err());
                outcome["ready_sentinel"] = serde_json::Value::Null;
                outcome["root_wait_status"] = 0x137f.into();
                assert!(check(&outcome, snapshot.clone()).is_err());
                outcome["root_wait_status"] = (rc << 8).into();
                outcome["output_closed"] = false.into();
                assert!(check(&outcome, snapshot).is_err());
            }
        }
    }

    #[test]
    fn full_artifact_over_inline_bound_streams_exact_bytes_and_rejects_changed_body() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let (_, f) = fixture();
        let mut registration: serde_json::Value =
            serde_json::from_str(f["registration_bytes_utf8"].as_str().unwrap()).unwrap();
        let handle = directory.path().join("ab_fixture");
        std::fs::create_dir(&handle).unwrap();
        registration["spool_root"] = directory.path().to_str().unwrap().into();
        registration["handle_dir"] = handle.to_str().unwrap().into();
        for field in ["helper", "recovery"] {
            registration[field]["path"] = handle.join(field).to_str().unwrap().into();
        }
        let binding = AdmittedSourceBinding::new(
            "test-admission",
            &serde_json::to_vec(&registration).unwrap(),
        )
        .unwrap();
        let mut output = std::fs::File::create(handle.join("completion-output-v2.bin")).unwrap();
        let chunk = [0xff; 65536]; // invalid UTF-8: raw bytes must not be silently changed
        let mut hasher = Sha256::new();
        for _ in 0..320 {
            output.write_all(&chunk).unwrap();
            hasher.update(chunk);
        }
        drop(output);
        let mut outcome: serde_json::Value =
            serde_json::from_str(f["outcome_bytes_utf8"].as_str().unwrap()).unwrap();
        let mut snapshot: serde_json::Value =
            serde_json::from_str(f["artifact_snapshot_bytes_utf8"].as_str().unwrap()).unwrap();
        outcome["registration_digest"] = binding.registration_digest().into();
        snapshot["registration_digest"] = binding.registration_digest().into();
        let outcome = serde_json::to_vec(&outcome).unwrap();
        snapshot["outcome_sha256"] = sha256(&outcome).into();
        snapshot["outcome_byte_len"] = outcome.len().into();
        snapshot["output"]["sha256"] = format!("{:x}", hasher.finalize()).into();
        snapshot["output"]["byte_len"] = (320 * chunk.len()).into();
        let snapshot = serde_json::to_vec(&snapshot).unwrap();
        assert!(snapshot.len() < 2048);
        println!(
            "artifact sample raw_bytes={} snapshot_bytes={} buffer_bytes={}",
            320 * chunk.len(),
            snapshot.len(),
            chunk.len()
        );
        assert!(VerifiedCompletion::from_bytes(&binding, &snapshot, &outcome).is_err());
        std::fs::write(handle.join("completion-snapshot-v2.json"), snapshot).unwrap();
        std::fs::write(handle.join("source-outcome-v2.json"), outcome).unwrap();
        let evidence = VerifiedCompletion::from_source_files(&binding).unwrap();
        let CompletionOutput::Artifact(artifact) = evidence.snapshot.output else {
            panic!("lost artifact");
        };
        assert_eq!(artifact.byte_len, 20 * 1024 * 1024);
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .open(handle.join("completion-output-v2.bin"))
            .unwrap();
        output.write_all(b"x").unwrap();
        assert!(VerifiedCompletion::from_source_files(&binding).is_err());
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
