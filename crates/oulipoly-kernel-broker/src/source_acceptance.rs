//! Broker-owned capture of exact v2 source evidence. Capture is durable debt,
//! not source release, notification, recipient ACK, or native work authority.
use crate::source_physical::{CapturedOutput, SourceObservation, SourcePhysicalRegistry};
use oulipoly_state::completion_continuation::{
    CompletionOutput, MAX_OUTPUT_BYTES, MAX_REGISTRATION_BYTES, VerifiedCompletion,
    open_source_file, sha256,
};
use oulipoly_state::mailbox::{BrokerSidecar, BrokerSourceEffectGrant, BrokerSourceEvidenceSeal};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const MAX_SNAPSHOT: usize = 16 * 1024 * 1024;
const MAX_REPLY: usize = MAX_REGISTRATION_BYTES * 2;
// Four protocol-bounded JSON byte strings total at most 20 MiB; base64 and
// metadata fit here without imposing a second arbitrary source-output cap.
const MAX_MANIFEST: u64 = 32 * 1024 * 1024;

mod base64_vec {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(encoded).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SourceV2Candidate {
    pub grant_id: String,
    pub registration_id: String,
    pub snapshot_sha256: String,
    pub outcome_sha256: String,
    pub recovery_stdout_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub byte_len: u64,
    pub sha256: String,
}

fn identity(file: &File, digest: String, byte_len: u64) -> Result<FileIdentity, String> {
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.nlink() != 1 || meta.len() != byte_len {
        return Err("evidence file changed during read".into());
    }
    Ok(FileIdentity {
        device: meta.dev(),
        inode: meta.ino(),
        byte_len,
        sha256: digest,
    })
}

fn source_bytes(dir: &Path, name: &str, limit: usize) -> Result<(Vec<u8>, FileIdentity), String> {
    let mut file = open_source_file(dir, name, limit)?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("source evidence exceeds protocol bound".into());
    }
    let stamp = identity(&file, sha256(&bytes), bytes.len() as u64)?;
    let named = open_source_file(dir, name, limit)?
        .metadata()
        .map_err(|e| e.to_string())?;
    if (before.dev(), before.ino()) != (stamp.device, stamp.inode)
        || (named.dev(), named.ino(), named.len()) != (stamp.device, stamp.inode, stamp.byte_len)
    {
        return Err("source evidence name changed".into());
    }
    Ok((bytes, stamp))
}

fn artifact_stamp(file: &File, expected: &str, len: u64) -> Result<FileIdentity, String> {
    use sha2::{Digest, Sha256};
    let mut file = file.try_clone().map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > len {
            return Err("source artifact grew".into());
        }
        hash.update(&buffer[..n]);
    }
    let digest = format!("{:x}", hash.finalize());
    if size != len || digest != expected {
        return Err("source artifact hash/length conflict".into());
    }
    identity(&file, digest, size)
}

fn owned_artifact(path: &Path, expected: &str, len: u64) -> Result<FileIdentity, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    let named = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.uid() != 0
        || meta.mode() & 0o077 != 0
        || named.file_type().is_symlink()
        || (meta.dev(), meta.ino()) != (named.dev(), named.ino())
    {
        return Err("owned artifact file identity changed".into());
    }
    artifact_stamp(&file, expected, len)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Evidence {
    version: u32,
    grant: BrokerSourceEffectGrant,
    #[serde(with = "base64_vec")]
    registration: Vec<u8>,
    registration_file: FileIdentity,
    #[serde(with = "base64_vec")]
    snapshot: Vec<u8>,
    snapshot_file: FileIdentity,
    #[serde(with = "base64_vec")]
    outcome: Vec<u8>,
    outcome_file: FileIdentity,
    #[serde(with = "base64_vec")]
    reply: Vec<u8>,
    record_sha256: String,
    terminal_sha256: String,
    stdout: CapturedOutput,
    stderr: CapturedOutput,
    artifact_original: Option<FileIdentity>,
    artifact_owned: Option<FileIdentity>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CapturedSourceEvidence {
    pub candidate: SourceV2Candidate,
    pub manifest: FileIdentity,
}

fn record<'a>(
    physical: &'a SourcePhysicalRegistry,
    id: &str,
) -> Result<&'a crate::source_physical::SourcePhysicalRecord, String> {
    physical
        .records()
        .iter()
        .find(|r| r.grant.grant_id == id)
        .ok_or("source physical record absent".into())
}

fn drained(
    physical: &SourcePhysicalRegistry,
    id: &str,
) -> Result<(CapturedOutput, CapturedOutput), String> {
    match physical.observe(id).map_err(|e| e.to_string())? {
        SourceObservation::Drained {
            worker_wait_status: 0,
            stdout,
            stderr,
            cancel_requested: false,
        } => Ok((stdout, stderr)),
        _ => Err("source recovery has no successful uncancelled physical drain".into()),
    }
}

fn reply(
    physical: &SourcePhysicalRegistry,
    id: &str,
    stdout: &CapturedOutput,
) -> Result<Vec<u8>, String> {
    let mut file = physical
        .open_drained_stdout(id)
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_REPLY as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_REPLY
        || bytes.len() as u64 != stdout.byte_len
        || sha256(&bytes) != stdout.sha256
    {
        return Err("recovery reply is incomplete or exceeds v2 reply bound".into());
    }
    Ok(bytes)
}

/// Read-only diagnostic retained for the pre-capture fixture. Original files
/// remain same-UID mutable after this returns.
pub fn assess_v2_candidate(
    sidecar: &BrokerSidecar,
    physical: &SourcePhysicalRegistry,
    id: &str,
) -> Result<SourceV2Candidate, String> {
    let held = record(physical, id)?;
    let (stdout, stderr) = drained(physical, id)?;
    let binding = sidecar.read_consumed_source_candidate(&held.grant)?;
    let source = binding.registration()?;
    let (registration, _) = source_bytes(
        Path::new(&source.handle_dir),
        &source.registration_relative,
        MAX_REGISTRATION_BYTES,
    )?;
    if registration != binding.registration_bytes() {
        return Err("original registration differs from State admission".into());
    }
    let verified = VerifiedCompletion::from_source_files(&binding)?;
    verified.validate_source_reply(
        &serde_json::from_slice(&reply(physical, id, &stdout)?).map_err(|e| e.to_string())?,
    )?;
    if sidecar.read_consumed_source_candidate(&held.grant)? != binding
        || drained(physical, id)? != (stdout.clone(), stderr)
    {
        return Err("State or physical source changed during assessment".into());
    }
    Ok(SourceV2Candidate {
        grant_id: id.into(),
        registration_id: source.registration_id,
        snapshot_sha256: verified.snapshot_sha256,
        outcome_sha256: verified.outcome_sha256,
        recovery_stdout_sha256: stdout.sha256,
    })
}

/// Create-new names and fsync make any partial capture non-replayable debt.
/// The artifact is streamed, with no arbitrary positive-receipt size cutoff.
pub fn capture_v2_evidence(
    sidecar: &BrokerSidecar,
    physical: &SourcePhysicalRegistry,
    id: &str,
) -> Result<CapturedSourceEvidence, String> {
    let candidate = assess_v2_candidate(sidecar, physical, id)?;
    let held = record(physical, id)?;
    let binding = sidecar.read_consumed_source_candidate(&held.grant)?;
    let source = binding.registration()?;
    let dir = Path::new(&source.handle_dir);
    let (registration, registration_file) =
        source_bytes(dir, &source.registration_relative, MAX_REGISTRATION_BYTES)?;
    let (snapshot, snapshot_file) = source_bytes(dir, &source.snapshot_relative, MAX_SNAPSHOT)?;
    let (outcome, outcome_file) =
        source_bytes(dir, &source.outcome_relative, MAX_REGISTRATION_BYTES)?;
    let verified = VerifiedCompletion::from_source_files(&binding)?;
    if registration != binding.registration_bytes()
        || sha256(&snapshot) != candidate.snapshot_sha256
        || sha256(&outcome) != candidate.outcome_sha256
        || verified.snapshot_sha256 != candidate.snapshot_sha256
        || verified.outcome_sha256 != candidate.outcome_sha256
    {
        return Err("source evidence mixed bytes during capture".into());
    }
    let (stdout, stderr) = drained(physical, id)?;
    let reply = reply(physical, id, &stdout)?;
    verified.validate_source_reply(&serde_json::from_slice(&reply).map_err(|e| e.to_string())?)?;
    let (artifact_original, artifact_owned) =
        if let CompletionOutput::Artifact(a) = &verified.snapshot.output {
            let original = artifact_stamp(
                &open_source_file(dir, &a.relative, MAX_OUTPUT_BYTES)?,
                &a.sha256,
                a.byte_len,
            )?;
            let path = physical
                .evidence_path(id, "evidence-artifact")
                .map_err(|e| e.to_string())?;
            let mut owned = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&path)
                .map_err(|e| e.to_string())?;
            a.copy_verified(dir, &mut owned)?;
            owned.sync_all().map_err(|e| e.to_string())?;
            File::open(path.parent().ok_or("evidence parent absent")?)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
            let own = owned_artifact(&path, &a.sha256, a.byte_len)?;
            let again = artifact_stamp(
                &open_source_file(dir, &a.relative, MAX_OUTPUT_BYTES)?,
                &a.sha256,
                a.byte_len,
            )?;
            if again != original {
                return Err("original artifact changed during capture".into());
            }
            (Some(original), Some(own))
        } else {
            (None, None)
        };
    let evidence = Evidence {
        version: 1,
        grant: held.grant.clone(),
        registration,
        registration_file,
        snapshot,
        snapshot_file,
        outcome,
        outcome_file,
        reply,
        record_sha256: sha256(
            &physical
                .read_witness(id, "json", 16 * 1024)
                .map_err(|e| e.to_string())?,
        ),
        terminal_sha256: sha256(
            &physical
                .read_witness(id, "terminal.json", 16 * 1024)
                .map_err(|e| e.to_string())?,
        ),
        stdout,
        stderr,
        artifact_original,
        artifact_owned,
    };
    let bytes = serde_json::to_vec(&evidence).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err("source evidence manifest too large".into());
    }
    let path = physical
        .evidence_path(id, "evidence.json")
        .map_err(|e| e.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    File::open(path.parent().ok_or("evidence parent absent")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    let stamp = identity(&file, sha256(&bytes), bytes.len() as u64)?;
    let readback = read_captured_v2_evidence(sidecar, physical, id)?;
    if readback.candidate != candidate || readback.manifest != stamp {
        return Err("source evidence readback changed".into());
    }
    Ok(readback)
}

/// Repeat all joins immediately before a future positive commit. No caller
/// JSON or original pathname is sufficient authority for this readback.
pub fn read_captured_v2_evidence(
    sidecar: &BrokerSidecar,
    physical: &SourcePhysicalRegistry,
    id: &str,
) -> Result<CapturedSourceEvidence, String> {
    let path = physical
        .evidence_path(id, "evidence.json")
        .map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let bytes = physical
        .read_witness(id, "evidence.json", MAX_MANIFEST)
        .map_err(|e| e.to_string())?;
    let stamp = identity(&file, sha256(&bytes), bytes.len() as u64)?;
    let named = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if (named.dev(), named.ino()) != (stamp.device, stamp.inode)
        || named.uid() != 0
        || named.mode() & 0o077 != 0
    {
        return Err("source evidence manifest identity changed".into());
    }
    let captured: Evidence = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let held = record(physical, id)?;
    let binding = sidecar.read_consumed_source_candidate(&held.grant)?;
    let source = binding.registration()?;
    let (stdout, stderr) = drained(physical, id)?;
    if captured.version != 1
        || captured.grant != held.grant
        || captured.registration != binding.registration_bytes()
        || captured.stdout != stdout
        || captured.stderr != stderr
        || captured.record_sha256
            != sha256(
                &physical
                    .read_witness(id, "json", 16 * 1024)
                    .map_err(|e| e.to_string())?,
            )
        || captured.terminal_sha256
            != sha256(
                &physical
                    .read_witness(id, "terminal.json", 16 * 1024)
                    .map_err(|e| e.to_string())?,
            )
        || captured.reply != reply(physical, id, &stdout)?
    {
        return Err("source evidence physical or State binding changed".into());
    }
    let dir = Path::new(&source.handle_dir);
    for (relative, limit, expected, stamp) in [
        (
            &source.registration_relative,
            MAX_REGISTRATION_BYTES,
            &captured.registration,
            &captured.registration_file,
        ),
        (
            &source.snapshot_relative,
            MAX_SNAPSHOT,
            &captured.snapshot,
            &captured.snapshot_file,
        ),
        (
            &source.outcome_relative,
            MAX_REGISTRATION_BYTES,
            &captured.outcome,
            &captured.outcome_file,
        ),
    ] {
        let (bytes, current) = source_bytes(dir, relative, limit)?;
        if &bytes != expected || &current != stamp {
            return Err("original source evidence changed after capture".into());
        }
    }
    let verified = VerifiedCompletion::from_source_files(&binding)?;
    if verified.snapshot_sha256 != sha256(&captured.snapshot)
        || verified.outcome_sha256 != sha256(&captured.outcome)
    {
        return Err("captured source evidence conflicts with original".into());
    }
    verified.validate_source_reply(
        &serde_json::from_slice(&captured.reply).map_err(|e| e.to_string())?,
    )?;
    match (
        &verified.snapshot.output,
        &captured.artifact_original,
        &captured.artifact_owned,
    ) {
        (CompletionOutput::Artifact(a), Some(original), Some(owned)) => {
            if &artifact_stamp(
                &open_source_file(dir, &a.relative, MAX_OUTPUT_BYTES)?,
                &a.sha256,
                a.byte_len,
            )? != original
            {
                return Err("original artifact changed after capture".into());
            }
            let path = physical
                .evidence_path(id, "evidence-artifact")
                .map_err(|e| e.to_string())?;
            if &owned_artifact(&path, &a.sha256, a.byte_len)? != owned {
                return Err("owned artifact changed after capture".into());
            }
        }
        (CompletionOutput::Artifact(_), _, _) | (_, Some(_), _) | (_, _, Some(_)) => {
            return Err("source artifact capture mismatch".into());
        }
        _ => {}
    }
    if sidecar.read_consumed_source_candidate(&held.grant)? != binding
        || drained(physical, id)? != (stdout.clone(), stderr)
    {
        return Err("source changed at capture readback fence".into());
    }
    let result = CapturedSourceEvidence {
        candidate: SourceV2Candidate {
            grant_id: id.into(),
            registration_id: source.registration_id,
            snapshot_sha256: verified.snapshot_sha256,
            outcome_sha256: verified.outcome_sha256,
            recovery_stdout_sha256: stdout.sha256,
        },
        manifest: stamp,
    };
    if let Some(row) = sidecar.read_source_evidence(&held.grant)?
        && (!matches!(row.phase.as_str(), "captured" | "accepted")
            || row.seal.as_ref() != Some(&seal(&result)))
    {
        return Err("broker evidence file differs from durable State seal".into());
    }
    Ok(result)
}

fn seal(captured: &CapturedSourceEvidence) -> BrokerSourceEvidenceSeal {
    BrokerSourceEvidenceSeal {
        manifest_sha256: captured.manifest.sha256.clone(),
        manifest_device: captured.manifest.device,
        manifest_inode: captured.manifest.inode,
        manifest_byte_len: captured.manifest.byte_len,
        snapshot_sha256: captured.candidate.snapshot_sha256.clone(),
        outcome_sha256: captured.candidate.outcome_sha256.clone(),
        recovery_stdout_sha256: captured.candidate.recovery_stdout_sha256.clone(),
    }
}

/// Stage one exact snapshot in the retained broker ledger. An I/O failure
/// leaves the consumed grant and, when SQLite is available, explicit unknown
/// evidence debt. A lost reply can only be reconciled by exact readback.
pub fn capture_and_stage_v2_evidence(
    sidecar: &mut BrokerSidecar,
    physical: &SourcePhysicalRegistry,
    id: &str,
) -> Result<CapturedSourceEvidence, String> {
    let grant = record(physical, id)?.grant.clone();
    let result = (|| {
        let captured = capture_v2_evidence(sidecar, physical, id)?;
        let recorded = sidecar.record_source_evidence_snapshot(&grant, &seal(&captured))?;
        if recorded.phase != "captured" || recorded.seal.as_ref() != Some(&seal(&captured)) {
            return Err("captured source evidence transition changed".into());
        }
        Ok(captured)
    })();
    if result.is_err() {
        // A failed SQLite write cannot erase the already consumed grant.
        let _ = sidecar.retain_unknown_source_evidence(&grant);
    }
    result
}

/// The positive transition is closed on this lineage: there is no writer for
/// fresh v30 admission provenance yet. Its CAS and exact readback exist so
/// the fresh-lane integration can connect authority without importing v29.
pub fn commit_v2_evidence(
    sidecar: &mut BrokerSidecar,
    physical: &SourcePhysicalRegistry,
    id: &str,
) -> Result<(), String> {
    let captured = read_captured_v2_evidence(sidecar, physical, id)?;
    let grant = record(physical, id)?.grant.clone();
    let expected = seal(&captured);
    let staged = sidecar
        .read_source_evidence(&grant)?
        .ok_or("source evidence stage absent")?;
    if staged.phase != "captured" || staged.seal.as_ref() != Some(&expected) {
        return Err("source evidence stage changed before commit".into());
    }
    let accepted = sidecar.commit_source_evidence_acceptance(&grant, &expected)?;
    if accepted.phase != "accepted"
        || accepted.seal.as_ref() != Some(&expected)
        || read_captured_v2_evidence(sidecar, physical, id)? != captured
    {
        return Err("source evidence changed across commit readback".into());
    }
    Ok(())
}
