//! The subset of the pinned ACP v2 draft schema this client uses.
//!
//! Field names and required/optional status follow `schema/v2/schema.json`
//! at [`crate::pin::SCHEMA_COMMIT`]. Fields the client neither sends nor
//! reads are omitted, and unknown fields are ignored on input.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Method names, from `schema/v2/meta.json` at the pinned commit.
pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const SESSION_NEW: &str = "session/new";
    pub const SESSION_RESUME: &str = "session/resume";
    pub const SESSION_PROMPT: &str = "session/prompt";
    pub const SESSION_UPDATE: &str = "session/update";
}

/// JSON-RPC "Method not found".
pub const METHOD_NOT_FOUND: i64 = -32601;

/// `Implementation`: `name` and `version` are required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

/// `InitializeRequest`. `protocolVersion` and `info` are required.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequest {
    pub protocol_version: u16,
    pub info: Implementation,
    pub capabilities: Map<String, Value>,
    #[serde(rename = "_meta")]
    pub meta: Map<String, Value>,
}

/// `InitializeResponse`. Only `protocolVersion` is read before the version
/// check, so that a v1 response (whose other fields differ) is still
/// recognised as v1 rather than as a malformed response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponseVersion {
    pub protocol_version: u16,
}

/// `InitializeResponse`, read once the version is known to be 2.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub protocol_version: u16,
    pub info: Implementation,
    #[serde(default)]
    pub capabilities: AgentCapabilities,
    #[serde(rename = "_meta", default)]
    pub meta: Option<Map<String, Value>>,
}

/// `AgentCapabilities`. Omitted or `null` `session` means the agent does
/// not support the `session/*` surface; `{}` means the baseline methods,
/// including `session/resume`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentCapabilities {
    #[serde(default)]
    pub session: Option<Value>,
}

/// `NewSessionRequest`. `cwd` (absolute path) is required.
#[derive(Debug, Clone, Serialize)]
pub struct NewSessionRequest {
    pub cwd: String,
}

/// `NewSessionResponse`. `sessionId` is required.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResponse {
    pub session_id: String,
}

/// `ResumeSessionRequest`. Omitting `replayFrom` asks the agent to resume
/// without replaying history.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeSessionRequest {
    pub session_id: String,
    pub cwd: String,
}

/// `ContentBlock::Text`; every agent must accept it in prompts.
#[derive(Debug, Clone, Serialize)]
pub struct TextBlock {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

/// `PromptRequest`. `sessionId` and `prompt` are required.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub session_id: String,
    pub prompt: Vec<TextBlock>,
    #[serde(rename = "_meta")]
    pub meta: Map<String, Value>,
}

/// `PromptResponse`: acknowledges insertion. `messageId` is required and
/// non-null; omission and `null` are both invalid.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResponse {
    pub message_id: String,
    #[serde(rename = "_meta", default)]
    pub meta: Option<Map<String, Value>>,
}

/// `UpdateSessionNotification` (`session/update` params).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSessionNotification {
    pub session_id: String,
    pub update: Value,
}
