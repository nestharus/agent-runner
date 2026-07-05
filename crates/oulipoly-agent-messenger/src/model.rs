//! ## Declared roles
//!
//! - accessor
//!
//! Returned-artifact public data contracts and crate-internal carriers.
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-agent-messenger/src/model.rs
//!     role: intrinsic-surface
//!     Domain: agent_messenger_returned_artifact_contract
//!     Owns:
//!       - ReturnRequest and ReturnSource library request contract
//!       - StoreAddress returned-artifact address contract
//!       - ReturnedArtifact, ReturnedArtifactMeta, ReturnedArtifactRecord, ReturnedArtifactRef receipt contracts
//!       - ListReturnedRequest and ShowReturnedRequest lookup contracts
//!       - crate-internal ReturnPayload, ReturnLookup, and StoredReturnPayload carriers
//! ```

use crate::ReturnName;
use chrono::{DateTime, Utc};
use oulipoly_agent_scratchpad::ScratchpadName;
use oulipoly_agent_store::{ArtifactKey, PutReceipt};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ReturnRequest {
    pub db_path: PathBuf,
    pub invocation_uuid: Uuid,
    pub name: ReturnName,
    pub source: ReturnSource,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub return_channel: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum ReturnSource {
    Scratchpad {
        name: ScratchpadName,
        version: Option<u64>,
    },
    InlineBytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreAddress {
    pub workflow_run_id: String,
    pub artifact_name: String,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReturnedArtifactSource {
    Scratchpad { name: String, version: u64 },
    InlineBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnedArtifact {
    pub schema_version: u32,
    pub version_id: String,
    pub name: String,
    pub store_address: StoreAddress,
    pub sha256: String,
    pub content_len: u64,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub source: ReturnedArtifactSource,
    pub producer_invocation_uuid: Uuid,
    pub returned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnedArtifactMeta {
    pub version_id: String,
    pub name: String,
    pub store_address: StoreAddress,
    pub sha256: String,
    pub content_len: u64,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub source: ReturnedArtifactSource,
    pub producer_invocation_uuid: Uuid,
    pub returned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnedArtifactRecord {
    pub meta: ReturnedArtifactMeta,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnedArtifactRef {
    pub version_id: String,
    pub name: String,
    pub store_address: StoreAddress,
    pub sha256: String,
    pub content_len: u64,
    pub format_hint: Option<String>,
    pub verdict_line: Option<String>,
    pub source: ReturnedArtifactSource,
    pub producer_invocation_uuid: Uuid,
    pub returned_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ListReturnedRequest {
    pub db_path: PathBuf,
    pub invocation_uuid: Uuid,
    pub name: Option<ReturnName>,
}

#[derive(Debug, Clone)]
pub enum ShowReturnedRequest {
    VersionId {
        db_path: PathBuf,
        version_id: String,
    },
    Address {
        db_path: PathBuf,
        invocation_uuid: Uuid,
        name: ReturnName,
        version: Option<u64>,
    },
}

pub(crate) struct ReturnPayload {
    pub(crate) content: Vec<u8>,
    pub(crate) format_hint: Option<String>,
    pub(crate) verdict_line: Option<String>,
    pub(crate) source: ReturnedArtifactSource,
}

pub(crate) struct ReturnLookup {
    pub(crate) db_path: PathBuf,
    pub(crate) key: ArtifactKey,
    pub(crate) version: Option<u64>,
}

pub(crate) struct StoredReturnPayload {
    pub(crate) receipt: PutReceipt,
    pub(crate) source: ReturnedArtifactSource,
}
