//! Read-only, source-byte corroboration for one Codex rollout JSONL user turn.
//!
//! A `Certified` result certifies only this pinned file's bytes under the
//! declared exact UTF-8 transform. It does not bind the file to a selected
//! resident Codex process, authenticate a caller's Tail time, or authorize a
//! native-F receipt or ACK. The default Broker deliberately does not call it.

use base64::Engine as _;
use oulipoly_state::mailbox::FreshNativeFPreparation;
use serde::Serialize;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata, OpenOptions};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const CODEX_ROLLOUT_FORMAT: &str = "codex.rollout.jsonl/user-input-text-v1";
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 512 * 1024;
const MAX_BODY_BYTES: usize = 384 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactInputTransform {
    /// JSON `input_text.text` decoded as UTF-8, with no newline, whitespace,
    /// marker, or Unicode normalization. This is an assumption to be proven
    /// for the physical PTY-to-store path before product integration.
    ExactUtf8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    SourceUnavailable,
    SourceChanged,
    SourceTooLarge,
    IncompleteRecord,
    InvalidTail,
    PostTailSequence,
    EnvelopeMismatch,
    BodyMismatch,
    AdapterMismatch,
    MalformedSource,
    UnsupportedFormat,
    UnsupportedRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Pending(RefusalReason),
    Unknown(RefusalReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawVerdict {
    /// Raw-file agreement only; never a product receipt or ACK instruction.
    Certified(RawTurnCertificate),
    Pending(RefusalReason),
    Unknown(RefusalReason),
}

impl From<Refusal> for RawVerdict {
    fn from(value: Refusal) -> Self {
        match value {
            Refusal::Pending(reason) => Self::Pending(reason),
            Refusal::Unknown(reason) => Self::Unknown(reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceIdentity {
    pub device: u64,
    pub inode: u64,
}

impl SourceIdentity {
    fn of(meta: &Metadata) -> Self {
        Self {
            device: meta.dev(),
            inode: meta.ino(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTurnCertificate {
    pub format: &'static str,
    pub source: SourceIdentity,
    pub source_len: u64,
    pub source_sha256: String,
    pub tail_offset: u64,
    pub turn_offset: u64,
    pub turn_id: String,
    pub raw_body_sha256: String,
}

/// Immutable fields copied from State's prepared F, rather than projected
/// from a provider page. A caller must still supply independent custody of
/// the selected process and source; this struct cannot establish either.
pub struct PreparedInput<'a> {
    pub envelope_text: &'a str,
    pub envelope_sha256: &'a str,
    pub envelope_nonce: &'a str,
    pub source_id: &'a str,
    pub session_id: &'a str,
    pub row: i64,
    pub payload_sha256: &'a str,
    pub payload_byte_len: i64,
    pub provider_instance_id: &'a str,
    pub settings_id: &'a str,
    pub provider_session_id: &'a str,
    pub tail_resume_token: &'a str,
}

impl<'a> From<&'a FreshNativeFPreparation> for PreparedInput<'a> {
    fn from(value: &'a FreshNativeFPreparation) -> Self {
        Self {
            envelope_text: &value.envelope_text,
            envelope_sha256: &value.envelope_sha256,
            envelope_nonce: &value.envelope_nonce,
            source_id: &value.source_id,
            session_id: &value.session_id,
            row: value.seq,
            payload_sha256: &value.payload_sha256,
            payload_byte_len: value.payload_byte_len,
            provider_instance_id: &value.provider_instance_id,
            settings_id: &value.settings_id,
            provider_session_id: &value.provider_session_id,
            tail_resume_token: &value.tail_resume_token,
        }
    }
}

/// Selected adapter fields are comparison targets, never source authority.
/// The Tail source fields can be decoded from the adapter's opaque cursor,
/// but their agreement does not authenticate that cursor or the file writer.
#[derive(Debug, Clone)]
pub struct AdapterObservation<'a> {
    pub provider_instance_id: &'a str,
    pub settings_id: &'a str,
    pub session_id: &'a str,
    pub anchor_token: &'a str,
    pub tail_source: SourceIdentity,
    pub tail_source_len: u64,
    pub tail_offset: u64,
    pub page_start_sequence: u64,
    pub page_turn_count: u64,
    pub snapshot_complete: bool,
    pub source_bytes_examined: u64,
    pub turn_id: &'a str,
    pub role: &'a str,
    pub body_state: &'a str,
    pub body: &'a str,
    pub body_sha256: &'a str,
    pub canonical_text_sha256: &'a str,
}

/// Holds the actual pre-F descriptor. A path replacement is rejected even
/// though the descriptor continues to address the old inode.
pub struct PinnedCodexRollout {
    path: PathBuf,
    file: File,
    identity: SourceIdentity,
    session_id: String,
    tail_offset: u64,
    tail_sha256: String,
    prior_user_turns: u64,
}

impl PinnedCodexRollout {
    /// Capture a complete, bounded pre-F Tail. Call before the one-use F fence.
    /// A selected-process-to-path binding must be checked separately.
    pub fn capture(path: &Path, session_id: &str, format: &str) -> Result<Self, Refusal> {
        if format != CODEX_ROLLOUT_FORMAT {
            return Err(Refusal::Unknown(RefusalReason::UnsupportedFormat));
        }
        if session_id.is_empty() || session_id.len() > 256 {
            return Err(Refusal::Unknown(RefusalReason::UnsupportedFormat));
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| Refusal::Pending(RefusalReason::SourceUnavailable))?;
        let (bytes, meta) = read_stable(&file, path, || {})?;
        let records = parse_records(&bytes, session_id)?;
        let prior_user_turns = records.iter().try_fold(0u64, |count, record| {
            if user_body(&record.value)?.is_some() {
                count
                    .checked_add(1)
                    .ok_or(Refusal::Unknown(RefusalReason::UnsupportedRecord))
            } else {
                Ok(count)
            }
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity: SourceIdentity::of(&meta),
            session_id: session_id.to_owned(),
            tail_offset: bytes.len() as u64,
            tail_sha256: digest(&bytes),
            prior_user_turns,
        })
    }

    pub fn source_identity(&self) -> SourceIdentity {
        self.identity
    }

    pub fn tail_offset(&self) -> u64 {
        self.tail_offset
    }

    pub fn verify(
        &self,
        prepared: &PreparedInput<'_>,
        adapter: &AdapterObservation<'_>,
        transform: ExactInputTransform,
    ) -> RawVerdict {
        self.verify_after_read(prepared, adapter, transform, || {})
    }

    fn verify_after_read(
        &self,
        prepared: &PreparedInput<'_>,
        adapter: &AdapterObservation<'_>,
        transform: ExactInputTransform,
        after_read: impl FnOnce(),
    ) -> RawVerdict {
        let result = (|| -> Result<RawTurnCertificate, Refusal> {
            validate_envelope(prepared)?;
            if self.session_id != prepared.provider_session_id {
                return Err(Refusal::Pending(RefusalReason::EnvelopeMismatch));
            }
            let (bytes, meta) = read_stable(&self.file, &self.path, after_read)?;
            if SourceIdentity::of(&meta) != self.identity
                || bytes.len() as u64 <= self.tail_offset
                || digest(&bytes[..self.tail_offset as usize]) != self.tail_sha256
            {
                return Err(Refusal::Pending(RefusalReason::SourceChanged));
            }
            if adapter.tail_offset != self.tail_offset
                || adapter.tail_source_len != self.tail_offset
                || adapter.tail_source != self.identity
            {
                return Err(Refusal::Pending(RefusalReason::InvalidTail));
            }
            let records = parse_records(&bytes, &self.session_id)?;
            let post: Vec<_> = records
                .iter()
                .filter(|record| record.offset >= self.tail_offset)
                .collect();
            if post.len() != 1 || post[0].offset != self.tail_offset {
                return Err(Refusal::Pending(RefusalReason::PostTailSequence));
            }
            let record = post[0];
            let body = user_body(&record.value)?
                .ok_or(Refusal::Pending(RefusalReason::PostTailSequence))?;
            if body.len() > MAX_BODY_BYTES {
                return Err(Refusal::Unknown(RefusalReason::SourceTooLarge));
            }
            match transform {
                ExactInputTransform::ExactUtf8
                    if body.as_bytes() != prepared.envelope_text.as_bytes() =>
                {
                    return Err(Refusal::Pending(RefusalReason::BodyMismatch));
                }
                ExactInputTransform::ExactUtf8 => {}
            }
            let turn_id = format!("{}:byte:{}", self.session_id, record.offset);
            if !adapter_matches(adapter, prepared, self, bytes.len() as u64, &turn_id)? {
                return Err(Refusal::Pending(RefusalReason::AdapterMismatch));
            }
            Ok(RawTurnCertificate {
                format: CODEX_ROLLOUT_FORMAT,
                source: self.identity,
                source_len: bytes.len() as u64,
                source_sha256: digest(&bytes),
                tail_offset: self.tail_offset,
                turn_offset: record.offset,
                turn_id,
                raw_body_sha256: digest(body.as_bytes()),
            })
        })();
        match result {
            Ok(certificate) => RawVerdict::Certified(certificate),
            Err(refusal) => refusal.into(),
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn same_revision(a: &Metadata, b: &Metadata) -> bool {
    SourceIdentity::of(a) == SourceIdentity::of(b)
        && a.len() == b.len()
        && a.mode() == b.mode()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

fn read_stable(
    file: &File,
    path: &Path,
    after_read: impl FnOnce(),
) -> Result<(Vec<u8>, Metadata), Refusal> {
    let before = file
        .metadata()
        .map_err(|_| Refusal::Pending(RefusalReason::SourceUnavailable))?;
    if !before.is_file() {
        return Err(Refusal::Unknown(RefusalReason::UnsupportedFormat));
    }
    if before.len() > MAX_FILE_BYTES {
        return Err(Refusal::Unknown(RefusalReason::SourceTooLarge));
    }
    let mut bytes = vec![0; before.len() as usize];
    file.read_exact_at(&mut bytes, 0)
        .map_err(|_| Refusal::Pending(RefusalReason::SourceChanged))?;
    after_read();
    let after = file
        .metadata()
        .map_err(|_| Refusal::Pending(RefusalReason::SourceChanged))?;
    let path_meta = path
        .symlink_metadata()
        .map_err(|_| Refusal::Pending(RefusalReason::SourceChanged))?;
    if !same_revision(&before, &after) || !path_meta.is_file() || !same_revision(&after, &path_meta)
    {
        return Err(Refusal::Pending(RefusalReason::SourceChanged));
    }
    Ok((bytes, after))
}

struct Record {
    offset: u64,
    value: Value,
}

/// `serde_json::Value` normally keeps the last occurrence of a duplicate
/// object key. Reject ambiguous source bytes before interpreting any field.
struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
        let number = serde_json::Number::from_f64(value)
            .ok_or_else(|| E::custom("non-finite JSON number"))?;
        Ok(UniqueValue(Value::Number(number)))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value.to_owned())))
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = items.next_element::<UniqueValue>()? {
            values.push(value.0);
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut items: A) -> Result<Self::Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = items.next_key::<String>()? {
            let value = items.next_value::<UniqueValue>()?.0;
            if values.insert(key, value).is_some() {
                return Err(serde::de::Error::custom("duplicate JSON object key"));
            }
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}

fn parse_records(bytes: &[u8], session_id: &str) -> Result<Vec<Record>, Refusal> {
    if bytes.is_empty() || !bytes.ends_with(b"\n") {
        return Err(Refusal::Pending(RefusalReason::IncompleteRecord));
    }
    let mut offset = 0u64;
    let mut records = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.len() > MAX_RECORD_BYTES {
            return Err(Refusal::Unknown(RefusalReason::SourceTooLarge));
        }
        let UniqueValue(value) = serde_json::from_slice(line)
            .map_err(|_| Refusal::Unknown(RefusalReason::MalformedSource))?;
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return Err(Refusal::Unknown(RefusalReason::UnsupportedRecord));
        };
        if records.is_empty() {
            if kind != "session_meta"
                || value.pointer("/payload/id").and_then(Value::as_str) != Some(session_id)
            {
                return Err(Refusal::Pending(RefusalReason::EnvelopeMismatch));
            }
        } else if kind == "session_meta" {
            return Err(Refusal::Unknown(RefusalReason::UnsupportedRecord));
        }
        if !matches!(
            kind,
            "session_meta" | "response_item" | "event_msg" | "turn_context" | "compacted"
        ) {
            return Err(Refusal::Unknown(RefusalReason::UnsupportedRecord));
        }
        records.push(Record { offset, value });
        offset += line.len() as u64;
    }
    Ok(records)
}

fn user_body(value: &Value) -> Result<Option<&str>, Refusal> {
    if value.get("type").and_then(Value::as_str) != Some("response_item")
        || value.pointer("/payload/role").and_then(Value::as_str) != Some("user")
    {
        return Ok(None);
    }
    let Some(content) = value.pointer("/payload/content").and_then(Value::as_array) else {
        return Err(Refusal::Unknown(RefusalReason::UnsupportedRecord));
    };
    let Some(payload) = value.get("payload").and_then(Value::as_object) else {
        return Err(Refusal::Unknown(RefusalReason::UnsupportedRecord));
    };
    let Some(part) = content.first().and_then(Value::as_object) else {
        return Err(Refusal::Unknown(RefusalReason::UnsupportedRecord));
    };
    if value.pointer("/payload/type").and_then(Value::as_str) != Some("message")
        || payload.len() != 3
        || !payload.contains_key("type")
        || !payload.contains_key("role")
        || !payload.contains_key("content")
        || content.len() != 1
        || part.len() != 2
        || part.get("type").and_then(Value::as_str) != Some("input_text")
        || !part.contains_key("text")
    {
        return Err(Refusal::Unknown(RefusalReason::UnsupportedRecord));
    }
    content[0]
        .get("text")
        .and_then(Value::as_str)
        .map(Some)
        .ok_or(Refusal::Unknown(RefusalReason::UnsupportedRecord))
}

fn validate_envelope(prepared: &PreparedInput<'_>) -> Result<(), Refusal> {
    let body = prepared.envelope_text;
    if body.len() > MAX_BODY_BYTES || digest(body.as_bytes()) != prepared.envelope_sha256 {
        return Err(Refusal::Pending(RefusalReason::EnvelopeMismatch));
    }
    let fields: Vec<_> = body.split('\n').collect();
    let expected = [
        "[Oulipoly native F v1]".to_owned(),
        format!("nonce: {}", prepared.envelope_nonce),
        format!("source: {}", prepared.source_id),
        format!("session: {}", prepared.session_id),
        format!("row: {}", prepared.row),
        format!("payload-sha256: {}", prepared.payload_sha256),
    ];
    if fields.len() != 8
        || fields[..6] != expected
        || fields[7] != "[/Oulipoly native F v1]"
        || prepared.session_id != prepared.provider_session_id
        || prepared.payload_byte_len < 0
    {
        return Err(Refusal::Pending(RefusalReason::EnvelopeMismatch));
    }
    let encoded = fields[6]
        .strip_prefix("payload-base64: ")
        .ok_or(Refusal::Pending(RefusalReason::EnvelopeMismatch))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| Refusal::Pending(RefusalReason::EnvelopeMismatch))?;
    if decoded.len() as i64 != prepared.payload_byte_len
        || digest(&decoded) != prepared.payload_sha256
    {
        return Err(Refusal::Pending(RefusalReason::EnvelopeMismatch));
    }
    Ok(())
}

#[derive(Serialize)]
struct TextChunk<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'a str,
}

fn adapter_matches(
    adapter: &AdapterObservation<'_>,
    prepared: &PreparedInput<'_>,
    pinned: &PinnedCodexRollout,
    source_len: u64,
    turn_id: &str,
) -> Result<bool, Refusal> {
    let chunks = [TextChunk {
        kind: "text",
        text: prepared.envelope_text,
    }];
    let chunk_hash = digest(
        &serde_json::to_vec(&chunks)
            .map_err(|_| Refusal::Unknown(RefusalReason::UnsupportedFormat))?,
    );
    Ok(
        adapter.provider_instance_id == prepared.provider_instance_id
            && adapter.settings_id == prepared.settings_id
            && adapter.session_id == prepared.provider_session_id
            && adapter.anchor_token == prepared.tail_resume_token
            && adapter.page_start_sequence == pinned.prior_user_turns
            && adapter.page_turn_count == 1
            && adapter.snapshot_complete
            && adapter.source_bytes_examined == source_len
            && adapter.turn_id == turn_id
            && adapter.role == "user"
            && adapter.body_state == "inline"
            && adapter.body == prepared.envelope_text
            && adapter.body_sha256 == chunk_hash
            && adapter.canonical_text_sha256 == digest(prepared.envelope_text.trim().as_bytes()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    const BASELINE: &[u8] = include_bytes!("../tests/fixtures/age319-codex-rollout-baseline.jsonl");
    const SESSION: &str = "age319-synthetic-session";
    const NONCE: &str = "f05e0000-0000-4000-8000-000000000319";
    const SOURCE: &str = "synthetic-source-319";
    const PAYLOAD_SHA: &str = "ea77d7f3b40e5422286396d3f638bbffe9a85dc4d4d38ebedbc1261fb87c5bf7";
    const BODY_SHA: &str = "d8f8f687561664b3d9908305a7d6cc4f77d3f4567cdc8349e666505bd599d836";
    const CHUNK_SHA: &str = "091d70393092d7cf0d061043a4e16b1ac164b3f39a67b33d74429ee5caa61be7";
    const FILE_SHA: &str = "c873856dc87a9cde3a11eaf66063d81a895d7658f437af0c34f4c9d30b1a99f3";

    struct Fixture {
        _dir: TempDir,
        path: PathBuf,
        pinned: PinnedCodexRollout,
        body: String,
        body_hash: String,
        chunk_hash: String,
    }

    impl Fixture {
        fn new() -> Self {
            assert_eq!(digest(BASELINE), FILE_SHA);
            let dir = TempDir::new().unwrap();
            let path = dir
                .path()
                .join("rollout-synthetic-age319-synthetic-session.jsonl");
            std::fs::write(&path, &BASELINE[..87]).unwrap();
            let pinned = PinnedCodexRollout::capture(&path, SESSION, CODEX_ROLLOUT_FORMAT).unwrap();
            assert_eq!(pinned.tail_offset(), 87);
            let value: Value = serde_json::from_slice(&BASELINE[87..]).unwrap();
            let body = value
                .pointer("/payload/content/0/text")
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned();
            assert_eq!(digest(body.as_bytes()), BODY_SHA);
            Self {
                _dir: dir,
                path,
                pinned,
                body,
                body_hash: BODY_SHA.to_owned(),
                chunk_hash: CHUNK_SHA.to_owned(),
            }
        }

        fn append(&self, bytes: &[u8]) {
            OpenOptions::new()
                .append(true)
                .open(&self.path)
                .unwrap()
                .write_all(bytes)
                .unwrap();
        }

        fn append_baseline(&self) {
            self.append(&BASELINE[87..]);
        }

        fn append_user(&self, body: &str) {
            let mut value: Value = serde_json::from_slice(&BASELINE[87..]).unwrap();
            value["payload"]["content"][0]["text"] = Value::String(body.to_owned());
            let mut bytes = serde_json::to_vec(&value).unwrap();
            bytes.push(b'\n');
            self.append(&bytes);
        }

        fn prepared(&self) -> PreparedInput<'_> {
            PreparedInput {
                envelope_text: &self.body,
                envelope_sha256: &self.body_hash,
                envelope_nonce: NONCE,
                source_id: SOURCE,
                session_id: SESSION,
                row: 7,
                payload_sha256: PAYLOAD_SHA,
                payload_byte_len: 35,
                provider_instance_id: "synthetic-codex-installed",
                settings_id: "codex",
                provider_session_id: SESSION,
                tail_resume_token: "synthetic-tail-token",
            }
        }

        fn adapter(&self) -> AdapterObservation<'_> {
            AdapterObservation {
                provider_instance_id: "synthetic-codex-installed",
                settings_id: "codex",
                session_id: SESSION,
                anchor_token: "synthetic-tail-token",
                tail_source: self.pinned.source_identity(),
                tail_source_len: 87,
                tail_offset: 87,
                page_start_sequence: 0,
                page_turn_count: 1,
                snapshot_complete: true,
                source_bytes_examined: 547,
                turn_id: "age319-synthetic-session:byte:87",
                role: "user",
                body_state: "inline",
                body: &self.body,
                body_sha256: &self.chunk_hash,
                canonical_text_sha256: &self.body_hash,
            }
        }

        fn verify(&self) -> RawVerdict {
            self.pinned.verify(
                &self.prepared(),
                &self.adapter(),
                ExactInputTransform::ExactUtf8,
            )
        }
    }

    #[test]
    fn exact_offline_installed_adapter_fixture_matches_raw_bytes() {
        let fixture = Fixture::new();
        fixture.append_baseline();
        let RawVerdict::Certified(cert) = fixture.verify() else {
            panic!("synthetic baseline must certify raw-file agreement");
        };
        assert_eq!(cert.source_sha256, FILE_SHA);
        assert_eq!(cert.raw_body_sha256, BODY_SHA);
        assert_eq!(cert.source_len, 547);
        assert_eq!(cert.turn_offset, 87);
        assert_eq!(cert.turn_id, "age319-synthetic-session:byte:87");
        assert_eq!(cert.source, fixture.pinned.source_identity());
    }

    #[test]
    fn body_nonce_crlf_and_stripped_marker_all_refuse_raw_match() {
        let mutations = [
            Fixture::new().body.replace("row: 7", "row: 8"),
            Fixture::new()
                .body
                .replace(NONCE, "f05e0000-0000-4000-8000-000000000320"),
            Fixture::new()
                .body
                .replace(SESSION, "different-synthetic-session"),
            Fixture::new().body.replace('\n', "\r\n"),
            format!(
                "{}\n[OULIPOLY-DELIVERY 8d183a020b06d7a5d8017e471caa9be070f866e33c0dca89a552932b9d0065e4]",
                Fixture::new().body
            ),
        ];
        for changed_body in mutations {
            let fixture = Fixture::new();
            fixture.append_user(&changed_body);
            assert_eq!(
                fixture.verify(),
                RawVerdict::Pending(RefusalReason::BodyMismatch)
            );
        }
    }

    #[test]
    fn session_inode_tail_and_source_prefix_changes_refuse() {
        let fixture = Fixture::new();
        let wrong_header = String::from_utf8(BASELINE[..87].to_vec())
            .unwrap()
            .replace(SESSION, "different-synthetic-session");
        std::fs::write(&fixture.path, wrong_header).unwrap();
        assert!(matches!(
            PinnedCodexRollout::capture(&fixture.path, SESSION, CODEX_ROLLOUT_FORMAT),
            Err(Refusal::Pending(RefusalReason::EnvelopeMismatch))
        ));

        let fixture = Fixture::new();
        let replacement = fixture.path.with_extension("replacement");
        std::fs::write(&replacement, BASELINE).unwrap();
        std::fs::rename(replacement, &fixture.path).unwrap();
        assert_eq!(
            fixture.verify(),
            RawVerdict::Pending(RefusalReason::SourceChanged)
        );

        let fixture = Fixture::new();
        fixture.append_baseline();
        let mut adapter = fixture.adapter();
        adapter.tail_offset = 88;
        assert_eq!(
            fixture.pinned.verify(
                &fixture.prepared(),
                &adapter,
                ExactInputTransform::ExactUtf8
            ),
            RawVerdict::Pending(RefusalReason::InvalidTail)
        );
        adapter.tail_offset = 547;
        assert_eq!(
            fixture.pinned.verify(
                &fixture.prepared(),
                &adapter,
                ExactInputTransform::ExactUtf8
            ),
            RawVerdict::Pending(RefusalReason::InvalidTail)
        );

        let fixture = Fixture::new();
        let mut changed = BASELINE[..87].to_vec();
        changed[0] = b' ';
        std::fs::write(&fixture.path, changed).unwrap();
        fixture.append_baseline();
        assert_eq!(
            fixture.verify(),
            RawVerdict::Pending(RefusalReason::SourceChanged)
        );
    }

    #[test]
    fn incomplete_extra_and_intervening_records_refuse() {
        let fixture = Fixture::new();
        fixture.append(&BASELINE[87..BASELINE.len() - 1]);
        assert_eq!(
            fixture.verify(),
            RawVerdict::Pending(RefusalReason::IncompleteRecord)
        );

        let fixture = Fixture::new();
        fixture.append_baseline();
        fixture.append(b"{\"type\":\"event_msg\",\"payload\":{}}\n");
        assert_eq!(
            fixture.verify(),
            RawVerdict::Pending(RefusalReason::PostTailSequence)
        );

        let fixture = Fixture::new();
        fixture.append(b"{\"type\":\"event_msg\",\"payload\":{}}\n");
        fixture.append_baseline();
        assert_eq!(
            fixture.verify(),
            RawVerdict::Pending(RefusalReason::PostTailSequence)
        );

        let fixture = Fixture::new();
        fixture.append_baseline();
        fixture.append_baseline();
        assert_eq!(
            fixture.verify(),
            RawVerdict::Pending(RefusalReason::PostTailSequence)
        );
    }

    #[test]
    fn ambiguous_json_keys_and_extra_user_content_refuse() {
        let fixture = Fixture::new();
        let mut line = String::from_utf8(BASELINE[87..].to_vec()).unwrap();
        line = line.replacen(
            "\"role\":\"user\"",
            "\"role\":\"assistant\",\"role\":\"user\"",
            1,
        );
        fixture.append(line.as_bytes());
        assert_eq!(
            fixture.verify(),
            RawVerdict::Unknown(RefusalReason::MalformedSource)
        );

        let fixture = Fixture::new();
        let mut value: Value = serde_json::from_slice(&BASELINE[87..]).unwrap();
        value["payload"]["content"][0]["image_url"] = Value::String("hidden".to_owned());
        let mut line = serde_json::to_vec(&value).unwrap();
        line.push(b'\n');
        fixture.append(&line);
        assert_eq!(
            fixture.verify(),
            RawVerdict::Unknown(RefusalReason::UnsupportedRecord)
        );
    }

    #[test]
    fn selected_adapter_fields_and_read_race_refuse() {
        let fixture = Fixture::new();
        fixture.append_baseline();
        let mut adapter = fixture.adapter();
        adapter.turn_id = "age319-synthetic-session:byte:88";
        assert_eq!(
            fixture.pinned.verify(
                &fixture.prepared(),
                &adapter,
                ExactInputTransform::ExactUtf8
            ),
            RawVerdict::Pending(RefusalReason::AdapterMismatch)
        );
        adapter.turn_id = "age319-synthetic-session:byte:87";
        adapter.source_bytes_examined = 546;
        assert_eq!(
            fixture.pinned.verify(
                &fixture.prepared(),
                &adapter,
                ExactInputTransform::ExactUtf8
            ),
            RawVerdict::Pending(RefusalReason::AdapterMismatch)
        );
        adapter.source_bytes_examined = 547;
        adapter.page_turn_count = 2;
        assert_eq!(
            fixture.pinned.verify(
                &fixture.prepared(),
                &adapter,
                ExactInputTransform::ExactUtf8
            ),
            RawVerdict::Pending(RefusalReason::AdapterMismatch)
        );
        assert_eq!(
            fixture.pinned.verify_after_read(
                &fixture.prepared(),
                &fixture.adapter(),
                ExactInputTransform::ExactUtf8,
                || fixture.append(b"{}\n")
            ),
            RawVerdict::Pending(RefusalReason::SourceChanged)
        );
    }

    #[test]
    fn unsupported_format_symlink_and_size_bounds_refuse() {
        let fixture = Fixture::new();
        assert!(matches!(
            PinnedCodexRollout::capture(&fixture.path, SESSION, "unreviewed-version"),
            Err(Refusal::Unknown(RefusalReason::UnsupportedFormat))
        ));
        let link = fixture.path.with_extension("link");
        std::os::unix::fs::symlink(&fixture.path, &link).unwrap();
        assert!(matches!(
            PinnedCodexRollout::capture(&link, SESSION, CODEX_ROLLOUT_FORMAT),
            Err(Refusal::Pending(RefusalReason::SourceUnavailable))
        ));

        let fixture = Fixture::new();
        let oversized = File::options().write(true).open(&fixture.path).unwrap();
        oversized.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(matches!(
            PinnedCodexRollout::capture(&fixture.path, SESSION, CODEX_ROLLOUT_FORMAT),
            Err(Refusal::Unknown(RefusalReason::SourceTooLarge))
        ));

        let fixture = Fixture::new();
        fixture.append_user(&"x".repeat(MAX_BODY_BYTES + 1));
        assert_eq!(
            fixture.verify(),
            RawVerdict::Unknown(RefusalReason::SourceTooLarge)
        );

        let fixture = Fixture::new();
        fixture.append(&vec![b' '; MAX_RECORD_BYTES + 1]);
        fixture.append(b"\n");
        assert_eq!(
            fixture.verify(),
            RawVerdict::Unknown(RefusalReason::SourceTooLarge)
        );
    }
}
