use crate::provider_registry::{ProviderRegistry, ProviderRegistryHandle};
use crate::session_metadata::{SessionStorageType, TranscriptLookupMode};
use chrono::{DateTime, Utc};
use oulipoly_provider::generated::Artifact;
use oulipoly_state::StateDb;
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};

pub const S7A_NEUTRAL_SETTINGS_ID: &str = "s7a-neutral-settings";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProviderIdentity {
    pub model_name: String,
    pub provider_name: String,
    pub provider_instance_id: Option<String>,
    pub settings_id: String,
}

#[derive(Debug, Clone)]
pub struct SessionProviderLocateRequest<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub session_id: &'a str,
    pub lookup_mode: TranscriptLookupMode,
    pub effective_cwd: Option<&'a Path>,
    pub purpose: Option<&'a str>,
    pub tail_bytes_hint: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProviderLocatedTranscript {
    pub path: std::path::PathBuf,
    pub storage_classification: SessionStorageType,
    pub require_existing_observed: bool,
    pub format_id: Option<String>,
    pub source_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionProviderReadTurnsRequest<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub session_id: &'a str,
    pub effective_cwd: Option<&'a Path>,
}

#[derive(Debug, Clone)]
pub struct SessionProviderCaptureRequest<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub invocation_uuid: &'a str,
    pub effective_cwd: Option<&'a Path>,
}

#[derive(Debug, Clone)]
pub struct SessionProviderEnumerateRequest<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub limit: Option<u64>,
    pub cursor: Option<&'a str>,
    pub include_cwd: bool,
    pub include_turn_count: bool,
    pub since_unix_ms: Option<u64>,
    pub effective_cwd: Option<&'a Path>,
}

#[derive(Debug, Clone)]
pub struct SessionProviderLifecycleContext<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub invocation_uuid: &'a str,
    pub invocation_row_id: i64,
    pub effective_cwd: Option<&'a Path>,
    pub pinned_target: Option<&'a str>,
    pub start_bound_provider_session_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionProviderReadTurnsResult {
    pub turns: Vec<SessionProviderTurn>,
    pub turn_count: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionProviderTurn {
    pub session_id: String,
    pub turn_id: String,
    pub timestamp: DateTime<Utc>,
    pub role: String,
    pub parent_turn_id: Option<String>,
    pub is_sidechain: bool,
    pub is_compaction_boundary: bool,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionProviderCaptureResult {
    pub provider_session_id: Option<String>,
    pub state: Option<Value>,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProviderEnumerateSource {
    pub kind: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProviderEnumerateEntry {
    pub provider_session_id: String,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
    pub created_unix_ms: Option<u64>,
    pub updated_unix_ms: Option<u64>,
    pub turn_count: Option<u64>,
    pub source: SessionProviderEnumerateSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProviderEnumerateResult {
    pub sessions: Vec<SessionProviderEnumerateEntry>,
    pub complete: bool,
    pub next_cursor: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProviderError {
    token: String,
    message: String,
}

impl SessionProviderError {
    pub(crate) fn new(token: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            message: message.into(),
        }
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Display for SessionProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.token, self.message)
    }
}

impl std::error::Error for SessionProviderError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoRefProofOutput {
    pub lifecycle_stderr: Vec<u8>,
    pub lifecycle: crate::services::SessionLifecycleOutput,
}

pub struct NoRefProofRequest<'a> {
    pub state: &'a StateDb,
    pub registry: Option<ProviderRegistryHandle>,
    pub model_name: &'a str,
    pub provider_name: &'a str,
    pub session_id: &'a str,
    pub invocation_row_id: i64,
    pub invocation_uuid: &'a str,
}
