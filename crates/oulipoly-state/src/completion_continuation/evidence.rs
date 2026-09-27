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

const OUTPUT_COPY_BUFFER_BYTES: usize = 64 * 1024;

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
    Missing(MissingOriginalOutput),
}

/// Attributable producer evidence about unavailable original output, not a
/// synthetic terminal outcome. This proof travels inside the immutable snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissingOriginalOutput {
    pub representation: String,
    pub capture_state: String,
    pub reason: String,
    pub producer: SourceProcessIdentity,
    pub original_observer: SourceProcessIdentity,
    pub completion_revision: u64,
    pub outcome_sha256: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub selection: Option<OriginalOutputSelection>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub observed_byte_len: Option<u64>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalOutputSelection {
    pub device: u64,
    pub inode: u64,
    pub byte_len: u64,
}

impl MissingOriginalOutput {
    fn validate(&self, outcome: &SourceOutcome, digest: &str) -> Result<(), String> {
        let selection_valid = self
            .selection
            .as_ref()
            .is_some_and(|selection| selection.inode > 0);
        let reason_valid = match self.reason.as_str() {
            "original_selection_not_retained" => {
                self.selection.is_none() && self.observed_byte_len.is_none()
            }
            "selected_storage_lost" => selection_valid && self.observed_byte_len.is_none(),
            "selected_storage_short" => {
                selection_valid
                    && self.selection.as_ref().is_some_and(|s| {
                        self.observed_byte_len
                            .is_some_and(|length| length < s.byte_len)
                    })
            }
            _ => false,
        };
        if self.representation != "missing-original-output-v1"
            || self.capture_state != "irrecoverable"
            || !reason_valid
            || self.producer.pid <= 0
            || self.producer.boot_id.is_empty()
            || self.producer.starttime_ticks < 0
            || self.original_observer != outcome.observer
            || self.completion_revision != outcome.completion_revision
            || self.outcome_sha256 != digest
            || self.detail.trim().is_empty()
            || self.detail.len() > 4096
        {
            return Err(
                "missing original output lacks attributable permanent-loss evidence".into(),
            );
        }
        Ok(())
    }
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
    pub fn original_output_missing(&self) -> bool {
        matches!(self.snapshot.output, CompletionOutput::Missing(_))
    }

    /// The missing-output recipient payload must actually carry the validated
    /// proof, not turn a valid source proof into an empty successful notification.
    pub(crate) fn validate_missing_payload(&self, payload: &str, rc: i32) -> Result<(), String> {
        if !self.original_output_missing() {
            return Ok(());
        }
        let value: serde_json::Value = serde_json::from_str(payload).map_err(|e| e.to_string())?;
        if value["kind"] != "agent_bash_complete"
            || value["snapshot"]
                != serde_json::to_value(&self.snapshot).map_err(|e| e.to_string())?
            || value["outcome"] != serde_json::to_value(&self.outcome).map_err(|e| e.to_string())?
            || value["rc"] != self.snapshot.rc
            || rc != self.snapshot.rc
            || value.get("output_artifact") != Some(&serde_json::Value::Null)
        {
            return Err("missing-output payload conflicts with validated original evidence".into());
        }
        Ok(())
    }

    /// A helper reply is diagnostic until public evidence and exact hashes agree.
    pub fn validate_source_reply(&self, reply: &serde_json::Value) -> Result<(), String> {
        let expected_status = if self.original_output_missing() {
            "source_output_missing"
        } else {
            "source_ready"
        };
        let identity = serde_json::to_value(&self.snapshot.identity).map_err(|e| e.to_string())?;
        for (key, expected) in identity.as_object().ok_or("invalid source identity")? {
            if reply.get(key) != Some(expected) {
                return Err(format!("source reply identity conflict at {key}"));
            }
        }
        if reply["status"] != expected_status
            || reply["snapshot_sha256"] != self.snapshot_sha256
            || reply["outcome_sha256"] != self.outcome_sha256
        {
            return Err("source reply evidence/status conflict".into());
        }
        Ok(())
    }

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
        if let CompletionOutput::Missing(missing) = &snapshot.output {
            missing.validate(&outcome, &outcome_sha256)?;
            if snapshot.status != "original_output_unavailable" {
                return Err("missing output requires explicit unavailable status".into());
            }
        } else if snapshot.status == "original_output_unavailable" {
            return Err("unavailable status requires missing output evidence".into());
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
        if self.representation != "retained-output-v1"
            || self.relative != "completion-output-v2.bin"
            || !matches!(self.encoding.as_str(), "raw" | "utf8-lossy")
            || !is_sha256(&self.sha256)
        {
            return Err("unsupported completion output artifact".into());
        }
        let mut input = open_source_output(directory, &self.relative)?;
        let before = copy_verified_raw(&mut input, self.byte_len, &self.sha256, output)?;
        let named = open_source_output(directory, &self.relative)?;
        require_unchanged_output(&before, &named.metadata().map_err(|e| e.to_string())?)?;
        Ok(())
    }
}

/// Verify exact regular-file bytes using fixed memory. Read/write errors and
/// observed length, inode or mutation conflicts cannot produce a receipt.
/// Callers must stage writes and recheck the named inode before publication.
pub fn copy_verified_raw(
    input: &mut std::fs::File,
    expected_len: u64,
    expected_digest: &str,
    output: &mut impl std::io::Write,
) -> Result<std::fs::Metadata, String> {
    use std::io::Read;
    let before = input
        .metadata()
        .map_err(|e| format!("output metadata: {e}"))?;
    if !before.is_file() || before.len() != expected_len {
        return Err("output artifact length/type conflict".into());
    }
    let mut hasher = Sha256::new();
    let mut bytes = [0; OUTPUT_COPY_BUFFER_BYTES];
    let mut total = 0u64;
    loop {
        let count = match input.read(&mut bytes) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.map_err(|e| format!("output artifact read failed: {e}"))?,
        };
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or("output byte count overflow")?;
        if total > expected_len {
            return Err("output artifact grew during read".into());
        }
        hasher.update(&bytes[..count]);
        output
            .write_all(&bytes[..count])
            .map_err(|e| format!("output artifact write failed: {e}"))?;
    }
    if total != expected_len {
        return Err("output artifact short read".into());
    }
    if format!("{:x}", hasher.finalize()) != expected_digest {
        return Err("output artifact digest conflict".into());
    }
    require_unchanged_output(&before, &input.metadata().map_err(|e| e.to_string())?)?;
    Ok(before)
}

/// Check inode, length and filesystem-observed change times during a read,
/// and re-use this check on the freshly opened pathname. Same-UID writes
/// hidden by timestamp resolution are outside this observation guarantee.
pub fn require_unchanged_output(
    before: &std::fs::Metadata,
    after: &std::fs::Metadata,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.is_file()
            && after.is_file()
            && (
                before.dev(),
                before.ino(),
                before.len(),
                before.nlink(),
                before.mtime(),
                before.mtime_nsec(),
                before.ctime(),
                before.ctime_nsec(),
            ) == (
                after.dev(),
                after.ino(),
                after.len(),
                after.nlink(),
                after.mtime(),
                after.mtime_nsec(),
                after.ctime(),
                after.ctime_nsec(),
            )
        {
            return Ok(());
        }
    }
    Err("output artifact inode/length/change-time conflict".into())
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
    fn missing_output_wire_requires_permanent_attributable_exact_original_evidence() {
        let (binding, _) = fixture();
        let f: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/age360-missing-output-wire.json"
        ))
        .unwrap();
        let original: serde_json::Value =
            serde_json::from_str(f["missing_output_snapshot_bytes_utf8"].as_str().unwrap())
                .unwrap();
        let outcome = f["outcome_bytes_utf8"].as_str().unwrap().as_bytes();
        let check = |snapshot: &serde_json::Value| {
            VerifiedCompletion::from_bytes(
                &binding,
                &serde_json::to_vec(snapshot).unwrap(),
                outcome,
            )
        };
        let exact = VerifiedCompletion::from_bytes(
            &binding,
            f["missing_output_snapshot_bytes_utf8"]
                .as_str()
                .unwrap()
                .as_bytes(),
            outcome,
        )
        .unwrap();
        assert!(exact.original_output_missing());
        assert_eq!(exact.outcome.root_wait_status, Some(0));
        assert_eq!(exact.snapshot.rc, 0); // absent output does not invent workload failure
        exact
            .validate_source_reply(&f["missing_output_recovery_response"])
            .unwrap();
        for (pointer, value) in [
            ("/output/capture_state", serde_json::json!("pending")),
            ("/output/reason", serde_json::json!("read_error")),
            ("/output/producer/pid", serde_json::json!(0)),
            (
                "/output/original_observer/starttime_ticks",
                serde_json::json!(1),
            ),
            ("/output/completion_revision", serde_json::json!(2)),
            ("/output/outcome_sha256", serde_json::json!("a".repeat(64))),
            ("/output/detail", serde_json::json!(" ")),
            ("/registration_id", serde_json::json!("wrong")),
            ("/status", serde_json::json!("completed")),
        ] {
            let mut bad = original.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(check(&bad).is_err(), "accepted {pointer}");
        }
        for key in [
            "selection",
            "observed_byte_len",
            "producer",
            "capture_state",
        ] {
            let mut bad = original.clone();
            bad["output"].as_object_mut().unwrap().remove(key);
            assert!(check(&bad).is_err(), "accepted missing {key}");
        }
        let mut lost = original.clone();
        lost["output"]["reason"] = "selected_storage_lost".into();
        assert!(check(&lost).is_err());
        lost["output"]["selection"] = serde_json::json!({"device":1,"inode":2,"byte_len":42});
        check(&lost).unwrap();
        lost["output"]["selection"]["byte_len"] = serde_json::json!(u64::MAX);
        check(&lost).unwrap(); // missing selected storage has no positive output ceiling
        lost["output"]["selection"]["byte_len"] = 42.into();
        lost["output"]["reason"] = "selected_storage_short".into();
        assert!(check(&lost).is_err());
        lost["output"]["observed_byte_len"] = 42.into();
        assert!(check(&lost).is_err());
        lost["output"]["observed_byte_len"] = 41.into();
        check(&lost).unwrap();
        for status in ["pending", "unavailable", "source_ready"] {
            let mut reply = f["missing_output_recovery_response"].clone();
            reply["status"] = status.into();
            assert!(exact.validate_source_reply(&reply).is_err());
        }
        let successful = VerifiedCompletion::from_bytes(
            &binding,
            f["snapshot_bytes_utf8"].as_str().unwrap().as_bytes(),
            outcome,
        )
        .unwrap();
        assert!(!successful.original_output_missing());
        assert!(
            successful
                .validate_source_reply(&f["missing_output_recovery_response"])
                .is_err()
        );
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
    #[ignore = "streams 1 GiB + 1 byte; explicit raw-output regression control"]
    fn artifact_above_old_cap_passes_state_source_verification() {
        let directory = tempfile::tempdir().unwrap();
        let handle = directory.path().join("ab_fixture");
        std::fs::create_dir(&handle).unwrap();
        let (_, fixture) = fixture();
        let mut registration: serde_json::Value =
            serde_json::from_str(fixture["registration_bytes_utf8"].as_str().unwrap()).unwrap();
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
        let length = 1024 * 1024 * 1024 + 1u64;
        let digest = "6d9bfe50425f2dfe4e2ac07efee1f0bc9d567348ad4aed62704ffe6f5884e9a8";
        std::fs::File::create(handle.join("completion-output-v2.bin"))
            .unwrap()
            .set_len(length)
            .unwrap();
        let mut outcome: serde_json::Value =
            serde_json::from_str(fixture["outcome_bytes_utf8"].as_str().unwrap()).unwrap();
        let mut snapshot: serde_json::Value =
            serde_json::from_str(fixture["artifact_snapshot_bytes_utf8"].as_str().unwrap())
                .unwrap();
        outcome["registration_digest"] = binding.registration_digest().into();
        snapshot["registration_digest"] = binding.registration_digest().into();
        let outcome = serde_json::to_vec(&outcome).unwrap();
        snapshot["outcome_sha256"] = sha256(&outcome).into();
        snapshot["outcome_byte_len"] = outcome.len().into();
        snapshot["output"]["byte_len"] = length.into();
        snapshot["output"]["sha256"] = digest.into();
        let snapshot = serde_json::to_vec(&snapshot).unwrap();
        assert!(snapshot.len() < 2048);
        assert!(VerifiedCompletion::from_bytes(&binding, &snapshot, &outcome).is_err());
        std::fs::write(handle.join("completion-snapshot-v2.json"), &snapshot).unwrap();
        std::fs::write(handle.join("source-outcome-v2.json"), outcome).unwrap();
        let verified = VerifiedCompletion::from_source_files(&binding).unwrap();
        let CompletionOutput::Artifact(artifact) = verified.snapshot.output else {
            panic!("raw artifact absent")
        };
        assert_eq!(artifact.byte_len, length);
        assert_eq!(artifact.sha256, digest);
        println!(
            "State raw bytes={length} snapshot bytes={} sha256={digest}",
            snapshot.len()
        );
    }

    #[cfg(unix)]
    #[test]
    fn raw_copy_rejects_short_growth_replacement_and_same_length_writes() {
        use std::io::Write;
        struct Change<F: FnMut()>(Option<F>);
        impl<F: FnMut()> Write for Change<F> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if let Some(mut change) = self.0.take() {
                    change();
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for change in ["short", "grow", "replace", "same-length"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("completion-output-v2.bin");
            let bytes = vec![0xff; OUTPUT_COPY_BUFFER_BYTES * 2];
            std::fs::write(&path, &bytes).unwrap();
            // Make a real subsequent write observably change mtime without a
            // sleep or reliance on two writes landing in different clock ticks.
            std::fs::File::open(&path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH)
                .unwrap();
            let artifact = OutputArtifact {
                representation: "retained-output-v1".into(),
                relative: "completion-output-v2.bin".into(),
                sha256: sha256(&bytes),
                byte_len: bytes.len() as u64,
                encoding: "raw".into(),
            };
            let mut writer = Change(Some(|| {
                match change {
                    "short" => std::fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_len(1)
                        .unwrap(),
                    "grow" => std::fs::OpenOptions::new()
                        .append(true)
                        .open(&path)
                        .unwrap()
                        .write_all(b"x")
                        .unwrap(),
                    "replace" => {
                        let replacement = directory.path().join("replacement");
                        std::fs::write(&replacement, &bytes).unwrap();
                        std::fs::rename(replacement, &path).unwrap();
                    }
                    // Restore already-read bytes: digest alone cannot detect this.
                    _ => {
                        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                        file.write_all(b"x").unwrap();
                        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0)).unwrap();
                        file.write_all(&bytes[..1]).unwrap();
                    }
                }
            }));
            let error = artifact
                .copy_verified(directory.path(), &mut writer)
                .unwrap_err();
            println!("{change}: {error}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn raw_copy_disk_full_is_an_error_not_a_partial_receipt() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("completion-output-v2.bin"), b"body").unwrap();
        let artifact = OutputArtifact {
            representation: "retained-output-v1".into(),
            relative: "completion-output-v2.bin".into(),
            sha256: sha256(b"body"),
            byte_len: 4,
            encoding: "raw".into(),
        };
        let mut full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let error = artifact
            .copy_verified(directory.path(), &mut full)
            .unwrap_err();
        assert!(
            error.contains("write failed") && error.contains("os error 28"),
            "{error}"
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
