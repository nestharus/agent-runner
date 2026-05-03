use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{
    ModelConfigRepository, ProviderConfigSource, SessionStorage, SessionsConfigSource,
};
use crate::process::ProcessRunner;
use crate::sessions::locate_transcript_with_runner;
use crate::state::{ResumeError, SessionChainRepository};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalRecord {
    pub session_id: String,
    pub provider_name: String,
    pub turn_id: String,
    pub role: String,
    pub timestamp: String,
    pub content: Vec<ContentChunk>,
    pub source: RecordSource,
    pub unsupported_record: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentChunk {
    pub r#type: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordSource {
    pub storage_type: String,
    pub jsonl_path: PathBuf,
    pub line: u64,
    pub byte_start: u64,
    pub byte_end: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStorageType {
    ClaudeCode,
    CodexSession,
    Other,
}

impl SessionStorageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionStorageType::ClaudeCode => "claude_code",
            SessionStorageType::CodexSession => "codex_session",
            SessionStorageType::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportSessionMetadata {
    pub session_id: String,
    pub chain_id: String,
    pub provider_name: String,
    pub storage_type: SessionStorageType,
    pub jsonl_path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum ExportError {
    InvalidSessionId {
        input: String,
    },
    SessionNotFound {
        input: String,
    },
    AmbiguousSession {
        input: String,
    },
    UnsupportedStorage {
        provider_name: String,
        reason: String,
    },
    MalformedTranscript {
        path: PathBuf,
        line: u64,
        reason: String,
    },
    Operational {
        message: String,
    },
}

pub fn resolve_export_session_metadata_with_deps(
    chain_repo: &dyn SessionChainRepository,
    model_repo: &dyn ModelConfigRepository,
    provider_source: &dyn ProviderConfigSource,
    sessions_source: &dyn SessionsConfigSource,
    locator_runner: &dyn ProcessRunner,
    session_id: &str,
) -> Result<ExportSessionMetadata, ExportError> {
    let models = model_repo
        .load_models()
        .map_err(|message| ExportError::Operational { message })?;
    let providers_cfg = provider_source.load_providers().unwrap_or_default();
    let sessions_cfg = sessions_source.load_sessions().unwrap_or_default();
    let facts = chain_repo
        .resolve_resume_facts(session_id, None)
        .map_err(map_resume_error)?;

    if let Some(model_name) = &facts.inferred_model_name {
        let model = models
            .get(model_name)
            .ok_or_else(|| ExportError::Operational {
                message: format!("unknown model referenced by session chain: {model_name}"),
            })?;
        if !model
            .providers
            .iter()
            .any(|provider| provider.name == facts.active_provider)
        {
            return Err(ExportError::UnsupportedStorage {
                provider_name: facts.active_provider,
                reason: "session owner is not in the resolved model provider pool".to_string(),
            });
        }
    }

    let provider_entry = providers_cfg.get(&facts.active_provider).ok_or_else(|| {
        ExportError::UnsupportedStorage {
            provider_name: facts.active_provider.clone(),
            reason: "provider is missing from providers.toml".to_string(),
        }
    })?;
    let storage_type = match provider_entry.session_storage.as_ref() {
        Some(SessionStorage::ClaudeCode { .. }) => SessionStorageType::ClaudeCode,
        Some(SessionStorage::Codex { .. }) => SessionStorageType::CodexSession,
        None => {
            return Err(ExportError::UnsupportedStorage {
                provider_name: facts.active_provider,
                reason: "provider has no session_storage configuration".to_string(),
            });
        }
    };

    let jsonl_path = locate_transcript_with_runner(
        &sessions_cfg,
        &facts.active_provider,
        &facts.active_session_id,
        locator_runner,
    )
    .map_err(|message| ExportError::Operational { message })?
    .ok_or_else(|| ExportError::UnsupportedStorage {
        provider_name: facts.active_provider.clone(),
        reason: "provider has no transcript_locator configuration".to_string(),
    })?;

    Ok(ExportSessionMetadata {
        session_id: facts.active_session_id,
        chain_id: facts.chain_id,
        provider_name: facts.active_provider,
        storage_type,
        jsonl_path,
    })
}

fn map_resume_error(err: ResumeError) -> ExportError {
    match err {
        ResumeError::InvalidUuid { input } => ExportError::InvalidSessionId { input },
        ResumeError::NoChainFound { input } => ExportError::SessionNotFound { input },
        ResumeError::Ambiguous { input, .. } => ExportError::AmbiguousSession { input },
        ResumeError::ProviderModelMismatch {
            active_provider, ..
        } => ExportError::UnsupportedStorage {
            provider_name: active_provider,
            reason: "session owner is not in the resolved model provider pool".to_string(),
        },
        ResumeError::UnknownModel { model_name } => ExportError::Operational {
            message: format!("unknown model referenced by session chain: {model_name}"),
        },
        ResumeError::ActiveSegmentMissing { chain_id } => ExportError::Operational {
            message: format!("no active segment found for chain {chain_id}"),
        },
        ResumeError::ProviderNotConfigured { provider } => ExportError::UnsupportedStorage {
            provider_name: provider,
            reason: "session owner provider is not configured".to_string(),
        },
        ResumeError::ProviderMissingResume { provider_name } => ExportError::UnsupportedStorage {
            provider_name,
            reason: "session owner provider has no resume configuration".to_string(),
        },
        ResumeError::Db { message } => ExportError::Operational { message },
    }
}

pub fn read_canonical_transcript(
    metadata: &ExportSessionMetadata,
) -> Result<Vec<CanonicalRecord>, ExportError> {
    let bytes = fs::read(&metadata.jsonl_path).map_err(|e| ExportError::Operational {
        message: format!(
            "failed to read transcript {}: {e}",
            metadata.jsonl_path.display()
        ),
    })?;
    read_canonical_transcript_from_bytes(metadata, &bytes)
}

pub fn read_canonical_transcript_from_bytes(
    metadata: &ExportSessionMetadata,
    bytes: &[u8],
) -> Result<Vec<CanonicalRecord>, ExportError> {
    match metadata.storage_type {
        SessionStorageType::ClaudeCode => parse_claude_code_jsonl_bytes(metadata, bytes),
        SessionStorageType::CodexSession => parse_codex_rollout_jsonl_bytes(metadata, bytes),
        SessionStorageType::Other => Err(ExportError::UnsupportedStorage {
            provider_name: metadata.provider_name.clone(),
            reason: "storage type is other".to_string(),
        }),
    }
}

pub fn canonical_jsonl_bytes(records: &[CanonicalRecord]) -> Result<Vec<u8>, ExportError> {
    let mut out = Vec::new();
    for record in records {
        let line = serde_json::to_string(record).map_err(|e| ExportError::Operational {
            message: format!("failed to serialize canonical record: {e}"),
        })?;
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

pub fn parse_claude_code_jsonl(
    metadata: &ExportSessionMetadata,
) -> Result<Vec<CanonicalRecord>, ExportError> {
    let bytes = fs::read(&metadata.jsonl_path).map_err(|e| ExportError::Operational {
        message: format!(
            "failed to read transcript {}: {e}",
            metadata.jsonl_path.display()
        ),
    })?;
    parse_claude_code_jsonl_bytes(metadata, &bytes)
}

fn parse_claude_code_jsonl_bytes(
    metadata: &ExportSessionMetadata,
    bytes: &[u8],
) -> Result<Vec<CanonicalRecord>, ExportError> {
    let lines = scan_jsonl_bytes(bytes, &metadata.jsonl_path)?;
    let mut records = Vec::new();
    let mut latest_compaction_boundary = None;

    for line in lines {
        let Some(session_id) = line.value.get("sessionId").and_then(Value::as_str) else {
            // Some provider bookkeeping records intentionally omit session_id; a present
            // session_id that differs from metadata.session_id returns
            // ExportError::MalformedTranscript below.
            continue;
        };
        if session_id != metadata.session_id {
            return Err(ExportError::MalformedTranscript {
                path: metadata.jsonl_path.clone(),
                line: line.line,
                reason: format!(
                    "transcript sessionId {session_id} does not match requested session {}",
                    metadata.session_id
                ),
            });
        }

        let Some(native_type) = line.value.get("type").and_then(Value::as_str) else {
            // Records with a matching session_id but no native_type are not transcript turns.
            continue;
        };

        let turn_id = required_string(&line, "uuid", &metadata.jsonl_path)?;
        let timestamp = required_timestamp(&line, &metadata.jsonl_path)?;
        let unsupported_record = !matches!(native_type, "user" | "assistant");
        let content = if unsupported_record {
            Vec::new()
        } else {
            extract_claude_content(&line.value)
        };

        let record = CanonicalRecord {
            session_id: metadata.session_id.clone(),
            provider_name: metadata.provider_name.clone(),
            turn_id,
            role: native_type.to_string(),
            timestamp,
            content,
            source: line.source(metadata),
            unsupported_record,
        };

        if line.value.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
            latest_compaction_boundary = Some(records.len());
        }
        records.push(record);
    }

    let records = if let Some(index) = latest_compaction_boundary {
        records.into_iter().skip(index).collect()
    } else {
        records
    };
    validate_timestamp_order(&records, &metadata.jsonl_path)?;
    Ok(records)
}

pub fn parse_codex_rollout_jsonl(
    metadata: &ExportSessionMetadata,
) -> Result<Vec<CanonicalRecord>, ExportError> {
    let bytes = fs::read(&metadata.jsonl_path).map_err(|e| ExportError::Operational {
        message: format!(
            "failed to read transcript {}: {e}",
            metadata.jsonl_path.display()
        ),
    })?;
    parse_codex_rollout_jsonl_bytes(metadata, &bytes)
}

fn parse_codex_rollout_jsonl_bytes(
    metadata: &ExportSessionMetadata,
    bytes: &[u8],
) -> Result<Vec<CanonicalRecord>, ExportError> {
    let lines = scan_jsonl_bytes(bytes, &metadata.jsonl_path)?;
    let mut saw_matching_session_meta = false;
    let mut records = Vec::new();

    for line in lines {
        let Some(native_type) = line.value.get("type").and_then(Value::as_str) else {
            continue;
        };
        match native_type {
            "session_meta"
                if line
                    .value
                    .get("payload")
                    .and_then(|payload| payload.get("id"))
                    .and_then(Value::as_str)
                    == Some(metadata.session_id.as_str()) =>
            {
                saw_matching_session_meta = true;
            }
            "session_meta" => {}
            "response_item" => {
                let Some(payload) = line.value.get("payload") else {
                    continue;
                };
                if payload.get("type").and_then(Value::as_str) != Some("message") {
                    continue;
                }
                let Some(role @ ("user" | "assistant")) =
                    payload.get("role").and_then(Value::as_str)
                else {
                    continue;
                };
                let timestamp = required_timestamp(&line, &metadata.jsonl_path)?;
                let turn_id = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{}:{}", metadata.jsonl_path.display(), line.line));
                records.push(CanonicalRecord {
                    session_id: metadata.session_id.clone(),
                    provider_name: metadata.provider_name.clone(),
                    turn_id,
                    role: role.to_string(),
                    timestamp,
                    content: extract_content_chunks(payload.get("content")),
                    source: line.source(metadata),
                    unsupported_record: false,
                });
            }
            _ => {}
        }
    }

    if !saw_matching_session_meta {
        return Err(ExportError::MalformedTranscript {
            path: metadata.jsonl_path.clone(),
            line: 0,
            reason: format!(
                "transcript is missing matching codex session_meta for {}",
                metadata.session_id
            ),
        });
    }

    validate_timestamp_order(&records, &metadata.jsonl_path)?;
    Ok(records)
}

#[derive(Debug)]
struct SourceLine {
    line: u64,
    byte_start: u64,
    byte_end: u64,
    sha256: String,
    value: Value,
}

impl SourceLine {
    fn source(&self, metadata: &ExportSessionMetadata) -> RecordSource {
        RecordSource {
            storage_type: metadata.storage_type.as_str().to_string(),
            jsonl_path: metadata.jsonl_path.clone(),
            line: self.line,
            byte_start: self.byte_start,
            byte_end: self.byte_end,
            sha256: self.sha256.clone(),
        }
    }
}

fn scan_jsonl_bytes(bytes: &[u8], path: &Path) -> Result<Vec<SourceLine>, ExportError> {
    let mut out = Vec::new();
    let mut line_no = 1_u64;
    let mut offset = 0_usize;

    while offset < bytes.len() {
        let start = offset;
        while offset < bytes.len() && bytes[offset] != b'\n' {
            offset += 1;
        }
        let mut end = offset;
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        let line_bytes = &bytes[start..end];
        if !line_bytes.iter().all(u8::is_ascii_whitespace) {
            let text =
                std::str::from_utf8(line_bytes).map_err(|e| ExportError::MalformedTranscript {
                    path: path.to_path_buf(),
                    line: line_no,
                    reason: format!("transcript line is not UTF-8: {e}"),
                })?;
            let value = serde_json::from_str::<Value>(text).map_err(|e| {
                ExportError::MalformedTranscript {
                    path: path.to_path_buf(),
                    line: line_no,
                    reason: format!("transcript line is not valid JSON: {e}"),
                }
            })?;
            out.push(SourceLine {
                line: line_no,
                byte_start: start as u64,
                byte_end: end as u64,
                sha256: sha256_hex(line_bytes),
                value,
            });
        }

        if offset < bytes.len() && bytes[offset] == b'\n' {
            offset += 1;
        }
        line_no += 1;
    }

    Ok(out)
}

fn required_string(line: &SourceLine, field: &str, path: &Path) -> Result<String, ExportError> {
    line.value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ExportError::MalformedTranscript {
            path: path.to_path_buf(),
            line: line.line,
            reason: format!("transcript line is missing required {field}"),
        })
}

fn required_timestamp(line: &SourceLine, path: &Path) -> Result<String, ExportError> {
    let timestamp = required_string(line, "timestamp", path)?;
    DateTime::parse_from_rfc3339(&timestamp).map_err(|e| ExportError::MalformedTranscript {
        path: path.to_path_buf(),
        line: line.line,
        reason: format!("transcript timestamp is not RFC3339: {e}"),
    })?;
    Ok(timestamp)
}

fn validate_timestamp_order(records: &[CanonicalRecord], path: &Path) -> Result<(), ExportError> {
    let mut previous: Option<DateTime<Utc>> = None;
    for record in records {
        let current = DateTime::parse_from_rfc3339(&record.timestamp)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| ExportError::MalformedTranscript {
                path: path.to_path_buf(),
                line: record.source.line,
                reason: format!("transcript timestamp is not RFC3339: {e}"),
            })?;
        if let Some(previous) = previous
            && current < previous
        {
            return Err(ExportError::MalformedTranscript {
                path: path.to_path_buf(),
                line: record.source.line,
                reason: "transcript timestamps are not in provider order".to_string(),
            });
        }
        previous = Some(current);
    }
    Ok(())
}

fn extract_claude_content(value: &Value) -> Vec<ContentChunk> {
    if let Some(message) = value.get("message") {
        if let Some(text) = message.as_str() {
            return vec![text_chunk(text)];
        }
        if let Some(content) = message.get("content") {
            return extract_content_chunks(Some(content));
        }
    }
    extract_content_chunks(value.get("content"))
}

fn extract_content_chunks(value: Option<&Value>) -> Vec<ContentChunk> {
    match value {
        Some(Value::String(text)) => vec![text_chunk(text)],
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                if let Some(text) = item.as_str() {
                    return text_chunk(text);
                }
                let item_type = item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("text")
                    .to_string();
                let text = item
                    .get("text")
                    .or_else(|| item.get("content"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                ContentChunk {
                    r#type: canonical_chunk_type(&item_type).to_string(),
                    text,
                }
            })
            .collect(),
        Some(Value::Object(_)) => value
            .and_then(|object| object.get("text").and_then(Value::as_str))
            .map(|text| vec![text_chunk(text)])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn canonical_chunk_type(native_type: &str) -> &str {
    match native_type {
        "input_text" | "output_text" => "text",
        other => other,
    }
}

fn text_chunk(text: &str) -> ContentChunk {
    ContentChunk {
        r#type: "text".to_string(),
        text: Some(text.to_string()),
    }
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
