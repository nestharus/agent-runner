//! Exact source material for the existing State completion admission ledger.
//! Declared roles: parser, validator, mapper, accessor.
use crate::mailbox::CompletionEventRegistrationInput;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path};

pub const PROTOCOL: &str = "completion-continuation-v2";
pub const MAX_REGISTRATION_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProcessIdentity {
    pub pid: i64,
    pub boot_id: String,
    pub starttime_ticks: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedExecutable {
    pub path: String,
    pub sha256: String,
    pub environment_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenerIdentity {
    pub listener_id: String,
    pub session_id: String,
    pub owner_invocation_uuid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRegistration {
    pub protocol: String,
    pub domain_id: String,
    pub source_id: String,
    pub handle: String,
    pub registration_id: String,
    pub spool_root: String,
    pub handle_dir: String,
    pub meta_relative: String,
    pub log_relative: String,
    pub rc_relative: String,
    pub registration_relative: String,
    pub snapshot_relative: String,
    pub outcome_relative: String,
    pub owner_session_id: String,
    pub owner_invocation_uuid: String,
    pub registering_caller: SourceProcessIdentity,
    pub delivery_mode: String,
    pub completion_kind: String,
    pub completion_scope: String,
    pub helper: PinnedExecutable,
    pub recovery: PinnedExecutable,
    pub source_evidence_protocol: String,
    pub listener_revision: u64,
    pub listeners: Vec<ListenerIdentity>,
}

/// Serialized into the existing immutable obligation INSERT. The original bytes
/// survive JSON parsing; neither readback nor repair reserializes the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedSourceBinding {
    caller_admission_id: String,
    registration_bytes_utf8: String,
    registration_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admitted_listener: Option<ListenerIdentity>,
}

impl AdmittedSourceBinding {
    pub fn new(caller_admission_id: &str, bytes: &[u8]) -> Result<Self, String> {
        if caller_admission_id.trim().is_empty() || bytes.len() > MAX_REGISTRATION_BYTES {
            return Err("invalid completion continuation admission or registration length".into());
        }
        let registration_bytes_utf8 = std::str::from_utf8(bytes)
            .map_err(|e| e.to_string())?
            .to_owned();
        let result = Self {
            caller_admission_id: caller_admission_id.to_owned(),
            registration_bytes_utf8,
            registration_digest: sha256(bytes),
            admitted_listener: None,
        };
        result.registration()?;
        Ok(result)
    }

    pub fn registration(&self) -> Result<SourceRegistration, String> {
        let bytes = self.registration_bytes_utf8.as_bytes();
        if bytes.len() > MAX_REGISTRATION_BYTES || sha256(bytes) != self.registration_digest {
            return Err("completion registration digest/length conflict".into());
        }
        let registration: SourceRegistration =
            serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        registration.validate()?;
        if let Some(listener) = &self.admitted_listener
            && (listener.session_id.trim().is_empty()
                || listener.listener_id != listener.owner_invocation_uuid
                || uuid::Uuid::parse_str(&listener.owner_invocation_uuid).is_err())
        {
            return Err("invalid admitted listener identity".into());
        }
        Ok(registration)
    }

    /// A later listener is separate admitted authority, not a rewritten source.
    pub fn for_listener(
        &self,
        caller_admission_id: &str,
        listener: ListenerIdentity,
    ) -> Result<Self, String> {
        if caller_admission_id.trim().is_empty()
            || listener.session_id.trim().is_empty()
            || listener.listener_id != listener.owner_invocation_uuid
            || uuid::Uuid::parse_str(&listener.owner_invocation_uuid).is_err()
        {
            return Err("invalid late completion listener authority".into());
        }
        let mut bound = self.clone();
        bound.caller_admission_id = caller_admission_id.into();
        bound.admitted_listener = Some(listener);
        Ok(bound)
    }
    pub fn admission_listener(&self) -> Result<ListenerIdentity, String> {
        let source = self.registration()?;
        Ok(self
            .admitted_listener
            .clone()
            .unwrap_or_else(|| source.listeners[0].clone()))
    }
    pub(crate) fn is_late_listener(&self) -> bool {
        self.admitted_listener.is_some()
    }
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.registration_bytes_utf8 == other.registration_bytes_utf8
            && self.registration_digest == other.registration_digest
    }

    pub fn registration_bytes(&self) -> &[u8] {
        self.registration_bytes_utf8.as_bytes()
    }
    pub fn registration_digest(&self) -> &str {
        &self.registration_digest
    }
    pub fn caller_admission_id(&self) -> &str {
        &self.caller_admission_id
    }
    pub(crate) fn encoded(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        let binding: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        binding.registration()?;
        Ok(binding)
    }
    pub(crate) fn validate_input(
        &self,
        admission: &str,
        input: &CompletionEventRegistrationInput<'_>,
    ) -> Result<(), String> {
        let registration = self.registration()?;
        let paths = registration.paths();
        let listener = self.admission_listener()?;
        if admission != self.caller_admission_id
            || input.event_id != registration.handle
            || input.delivery_mode != registration.delivery_mode
            || input.owner_session_id != Some(listener.session_id.as_str())
            || input.owner_invocation_uuid != Some(listener.owner_invocation_uuid.as_str())
            || input.state_dir != registration.handle_dir
            || input.meta_path != paths[0]
            || input.log_path != paths[1]
            || input.rc_path != paths[2]
        {
            return Err("completion continuation immutable registration/input conflict".into());
        }
        Ok(())
    }
}

impl SourceRegistration {
    pub fn paths(&self) -> [String; 3] {
        [&self.meta_relative, &self.log_relative, &self.rc_relative].map(|p| {
            Path::new(&self.handle_dir)
                .join(p)
                .to_string_lossy()
                .into_owned()
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.protocol != PROTOCOL || self.source_evidence_protocol != PROTOCOL {
            return Err("unsupported completion continuation protocol".into());
        }
        for id in [&self.domain_id, &self.source_id, &self.registration_id] {
            uuid::Uuid::parse_str(id).map_err(|_| "invalid completion incarnation UUID")?;
        }
        for value in [
            &self.handle,
            &self.owner_session_id,
            &self.owner_invocation_uuid,
        ] {
            if value.is_empty() || value.trim() != value || value.len() > 4096 {
                return Err("invalid completion source identity".into());
            }
        }
        if !matches!(self.delivery_mode.as_str(), "sync" | "async")
            || !matches!(self.completion_kind.as_str(), "exit" | "ready")
            || !matches!(self.completion_scope.as_str(), "root" | "tree")
        {
            return Err("invalid completion mode/kind/scope".into());
        }
        validate_absolute(&self.spool_root)?;
        validate_absolute(&self.handle_dir)?;
        if Path::new(&self.handle_dir).parent() != Some(Path::new(&self.spool_root))
            || Path::new(&self.handle_dir)
                .file_name()
                .and_then(|p| p.to_str())
                != Some(&self.handle)
        {
            return Err(
                "completion handle directory does not match spool/incarnation routing".into(),
            );
        }
        for relative in [
            &self.meta_relative,
            &self.log_relative,
            &self.rc_relative,
            &self.registration_relative,
            &self.snapshot_relative,
            &self.outcome_relative,
        ] {
            if relative.is_empty()
                || Path::new(relative)
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
            {
                return Err("completion source path is not a confined relative path".into());
            }
        }
        for image in [&self.helper, &self.recovery] {
            validate_absolute(&image.path)?;
            if !Path::new(&image.path).starts_with(&self.handle_dir)
                || !is_sha256(&image.sha256)
                || !is_sha256(&image.environment_sha256)
            {
                return Err("invalid pinned completion executable binding".into());
            }
        }
        if self.registering_caller.pid <= 0
            || self.registering_caller.starttime_ticks < 0
            || self.registering_caller.boot_id.is_empty()
        {
            return Err("invalid registering caller identity".into());
        }
        // Native registration initially admits the owner. Late listeners use
        // existing authoritative listener admission, not producer-invented rows.
        if self.listener_revision != 1
            || self.listeners.len() != 1
            || self.listeners[0].listener_id != self.owner_invocation_uuid
            || self.listeners[0].session_id != self.owner_session_id
            || self.listeners[0].owner_invocation_uuid != self.owner_invocation_uuid
        {
            return Err("initial completion listener identity/revision conflict".into());
        }
        Ok(())
    }
}

fn validate_absolute(path: &str) -> Result<(), String> {
    if !Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("completion source path must be absolute and normalized".into());
    }
    Ok(())
}
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn is_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/fixtures/age360-paired-wire.json")).unwrap()
    }

    #[test]
    fn completion_continuation_retains_exact_canonical_wire_digest() {
        let fixture = fixture();
        let bytes = fixture["registration_bytes_utf8"]
            .as_str()
            .unwrap()
            .as_bytes();
        let binding = AdmittedSourceBinding::new("test", bytes).unwrap();
        assert_eq!(
            binding.registration_digest(),
            fixture["common_identity"]["registration_digest"]
                .as_str()
                .unwrap()
        );
        assert_eq!(binding.registration_bytes(), bytes);
        let mut other = bytes.to_vec();
        other.push(b' ');
        let other = AdmittedSourceBinding::new("test", &other).unwrap();
        assert_eq!(
            binding.registration().unwrap(),
            other.registration().unwrap()
        );
        assert_ne!(binding.registration_digest(), other.registration_digest());
    }

    #[test]
    fn completion_continuation_rejects_unconfined_paths_and_incompatible_protocol() {
        for (field, value) in [
            ("meta_relative", "../other/meta.json"),
            ("handle_dir", "/private/elsewhere/ab_fixture"),
            ("protocol", "owned-v1"),
            ("source_id", "ab_fixture"),
        ] {
            let mut registration = fixture()["registration"].clone();
            registration[field] = value.into();
            assert!(
                AdmittedSourceBinding::new("test", &serde_json::to_vec(&registration).unwrap())
                    .is_err()
            );
        }
    }
}

mod evidence;
mod source_files;
pub use evidence::{
    CompletionIdentity, CompletionOutput, CompletionSnapshot, MAX_OUTPUT_BYTES,
    MissingOriginalOutput, OriginalOutputSelection, OutputArtifact, SourceOutcome,
    VerifiedCompletion,
};
pub use source_files::{open_source_file, read_source_file};

/// Validate the current admission extension without repairing or manufacturing it.
/// Historical schema23 readers remain readable and are not migrated here.
pub(crate) fn validate_admission_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    let shape: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('invocation_completion_obligations') WHERE name='completion_v2_binding' AND type='BLOB' AND [notnull]=0 AND dflt_value IS NULL",
        [], |r|r.get(0)).map_err(|e|e.to_string())?;
    if shape != 1 {
        return Err(
            "completion continuation schema24 admission binding missing or incompatible".into(),
        );
    }
    Ok(())
}

/// Explicitly compiled fault fixtures; absent from normal binaries. Refuse a
/// host-network invocation even if someone accidentally inherits fixture vars.
#[cfg(all(feature = "age360-fault-fixtures", target_os = "linux"))]
pub fn age360_fault_barrier(name: &str) {
    let Some(root) = std::env::var_os("AGE360_FAULT_ROOT") else {
        return;
    };
    let Some(parent_net) = std::env::var_os("AGE360_FAULT_PARENT_NET") else {
        return;
    };
    if std::fs::read_link("/proc/self/ns/net")
        .ok()
        .is_none_or(|net| net.as_os_str() == parent_net)
    {
        return;
    }
    let root = Path::new(&root);
    let hold = root.join(format!("{name}.hold"));
    if !hold.exists() {
        return;
    }
    let _ = std::fs::write(
        root.join(format!("{name}.reached")),
        std::process::id().to_string(),
    );
    while hold.exists() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
#[cfg(all(feature = "age360-fault-fixtures", not(target_os = "linux")))]
pub fn age360_fault_barrier(_name: &str) {}
