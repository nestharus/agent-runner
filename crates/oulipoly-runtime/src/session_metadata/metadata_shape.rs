//! ## Declared roles
//! mapper
//!
//! Assembly of the public SessionMetadata DTO from resolved parts.

use super::{SessionMetadata, SessionStorageType, TranscriptState};
use std::path::PathBuf;

pub(super) struct SessionMetadataParts {
    pub session_id: String,
    pub chain_id: String,
    pub active_segment_id: i64,
    pub provider_name: String,
    pub storage_type: SessionStorageType,
    pub jsonl_path: PathBuf,
    pub workspace_root: PathBuf,
    pub mutable: bool,
}

pub(super) fn session_metadata_from_parts(parts: SessionMetadataParts) -> SessionMetadata {
    SessionMetadata {
        session_id: parts.session_id,
        chain_id: parts.chain_id,
        active_segment_id: parts.active_segment_id,
        provider_name: parts.provider_name,
        storage_type: parts.storage_type,
        jsonl_path: parts.jsonl_path,
        workspace_root: parts.workspace_root,
        transcript_state: TranscriptState::Available,
        mutable: parts.mutable,
    }
}
