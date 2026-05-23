//! ## Declared roles
//!
//! `parser`, `validator`, `mapper`, `formatter`, `accessor`, `predicate`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model.rs
//!     role: intrinsic-surface
//!     Domain: model_provider_session_config
//!     Owns:
//!       - provider resume strategy (resume command shape)
//!       - session_capture configuration (stdout parse, jsonl tail, none)
//!       - session_storage configuration (locator routing)
//!       - prompt mode (positional, file, stdin) and provider derivation
//! ```

use crate::claude_tool_filter::{ClaudeToolFilterShape, validate_proxy_claude_filter_shape};
use crate::provider_implementation_ref::{
    ProviderImplementationRef, ProviderImplementationRefError,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InvocationMode {
    #[default]
    Headless,
    Proxy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable identifier for this provider (the CLI account / binary).
    /// Used as the key for per-account state like quota tracking.
    /// If omitted in TOML, derived from command+args via `derive_provider_name`.
    pub name: String,
    #[serde(default)]
    pub command: String,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<ResumeStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_capture: Option<SessionCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_acceptance: Option<ResumeAcceptanceRules>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_storage: Option<SessionStorage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_restrictions: Option<ToolRestrictions>,
    #[serde(default)]
    pub invocation_mode: InvocationMode,
}

impl ProviderConfig {
    /// Build a ProviderConfig, auto-deriving `name` from command+args.
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        let command = command.into();
        let name = derive_provider_name(&command, &args);
        Self {
            name,
            command,
            args,
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: InvocationMode::Headless,
        }
    }

    pub fn model_provider(name: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            command: String::new(),
            args,
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: InvocationMode::Headless,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ToolRestrictions {
    pub kind: ToolRestrictionKind,
    #[serde(default, skip_serializing_if = "ClaudeRestrictions::is_empty")]
    pub claude: ClaudeRestrictions,
    #[serde(default, skip_serializing_if = "CodexRestrictions::is_empty")]
    pub codex: CodexRestrictions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolRestrictionKind {
    #[default]
    Claude,
    Codex,
}

impl std::fmt::Display for ToolRestrictionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => f.write_str("claude"),
            Self::Codex => f.write_str("codex"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeRestrictions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disallowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disable_slash_commands: bool,
}

impl ClaudeRestrictions {
    pub fn is_empty(&self) -> bool {
        self.disallowed_tools.is_empty()
            && self.allowed_tools.is_empty()
            && !self.disable_slash_commands
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexRestrictions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_pairs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_features: Vec<String>,
}

impl CodexRestrictions {
    pub fn is_empty(&self) -> bool {
        self.config_pairs.is_empty() && self.disabled_features.is_empty()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeStrategy {
    pub kind: ResumeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subcommand: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResumeKind {
    Flag,
    Subcommand,
}

impl ResumeStrategy {
    pub fn validate(&self) -> Result<(), String> {
        match self.kind {
            ResumeKind::Flag => {
                if self.flag.is_none() {
                    return Err("resume.kind = flag requires `flag`".into());
                }
                if self.subcommand.is_some() {
                    return Err("resume.kind = flag does not allow `subcommand`".into());
                }
                Ok(())
            }
            ResumeKind::Subcommand => {
                if !matches!(self.subcommand.as_ref(), Some(parts) if !parts.is_empty()) {
                    return Err("resume.kind = subcommand requires non-empty `subcommand`".into());
                }
                if self.flag.is_some() {
                    return Err("resume.kind = subcommand does not allow `flag`".into());
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeAcceptanceRules {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_output_patterns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_output_patterns: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCapture {
    pub kind: SessionCaptureKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readback_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_flag: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionCaptureKind {
    None,
    ForcedFlagVerified,
    StdoutJsonEvent,
}

impl SessionCapture {
    /// Per-kind required fields per `tmp/01-pr-c-contract.md`. Catches
    /// malformed configs at load time instead of letting them surface
    /// as silent capture failures at execution time (V10 — observable).
    pub fn validate(&self) -> Result<(), String> {
        match self.kind {
            SessionCaptureKind::None => Ok(()),
            SessionCaptureKind::ForcedFlagVerified => {
                if self.flag.is_none() {
                    return Err(
                        "session_capture.kind = forced_flag_verified requires `flag`".into(),
                    );
                }
                Ok(())
            }
            SessionCaptureKind::StdoutJsonEvent => {
                if self.json_flag.is_none() {
                    return Err(
                        "session_capture.kind = stdout_json_event requires `json_flag`".into(),
                    );
                }
                if self.last_message_flag.is_none() {
                    return Err(
                        "session_capture.kind = stdout_json_event requires `last_message_flag`"
                            .into(),
                    );
                }
                if self.event_type.is_none() {
                    return Err(
                        "session_capture.kind = stdout_json_event requires `event_type`".into(),
                    );
                }
                if self.event_id_path.is_none() {
                    return Err(
                        "session_capture.kind = stdout_json_event requires `event_id_path`".into(),
                    );
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionStorage {
    Script {
        cwd_script: String,
        #[serde(default)]
        transcript_script: Option<String>,
        #[serde(default)]
        storage_type: Option<ScriptSessionStorageType>,
    },
    ClaudeCode {
        projects_dir: PathBuf,
    },
    Codex {
        sessions_dir: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScriptSessionStorageType {
    ClaudeCode,
    CodexSession,
}

impl SessionStorage {
    pub fn expand_tilde(self) -> Self {
        match self {
            SessionStorage::Script {
                cwd_script,
                transcript_script,
                storage_type,
            } => SessionStorage::Script {
                cwd_script,
                transcript_script,
                storage_type,
            },
            SessionStorage::ClaudeCode { projects_dir } => SessionStorage::ClaudeCode {
                projects_dir: expand_leading_tilde(projects_dir),
            },
            SessionStorage::Codex { sessions_dir } => SessionStorage::Codex {
                sessions_dir: expand_leading_tilde(sessions_dir),
            },
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            SessionStorage::Script {
                cwd_script,
                transcript_script,
                storage_type,
            } => {
                if cwd_script.trim().is_empty() {
                    return Err("session_storage.kind = script requires `cwd_script`".into());
                }
                if transcript_script
                    .as_ref()
                    .is_some_and(|script| script.trim().is_empty())
                {
                    return Err(
                        "session_storage.kind = script requires non-empty `transcript_script`"
                            .into(),
                    );
                }
                if transcript_script.is_some() != storage_type.is_some() {
                    return Err(
                        "session_storage.kind = script requires `transcript_script` and `storage_type` together"
                            .into(),
                    );
                }
            }
            SessionStorage::ClaudeCode { projects_dir } => {
                if projects_dir.as_os_str().is_empty() {
                    return Err("session_storage.kind = claude_code requires `projects_dir`".into());
                }
            }
            SessionStorage::Codex { sessions_dir } => {
                if sessions_dir.as_os_str().is_empty() {
                    return Err("session_storage.kind = codex requires `sessions_dir`".into());
                }
            }
        }
        Ok(())
    }

    pub fn cwd_script(&self) -> String {
        match self {
            SessionStorage::Script { cwd_script, .. } => cwd_script.clone(),
            SessionStorage::ClaudeCode { projects_dir } => {
                format!(
                    "claude-code-cwd {}",
                    shell_word(&projects_dir.display().to_string())
                )
            }
            SessionStorage::Codex { sessions_dir } => {
                format!(
                    "codex-cwd {}",
                    shell_word(&sessions_dir.display().to_string())
                )
            }
        }
    }

    pub fn transcript_script(&self) -> Option<&str> {
        match self {
            SessionStorage::Script {
                transcript_script, ..
            } => transcript_script.as_deref(),
            _ => None,
        }
    }

    pub fn script_storage_type(&self) -> Option<ScriptSessionStorageType> {
        match self {
            SessionStorage::Script { storage_type, .. } => *storage_type,
            SessionStorage::ClaudeCode { .. } => Some(ScriptSessionStorageType::ClaudeCode),
            SessionStorage::Codex { .. } => Some(ScriptSessionStorageType::CodexSession),
        }
    }
}

fn shell_word(input: &str) -> String {
    if input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~'))
    {
        return input.to_string();
    }
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

fn expand_leading_tilde(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    let Some(home) = dirs::home_dir() else {
        return path;
    };
    if raw == "~" {
        return home;
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    path
}

/// Derive a provider name from a command + args vector.
///
/// The heuristic picks the first token that looks like an executable name —
/// skipping flags, `env` and its flag arguments (e.g. the VAR in `-u VAR`),
/// and env-var assignments of the form `FOO=bar`. Returns the command string
/// itself as a last-resort fallback.
pub fn derive_provider_name(command: &str, args: &[String]) -> String {
    let command_tokens = crate::providers::shell_split(command);
    let mut all: Vec<&str> = Vec::with_capacity(command_tokens.len() + args.len());
    if command_tokens.is_empty() {
        all.push(command);
    } else {
        all.extend(command_tokens.iter().map(String::as_str));
    }
    for a in args {
        all.push(a.as_str());
    }

    let mut i = 0;
    while i < all.len() {
        let t = all[i];
        // Skip env wrapper itself.
        if t == "env" || t.ends_with("/env") {
            i += 1;
            continue;
        }
        // Skip flags (and consume their value arg for env-style single-char flags).
        if let Some(rest) = t.strip_prefix("--") {
            if rest.is_empty() {
                i += 1;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(rest) = t.strip_prefix('-') {
            // env's `-u VAR` / `-e VAR` take a value argument (the var name).
            if matches!(rest, "u" | "e" | "S") {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        // Skip env var assignments (FOO=bar).
        if t.contains('=') && t.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            i += 1;
            continue;
        }
        return t.to_string();
    }
    command.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub prompt_mode: PromptMode,
    pub providers: Vec<ProviderConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderImplementationRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    Stdin,
    Arg,
}

// --- Input schema ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputType {
    String,
    Integer {
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
    },
    Number {
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
    Boolean,
    Enum {
        options: Vec<String>,
    },
    Array {
        item_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_items: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_items: Option<usize>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputDef {
    pub name: String,
    pub input_type: InputType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default_input: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// CLI flag to pass this input as (e.g. "--size", "--duration")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
}

impl ModelConfig {
    pub fn default_input(&self) -> Option<&InputDef> {
        self.inputs.iter().find(|i| i.default_input)
    }
}

// --- Raw TOML structures for deserialization ---

#[derive(Debug, Clone, Deserialize)]
struct RawModelToml {
    command: Option<String>,
    args: Option<Vec<String>>,
    interactive_args: Option<Vec<String>>,
    resume: Option<ResumeStrategy>,
    prompt_mode: Option<String>,
    session_capture: Option<SessionCapture>,
    resume_acceptance: Option<ResumeAcceptanceRules>,
    session_storage: Option<SessionStorage>,
    #[serde(default)]
    provider: Option<ProviderImplementationRef>,
    providers: Option<Vec<RawProvider>>,
    inputs: Option<Vec<RawInput>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProvider {
    #[serde(default)]
    name: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    interactive_args: Option<Vec<String>>,
    resume: Option<ResumeStrategy>,
    prompt_mode: Option<String>,
    session_capture: Option<SessionCapture>,
    resume_acceptance: Option<ResumeAcceptanceRules>,
    session_storage: Option<SessionStorage>,
    system_prompt_override: Option<String>,
    tool_restrictions: Option<ToolRestrictions>,
    #[serde(default)]
    invocation_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawInput {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    default_input: bool,
    default: Option<toml::Value>,
    description: Option<String>,
    flag: Option<String>,
    // Type-specific fields
    options: Option<Vec<String>>,
    min: Option<f64>,
    max: Option<f64>,
    item_type: Option<String>,
    min_items: Option<usize>,
    max_items: Option<usize>,
}

#[derive(Debug)]
pub enum ModelError {
    Io(std::io::Error),
    Toml(String, String),
    ProviderImplementationRef {
        model: String,
        source: ProviderImplementationRefError,
    },
    InvocationModeIsRootOnly {
        model: String,
        provider: String,
    },
    Other(String, String),
}

impl ModelError {
    fn with_model_name(self, name: &str) -> Self {
        match self {
            Self::Toml(model, err) if model == "<unknown>" => Self::Toml(name.to_string(), err),
            Self::ProviderImplementationRef { model, source } if model == "<unknown>" => {
                Self::ProviderImplementationRef {
                    model: name.to_string(),
                    source,
                }
            }
            Self::InvocationModeIsRootOnly { model, provider } if model == "<unknown>" => {
                Self::InvocationModeIsRootOnly {
                    model: name.to_string(),
                    provider,
                }
            }
            Self::Other(model, err) if model == "<unknown>" => {
                Self::Other(name.to_string(), err.replace("<unknown>", name))
            }
            other => other,
        }
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.to_string().contains(needle)
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error reading model TOML: {err}"),
            Self::Toml(model, err) => write!(f, "toml parse error in model {model}: {err}"),
            Self::ProviderImplementationRef { model, source } => {
                write!(f, "model {model}: {source}")
            }
            Self::InvocationModeIsRootOnly { model, provider } => write!(
                f,
                "model {model}: provider {provider}: invocation_mode is root-only and must move to providers.toml"
            ),
            Self::Other(model, err) => write!(f, "model {model} {err}"),
        }
    }
}

impl std::error::Error for ModelError {}

impl From<std::io::Error> for ModelError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ModelError> for String {
    fn from(value: ModelError) -> Self {
        value.to_string()
    }
}

impl PartialEq for ModelError {
    fn eq(&self, other: &Self) -> bool {
        use ModelError::*;
        match (self, other) {
            (Io(left), Io(right)) => left.to_string() == right.to_string(),
            (Toml(left_model, left_err), Toml(right_model, right_err)) => {
                left_model == right_model && left_err == right_err
            }
            (
                ProviderImplementationRef {
                    model: left_model,
                    source: left_source,
                },
                ProviderImplementationRef {
                    model: right_model,
                    source: right_source,
                },
            ) => left_model == right_model && left_source == right_source,
            (
                InvocationModeIsRootOnly {
                    model: left_model,
                    provider: left_provider,
                },
                InvocationModeIsRootOnly {
                    model: right_model,
                    provider: right_provider,
                },
            ) => left_model == right_model && left_provider == right_provider,
            (Other(left_model, left_err), Other(right_model, right_err)) => {
                left_model == right_model && left_err == right_err
            }
            _ => false,
        }
    }
}

impl Eq for ModelError {}

fn parse_model_toml(text: &str) -> Result<RawModelToml, ModelError> {
    toml::from_str(text).map_err(|err| ModelError::Toml("<unknown>".to_string(), err.to_string()))
}

fn validate_model_toml_against_providers(
    raw: &RawModelToml,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<(), ModelError> {
    validate_raw_model_provider_fields(raw)?;
    let model = apply_model_construction(raw);
    validate_provider_aware_shape("<unknown>", &model, providers)
}

fn validate_raw_model_provider_fields(raw: &RawModelToml) -> Result<(), ModelError> {
    validate_legacy_model_fields(raw)?;
    if let Some(provider) = raw.provider.as_ref() {
        provider
            .validate()
            .map_err(|source| ModelError::ProviderImplementationRef {
                model: "<unknown>".to_string(),
                source,
            })?;
    }
    validate_raw_model_providers(raw)?;
    if let Some(inputs) = raw.inputs.clone() {
        parse_inputs(inputs).map_err(|err| ModelError::Other("<unknown>".to_string(), err))?;
    }
    Ok(())
}

fn apply_model_construction(raw: &RawModelToml) -> ModelConfig {
    construct_model_config_from_raw(raw.clone())
}

fn validate_provider_aware_shape(
    model_name: &str,
    model: &ModelConfig,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<(), ModelError> {
    if let Some(providers) = providers {
        validate_codex_model_arg_overlap(model_name, model, providers)
            .map_err(|err| ModelError::Other(model_name.to_string(), err))?;
        validate_proxy_claude_model_shape(model_name, model, providers)?;
    }
    Ok(())
}

fn construct_model_config_from_raw(raw: RawModelToml) -> ModelConfig {
    let providers = raw
        .providers
        .unwrap_or_default()
        .into_iter()
        .map(raw_provider_to_config)
        .collect();
    let inputs = raw
        .inputs
        .map(parse_inputs)
        .transpose()
        .expect("validated model inputs")
        .unwrap_or_default();
    ModelConfig {
        name: String::new(),
        prompt_mode: raw
            .prompt_mode
            .as_deref()
            .map(crate::providers::parse_prompt_mode)
            .unwrap_or(PromptMode::Stdin),
        providers,
        inputs,
        provider: raw.provider,
    }
}

fn emit_model_toml(model: &ModelConfig) -> String {
    model.to_toml()
}

fn read_model_files(dir: &Path) -> Result<Vec<(PathBuf, String)>, ModelError> {
    list_model_toml_paths(dir)?
        .into_iter()
        .map(|path| {
            let text = read_file_contents(&path)?;
            Ok(pair_path_and_text(path, text))
        })
        .collect()
}

fn read_model_dir_paths(dir: &Path) -> Result<Vec<PathBuf>, ModelError> {
    let mut paths = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
        Err(err) => return Err(ModelError::Io(err)),
    };
    for entry in entries {
        let entry = entry?;
        paths.push(entry.path());
    }
    Ok(paths)
}

fn select_toml_paths(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.retain(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"));
    paths.sort();
    paths
}

fn list_model_toml_paths(dir: &Path) -> Result<Vec<PathBuf>, ModelError> {
    Ok(select_toml_paths(read_model_dir_paths(dir)?))
}

fn read_file_contents(path: &Path) -> Result<String, ModelError> {
    std::fs::read_to_string(path).map_err(ModelError::Io)
}

fn pair_path_and_text(path: PathBuf, text: String) -> (PathBuf, String) {
    (path, text)
}

fn parse_model_files(
    files: Vec<(PathBuf, String)>,
) -> Result<Vec<(PathBuf, RawModelToml)>, ModelError> {
    files
        .into_iter()
        .map(|(path, text)| {
            let raw = parse_model_toml(&text).map_err(|err| match model_name_from_path(&path) {
                Ok(name) => err.with_model_name(&name),
                Err(_) => err,
            })?;
            Ok((path, raw))
        })
        .collect()
}

fn validate_models_against_providers(
    raws: &[(PathBuf, RawModelToml)],
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<(), ModelError> {
    for (path, raw) in raws {
        let name = model_name_from_path(path)?;
        validate_model_toml_against_providers(raw, providers)
            .map_err(|err| err.with_model_name(&name))?;
    }
    Ok(())
}

fn model_name_from_path(path: &Path) -> Result<String, ModelError> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            ModelError::Other(
                "<unknown>".to_string(),
                format!("invalid model filename {}", path.display()),
            )
        })
}

fn validate_legacy_model_fields(raw: &RawModelToml) -> Result<(), ModelError> {
    if raw.command.is_some()
        || raw.args.is_some()
        || raw.interactive_args.is_some()
        || raw.resume.is_some()
        || raw.prompt_mode.is_some()
        || raw.session_capture.is_some()
        || raw.resume_acceptance.is_some()
        || raw.session_storage.is_some()
    {
        return Err(ModelError::Other(
            "<unknown>".to_string(),
            "Old per-provider config detected in <unknown>.toml; keep runtime provider fields in providers.toml and run `agents migrate-config` to repair existing files."
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_raw_model_providers(raw: &RawModelToml) -> Result<(), ModelError> {
    let providers = raw
        .providers
        .as_ref()
        .filter(|providers| !providers.is_empty())
        .ok_or_else(|| {
            ModelError::Other(
                "<unknown>".to_string(),
                "model must declare at least one [[providers]]".to_string(),
            )
        })?;

    for provider in providers {
        let provider_name = raw_provider_name(provider);
        if provider.command.is_some()
            || provider.resume.is_some()
            || provider.prompt_mode.is_some()
            || provider.session_capture.is_some()
            || provider.resume_acceptance.is_some()
            || provider.session_storage.is_some()
        {
            return Err(ModelError::Other(
                "<unknown>".to_string(),
                format!(
                    "Old per-provider config detected in <unknown>.toml provider {provider_name}; keep runtime provider fields in providers.toml and run `agents migrate-config` to repair existing files."
                ),
            ));
        }
        if provider.system_prompt_override.is_some() || provider.tool_restrictions.is_some() {
            return Err(ModelError::Other(
                "<unknown>".to_string(),
                format!(
                    "provider {provider_name}: system_prompt_override and tool_restrictions are root-only provider settings; move them to providers.toml"
                ),
            ));
        }
        if provider.invocation_mode.is_some() {
            return Err(ModelError::InvocationModeIsRootOnly {
                model: "<unknown>".to_string(),
                provider: provider_name,
            });
        }
    }
    Ok(())
}

fn raw_provider_name(provider: &RawProvider) -> String {
    provider.name.clone().unwrap_or_else(|| {
        provider
            .command
            .as_deref()
            .map(|command| derive_provider_name(command, provider.args.as_deref().unwrap_or(&[])))
            .unwrap_or_else(|| "<unknown>".to_string())
    })
}

fn raw_provider_to_config(raw: RawProvider) -> ProviderConfig {
    let args = raw.args.unwrap_or_default();
    let name = raw.name.unwrap_or_else(|| {
        raw.command
            .as_deref()
            .map(|command| derive_provider_name(command, &args))
            .unwrap_or_default()
    });
    ProviderConfig {
        name,
        command: String::new(),
        args,
        interactive_args: raw.interactive_args,
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: InvocationMode::Headless,
    }
}

fn validate_proxy_claude_model_shape(
    model_name: &str,
    model: &ModelConfig,
    providers: &crate::providers::ProvidersConfig,
) -> Result<(), ModelError> {
    for model_provider in &model.providers {
        if !is_claude_provider(model_provider, providers) {
            continue;
        }
        let Some(effective) = resolve_effective_provider_for_model(model_provider, providers)
        else {
            continue;
        };
        let argv = build_provider_argv_tokens(&effective);
        let shape =
            ClaudeToolFilterShape::detect_in_argv(&argv).unwrap_or(ClaudeToolFilterShape::NoFilter);
        validate_proxy_claude_filter_shape(effective.invocation_mode, shape).map_err(|err| {
            ModelError::Other(
                model_name.to_string(),
                format!("provider {}: {err}", model_provider.name),
            )
        })?;
    }
    Ok(())
}

fn is_claude_provider(
    model_provider: &ProviderConfig,
    providers: &crate::providers::ProvidersConfig,
) -> bool {
    let Some(root) = providers.get(&model_provider.name) else {
        return false;
    };
    let root_family = derive_provider_name(
        root.command.as_deref().unwrap_or(&model_provider.name),
        &root.args,
    );
    root_family
        .rsplit('/')
        .next()
        .unwrap_or(&root_family)
        .starts_with("claude")
        || model_provider.name.starts_with("claude")
}

fn resolve_effective_provider_for_model(
    model_provider: &ProviderConfig,
    providers: &crate::providers::ProvidersConfig,
) -> Option<ProviderConfig> {
    providers
        .effective_provider(model_provider)
        .ok()
        .map(|(effective, _)| effective)
}

fn build_provider_argv_tokens(provider: &ProviderConfig) -> Vec<String> {
    let mut argv = crate::providers::shell_split(&provider.command);
    argv.extend(provider.args.iter().cloned());
    if let Some(interactive_args) = provider.interactive_args.as_ref() {
        argv.extend(interactive_args.iter().cloned());
    }
    argv
}

fn parse_input_type(raw: &RawInput) -> Result<InputType, String> {
    match raw.type_name.as_str() {
        "string" => Ok(InputType::String),
        "integer" => Ok(InputType::Integer {
            min: raw.min.map(|v| v as i64),
            max: raw.max.map(|v| v as i64),
        }),
        "number" => Ok(InputType::Number {
            min: raw.min,
            max: raw.max,
        }),
        "boolean" => Ok(InputType::Boolean),
        "enum" => {
            let options = raw
                .options
                .clone()
                .ok_or_else(|| format!("Input '{}': enum type requires 'options'", raw.name))?;
            Ok(InputType::Enum { options })
        }
        "array" => {
            let item_type = raw
                .item_type
                .clone()
                .unwrap_or_else(|| "string".to_string());
            Ok(InputType::Array {
                item_type,
                min_items: raw.min_items,
                max_items: raw.max_items,
            })
        }
        other => Err(format!("Input '{}': unknown type '{}'", raw.name, other)),
    }
}

fn parse_inputs(raw_inputs: Vec<RawInput>) -> Result<Vec<InputDef>, String> {
    let mut inputs = Vec::new();
    let mut has_default_input = false;

    for raw in raw_inputs {
        if raw.default_input {
            if has_default_input {
                return Err(format!(
                    "Input '{}': only one input can have default_input = true",
                    raw.name
                ));
            }
            has_default_input = true;
        }

        let input_type = parse_input_type(&raw)?;
        inputs.push(InputDef {
            name: raw.name,
            input_type,
            required: raw.required,
            default_input: raw.default_input,
            default: raw.default,
            description: raw.description,
            flag: raw.flag,
        });
    }

    Ok(inputs)
}

impl ModelConfig {
    pub fn to_toml(&self) -> String {
        let mut out = String::new();

        let mode_str = match self.prompt_mode {
            PromptMode::Stdin => "stdin",
            PromptMode::Arg => "arg",
        };

        if let Some(provider) = self.provider.as_ref() {
            append_provider_implementation_ref(&mut out, provider);
        }

        if self.providers.len() == 1 {
            let p = &self.providers[0];
            out.push_str("[[providers]]\n");
            out.push_str(&format!("name = \"{}\"\n", p.name));
            out.push_str(&format!("args = [{}]\n", format_string_list(&p.args)));
            append_optional_string_list(
                &mut out,
                "interactive_args",
                p.interactive_args.as_deref(),
            );
        } else {
            let _ = mode_str;
            for p in &self.providers {
                out.push_str("\n[[providers]]\n");
                out.push_str(&format!("name = \"{}\"\n", p.name));
                out.push_str(&format!("args = [{}]\n", format_string_list(&p.args)));
                append_optional_string_list(
                    &mut out,
                    "interactive_args",
                    p.interactive_args.as_deref(),
                );
            }
        }

        // Inputs
        for input in &self.inputs {
            out.push('\n');
            out.push_str("[[inputs]]\n");
            out.push_str(&format!("name = \"{}\"\n", input.name));
            out.push_str(&format!(
                "type = \"{}\"\n",
                input_type_name(&input.input_type)
            ));
            if input.required {
                out.push_str("required = true\n");
            }
            if input.default_input {
                out.push_str("default_input = true\n");
            }
            if let Some(ref flag) = input.flag {
                out.push_str(&format!("flag = \"{}\"\n", flag));
            }
            if let Some(ref desc) = input.description {
                out.push_str(&format!("description = \"{}\"\n", desc));
            }
            if let Some(ref default) = input.default {
                out.push_str(&format!("default = {}\n", format_toml_value(default)));
            }

            // Type-specific fields
            match &input.input_type {
                InputType::Integer { min, max } => {
                    if let Some(v) = min {
                        out.push_str(&format!("min = {}.0\n", v));
                    }
                    if let Some(v) = max {
                        out.push_str(&format!("max = {}.0\n", v));
                    }
                }
                InputType::Number { min, max } => {
                    if let Some(v) = min {
                        out.push_str(&format!("min = {}\n", v));
                    }
                    if let Some(v) = max {
                        out.push_str(&format!("max = {}\n", v));
                    }
                }
                InputType::Enum { options } => {
                    let opts: Vec<String> = options.iter().map(|o| format!("\"{}\"", o)).collect();
                    out.push_str(&format!("options = [{}]\n", opts.join(", ")));
                }
                InputType::Array {
                    item_type,
                    min_items,
                    max_items,
                } => {
                    out.push_str(&format!("item_type = \"{}\"\n", item_type));
                    if let Some(v) = min_items {
                        out.push_str(&format!("min_items = {}\n", v));
                    }
                    if let Some(v) = max_items {
                        out.push_str(&format!("max_items = {}\n", v));
                    }
                }
                _ => {}
            }
        }

        out
    }

    pub fn from_toml(
        text: &str,
        providers: Option<&crate::providers::ProvidersConfig>,
    ) -> Result<Self, ModelError> {
        let raw = parse_model_toml(text)?;
        validate_model_toml_against_providers(&raw, providers)?;
        Ok(construct_model_config_from_raw(raw))
    }

    pub fn from_toml_with_name(
        name: &str,
        text: &str,
        providers: Option<&crate::providers::ProvidersConfig>,
    ) -> Result<Self, ModelError> {
        let model = Self::from_toml(text, providers).map_err(|err| err.with_model_name(name))?;
        Ok(apply_name_to_model(model, name))
    }
}

fn apply_name_to_model(mut model: ModelConfig, name: &str) -> ModelConfig {
    if model.name.is_empty() {
        model.name = name.to_string();
    }
    model
}

pub fn render_validated_model_toml(
    model: &ModelConfig,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<String, ModelError> {
    let rendered = emit_model_toml(model);
    let raw = parse_model_toml(&rendered)?;
    validate_model_toml_against_providers(&raw, providers)?;
    Ok(rendered)
}

/// Render a Rust string as a TOML basic-string literal, escaping
/// backslashes, quotes, and control characters per the TOML spec.
/// Defers to the `toml` crate's serializer to avoid hand-rolling
/// escape rules that drift from the spec.
fn toml_string_literal(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn append_optional_string_list(out: &mut String, key: &str, values: Option<&[String]>) {
    if let Some(values) = values {
        out.push_str(&format!("{key} = [{}]\n", format_string_list(values)));
    }
}

fn append_provider_implementation_ref(out: &mut String, provider: &ProviderImplementationRef) {
    let mut fields = Vec::new();
    if let Some(path) = provider.path.as_deref() {
        fields.push(format!("path = {}", toml_string_literal(path)));
    }
    if let Some(crate_name) = provider.crate_name.as_deref() {
        fields.push(format!("crate = {}", toml_string_literal(crate_name)));
    }
    if let Some(version) = provider.version.as_deref() {
        fields.push(format!("version = {}", toml_string_literal(version)));
    }
    if let Some(binary) = provider.binary.as_deref() {
        fields.push(format!("binary = {}", toml_string_literal(binary)));
    }
    if let Some(script) = provider.script.as_deref() {
        fields.push(format!("script = {}", toml_string_literal(script)));
    }
    out.push_str(&format!("provider = {{ {} }}\n\n", fields.join(", ")));
}

fn format_string_list(values: &[String]) -> String {
    values
        .iter()
        .map(|value| toml_string_literal(value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn input_type_name(t: &InputType) -> &'static str {
    match t {
        InputType::String => "string",
        InputType::Integer { .. } => "integer",
        InputType::Number { .. } => "number",
        InputType::Boolean => "boolean",
        InputType::Enum { .. } => "enum",
        InputType::Array { .. } => "array",
    }
}

fn format_toml_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("\"{}\"", s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => format!("{}", f),
        toml::Value::Boolean(b) => b.to_string(),
        other => format!("{}", other),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CodexArgPart {
    Standalone(String),
    Pair { flag: String, value: String },
}

impl CodexArgPart {
    fn display(&self) -> String {
        match self {
            Self::Standalone(token) => token.clone(),
            Self::Pair { flag, value } => format!("({flag}, {value})"),
        }
    }
}

fn split_codex_arg_parts(args: &[String]) -> Vec<CodexArgPart> {
    let mut parts = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let token = &args[i];
        if matches!(token.as_str(), "-c" | "--config") {
            if let Some(value) = args.get(i + 1) {
                parts.push(CodexArgPart::Pair {
                    flag: token.clone(),
                    value: value.clone(),
                });
                i += 2;
            } else {
                parts.push(CodexArgPart::Standalone(token.clone()));
                i += 1;
            }
        } else {
            parts.push(CodexArgPart::Standalone(token.clone()));
            i += 1;
        }
    }

    parts
}

fn codex_arg_overlap(root_args: &[String], model_args: &[String]) -> Option<String> {
    let root_parts = split_codex_arg_parts(root_args)
        .into_iter()
        .collect::<HashSet<_>>();

    split_codex_arg_parts(model_args)
        .into_iter()
        .find(|part| root_parts.contains(part))
        .map(|part| part.display())
}

fn codex_config_pair_key(value: &str) -> &str {
    value.split_once('=').map(|(key, _)| key).unwrap_or(value)
}

fn codex_model_config_pair_keys(args: &[String]) -> HashSet<String> {
    split_codex_arg_parts(args)
        .into_iter()
        .filter_map(|part| match part {
            CodexArgPart::Pair { value, .. } => Some(codex_config_pair_key(&value).to_string()),
            CodexArgPart::Standalone(_) => None,
        })
        .collect()
}

fn codex_typed_policy_overlap(
    root: &crate::providers::ProviderEntry,
    model_args: &[String],
) -> Option<String> {
    let policy_pairs = root
        .tool_restrictions
        .as_ref()
        .filter(|restrictions| restrictions.kind == ToolRestrictionKind::Codex)?
        .codex
        .config_pairs
        .iter()
        .collect::<Vec<_>>();
    if policy_pairs.is_empty() {
        return None;
    }

    let model_keys = codex_model_config_pair_keys(model_args);
    policy_pairs
        .into_iter()
        .find(|pair| model_keys.contains(codex_config_pair_key(pair)))
        .map(|pair| pair.to_string())
}

pub(crate) fn validate_codex_model_arg_overlap(
    model_name: &str,
    model: &ModelConfig,
    providers: &crate::providers::ProvidersConfig,
) -> Result<(), String> {
    for model_provider in model.providers.iter() {
        let Some(root_codex) = providers.get(&model_provider.name) else {
            continue;
        };
        let root_family = derive_provider_name(
            root_codex
                .command
                .as_deref()
                .unwrap_or(&model_provider.name),
            &root_codex.args,
        );
        if !root_family.starts_with("codex") && !model_provider.name.starts_with("codex") {
            continue;
        }

        if let Some(display) = codex_arg_overlap(&root_codex.args, &model_provider.args) {
            return Err(format!(
                "Model {model_name} provider {}: args token \"{display}\" duplicates root [{}].args; leave provider-level Codex flags in providers.toml and keep only model-specific flags in model TOML; run `agents migrate-config` to repair existing files.",
                model_provider.name, model_provider.name
            ));
        }

        if let Some(display) = codex_typed_policy_overlap(root_codex, &model_provider.args) {
            return Err(format!(
                "Model {model_name} provider {}: typed-policy config pair \"{display}\" duplicates model args; leave provider-level Codex policy in providers.toml and keep only model-specific flags in model TOML.",
                model_provider.name
            ));
        }

        if let (Some(root_interactive_args), Some(model_interactive_args)) = (
            root_codex.interactive_args.as_deref(),
            model_provider.interactive_args.as_deref(),
        ) && !root_interactive_args.is_empty()
            && !model_interactive_args.is_empty()
            && let Some(display) = codex_arg_overlap(root_interactive_args, model_interactive_args)
        {
            return Err(format!(
                "Model {model_name} provider {}: interactive_args token \"{display}\" duplicates root [{}].interactive_args; leave provider-level Codex flags in providers.toml and keep only model-specific flags in model TOML; run `agents migrate-config` to repair existing files.",
                model_provider.name, model_provider.name
            ));
        }
    }

    Ok(())
}

pub fn load_models(
    models_dir: &Path,
    providers: Option<&crate::providers::ProvidersConfig>,
) -> Result<HashMap<String, ModelConfig>, ModelError> {
    let files = read_model_files(models_dir)?;
    let raws = parse_model_files(files)?;
    validate_models_against_providers(&raws, providers)?;
    build_named_model_map(raws)
}

fn build_named_model_map(
    raws: Vec<(PathBuf, RawModelToml)>,
) -> Result<HashMap<String, ModelConfig>, ModelError> {
    raws.into_iter()
        .map(|(path, raw)| {
            let name = model_name_from_path(&path)?;
            let mut model = construct_model_config_from_raw(raw);
            model.name = name.clone();
            Ok((name, model))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::providers::{ProviderEntry, ProvidersConfig};

    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn function_body<'a>(source: &'a str, needle: &str) -> &'a str {
        let start = source.find(needle).expect("function signature exists");
        let brace_start = source[start..].find('{').expect("function body starts") + start;
        let mut depth = 0usize;
        for (offset, ch) in source[brace_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[brace_start + 1..brace_start + offset];
                    }
                }
                _ => {}
            }
        }
        panic!("function body ends");
    }

    fn assert_contains_in_order(body: &str, expected: &[&str]) {
        let mut cursor = 0usize;
        for token in expected {
            let found = body[cursor..]
                .find(token)
                .unwrap_or_else(|| panic!("missing orchestration call {token} in {body}"));
            cursor += found + token.len();
        }
    }

    fn assert_forbidden_absent(body: &str, forbidden: &[&str]) {
        for token in forbidden {
            assert!(
                !body.contains(token),
                "orchestration shell must not contain inline logic token {token}: {body}"
            );
        }
    }

    fn write_providers_toml(root: &Path, body: &str) {
        fs::write(root.join("providers.toml"), body).unwrap();
    }

    fn write_model_toml(models_dir: &Path, name: &str, body: &str) {
        fs::create_dir_all(models_dir).unwrap();
        fs::write(models_dir.join(format!("{name}.toml")), body).unwrap();
    }

    fn load_temp_models(
        providers_toml: &str,
        model_name: &str,
        model_toml: &str,
    ) -> Result<HashMap<String, ModelConfig>, String> {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let models_dir = root.join("models");
        write_providers_toml(root, providers_toml);
        write_model_toml(&models_dir, model_name, model_toml);

        let providers = ProvidersConfig::load(&root.join("providers.toml")).unwrap();
        // AGE-40 Step 6c adds the `load_models(&Path, Option<&ProvidersConfig>)` signature.
        Ok(load_models(&models_dir, Some(&providers))?)
    }

    fn test_model(provider_name: &str, args: &[&str]) -> ModelConfig {
        ModelConfig {
            name: "gpt-high".into(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider(
                provider_name,
                args.iter().map(|arg| (*arg).to_string()).collect(),
            )],
            inputs: vec![],
            provider: None,
        }
    }

    fn test_providers(provider_name: &str, args: &[&str]) -> ProvidersConfig {
        let mut entries = HashMap::new();
        entries.insert(
            provider_name.to_string(),
            ProviderEntry {
                args: args.iter().map(|arg| (*arg).to_string()).collect(),
                ..ProviderEntry::default()
            },
        );
        ProvidersConfig { entries }
    }

    fn codex_providers(args: &[&str], interactive_args: Option<&[&str]>) -> ProvidersConfig {
        let mut entries = HashMap::new();
        entries.insert(
            "codex".to_string(),
            ProviderEntry {
                args: args.iter().map(|arg| (*arg).to_string()).collect(),
                interactive_args: interactive_args
                    .map(|args| args.iter().map(|arg| (*arg).to_string()).collect()),
                ..ProviderEntry::default()
            },
        );
        ProvidersConfig { entries }
    }

    fn codex_providers_with_typed_policy(config_pairs: &[&str]) -> ProvidersConfig {
        let mut entries = HashMap::new();
        entries.insert(
            "codex".to_string(),
            ProviderEntry {
                command: Some("codex".to_string()),
                args: vec!["exec".to_string()],
                tool_restrictions: Some(ToolRestrictions {
                    kind: ToolRestrictionKind::Codex,
                    claude: ClaudeRestrictions::default(),
                    codex: CodexRestrictions {
                        config_pairs: config_pairs
                            .iter()
                            .map(|pair| (*pair).to_string())
                            .collect(),
                        disabled_features: Vec::new(),
                    },
                }),
                ..ProviderEntry::default()
            },
        );
        ProvidersConfig { entries }
    }

    #[test]
    fn model_config_from_toml_orchestrates_parse_validate_construct() {
        let body = function_body(
            include_str!("model.rs"),
            "pub fn from_toml(\n        text: &str,\n        providers: Option<&crate::providers::ProvidersConfig>,\n    ) -> Result<Self, ModelError>",
        );

        assert_contains_in_order(
            body,
            &[
                "parse_model_toml",
                "validate_model_toml_against_providers",
                "construct_model_config_from_raw",
            ],
        );
        assert_forbidden_absent(
            body,
            &[
                "toml::from_str",
                "parse_inputs",
                "ProviderConfig {",
                "validate_interactive_args",
                "validate_resume_interactive_args",
            ],
        );
    }

    #[test]
    fn render_validated_model_toml_orchestrates_emit_parse_validate() {
        let body = function_body(
            include_str!("model.rs"),
            "pub fn render_validated_model_toml(",
        );

        assert_contains_in_order(
            body,
            &[
                "emit_model_toml",
                "parse_model_toml",
                "validate_model_toml_against_providers",
            ],
        );
        assert_forbidden_absent(
            body,
            &[
                ".to_toml",
                "ModelConfig::from_toml",
                "validate_codex_model_arg_overlap",
                "toml::from_str",
            ],
        );
    }

    #[test]
    fn load_models_orchestrates_read_parse_validate_construct() {
        let body = function_body(include_str!("model.rs"), "pub fn load_models(");

        assert_contains_in_order(
            body,
            &[
                "read_model_files",
                "parse_model_files",
                "validate_models_against_providers",
                "build_named_model_map",
            ],
        );
        assert_forbidden_absent(
            body,
            &[
                "fs::read_dir",
                "fs::read_to_string",
                "ModelConfig::from_toml",
                "validate_codex_model_arg_overlap",
                "models.insert",
            ],
        );
    }

    #[test]
    fn derive_provider_name_simple() {
        assert_eq!(derive_provider_name("claude", &[]), "claude");
    }

    #[test]
    fn derive_provider_name_env_wrapper() {
        let args = vec![
            "-u".to_string(),
            "CLAUDECODE".to_string(),
            "claude2".to_string(),
            "-p".to_string(),
            "--model".to_string(),
            "opus".to_string(),
        ];
        assert_eq!(derive_provider_name("env", &args), "claude2");
    }

    #[test]
    fn derive_provider_name_prefixed_command_string() {
        assert_eq!(
            derive_provider_name("env -u CODEX_ENV codex", &["exec".to_string()]),
            "codex"
        );
    }

    #[test]
    fn derive_provider_name_env_assignment() {
        let args = vec!["FOO=bar".to_string(), "claude3".to_string()];
        assert_eq!(derive_provider_name("env", &args), "claude3");
    }

    #[test]
    fn provider_config_auto_derives_name() {
        let p = ProviderConfig::new(
            "env",
            vec![
                "-u".to_string(),
                "CLAUDECODE".to_string(),
                "claude2".to_string(),
                "-p".to_string(),
            ],
        );
        assert_eq!(p.name, "claude2");
    }

    #[test]
    fn parse_model_provider_flags_only() {
        let toml = r#"
[[providers]]
name = "claude2"
args = ["-p", "--model", "opus", "--output-format", "json"]
interactive_args = ["--model", "opus"]
"#;
        let config = ModelConfig::from_toml_with_name("claude-opus", toml, None).unwrap();
        let provider = &config.providers[0];

        assert_eq!(provider.name, "claude2");
        assert_eq!(provider.command, "");
        assert_eq!(
            provider.args,
            ["-p", "--model", "opus", "--output-format", "json"]
        );
        assert_eq!(
            provider.interactive_args.as_deref(),
            Some(&["--model".to_string(), "opus".to_string()][..])
        );
        assert!(provider.resume.is_none());
        assert!(provider.session_capture.is_none());
        assert!(provider.session_storage.is_none());
    }

    #[test]
    fn model_roundtrip_keeps_model_provider_fields_only() {
        let original = r#"
[[providers]]
name = "claude"
args = ["-p", "--model", "sonnet"]

[[providers]]
name = "claude2"
args = ["-p", "--model", "opus"]
interactive_args = ["--model", "opus"]

[[inputs]]
name = "prompt"
type = "string"
default_input = true
"#;
        let c1 = ModelConfig::from_toml_with_name("claude-opus", original, None).unwrap();
        let rendered = c1.to_toml();
        assert!(!rendered.contains("command ="));
        assert!(!rendered.contains("prompt_mode"));
        assert!(!rendered.contains("session_storage"));

        let c2 = ModelConfig::from_toml_with_name("claude-opus", &rendered, None).unwrap();
        assert_eq!(c1.providers.len(), c2.providers.len());
        assert_eq!(
            c1.providers[1].interactive_args,
            c2.providers[1].interactive_args
        );
        assert_eq!(c1.inputs.len(), c2.inputs.len());
    }

    #[test]
    fn rejects_no_providers() {
        let toml = r#"
[[inputs]]
name = "prompt"
type = "string"
"#;
        assert!(ModelConfig::from_toml_with_name("test", toml, None).is_err());
    }

    #[test]
    fn rejects_duplicate_default_input() {
        let toml = r#"
[[providers]]
name = "test"

[[inputs]]
name = "a"
type = "string"
default_input = true

[[inputs]]
name = "b"
type = "string"
default_input = true
"#;
        let result = ModelConfig::from_toml_with_name("test", toml, None);
        assert!(result.unwrap_err().contains("only one input"));
    }

    #[test]
    fn rejects_enum_without_options() {
        let toml = r#"
[[providers]]
name = "test"

[[inputs]]
name = "format"
type = "enum"
"#;
        let result = ModelConfig::from_toml_with_name("test", toml, None);
        assert!(result.unwrap_err().contains("requires 'options'"));
    }

    #[test]
    fn config_load_rejects_old_per_provider_blocks_in_model_toml() {
        let toml = r#"
[[providers]]
name = "claude2"
command = "env"
args = ["-u", "CLAUDECODE", "claude2", "-p", "--model", "opus"]
prompt_mode = "stdin"

[providers.resume]
kind = "flag"
flag = "--resume"
"#;
        let err = ModelConfig::from_toml_with_name("claude-opus", toml, None).unwrap_err();
        assert!(err.contains("Old per-provider config detected in claude-opus.toml"));
        assert!(err.contains("agents migrate-config"));
    }

    #[test]
    fn model_toml_rejects_age28_provider_fields() {
        let toml = r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
system_prompt_override = "root-only policy must not live in model TOML"

[providers.tool_restrictions]
kind = "claude"

[providers.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#;

        let err = ModelConfig::from_toml_with_name("claude-opus", toml, None).unwrap_err();

        assert!(err.contains("model claude-opus provider claude"), "{err}");
        assert!(err.contains("system_prompt_override"), "{err}");
        assert!(err.contains("root-only"), "{err}");
        assert!(err.contains("providers.toml"), "{err}");
    }

    #[test]
    fn old_top_level_model_config_is_rejected() {
        let toml = r#"
command = "claude"
args = ["-p", "--model", "opus"]
prompt_mode = "stdin"
"#;
        let err = ModelConfig::from_toml_with_name("claude-opus", toml, None).unwrap_err();
        assert!(err.contains("agents migrate-config"));
    }

    #[test]
    fn load_models_rejects_codex_dangerously_flag_overlap() {
        let err = load_temp_models(
            r#"
[codex]
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
            "gpt-high",
            r#"
[[providers]]
name = "codex"
args = ["--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.5"]
"#,
        )
        .unwrap_err();

        assert!(err.contains("--dangerously-bypass-approvals-and-sandbox"));
    }

    #[test]
    fn load_models_accepts_canonical_codex_split() {
        let models = load_temp_models(
            r#"
[codex]
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
            "gpt-high",
            r#"
[[providers]]
name = "codex"
args = ["-m", "gpt-5.5"]
"#,
        )
        .unwrap();

        assert_eq!(
            models["gpt-high"].providers[0].args,
            vec!["-m".to_string(), "gpt-5.5".to_string()]
        );
    }

    #[test]
    fn render_validated_model_toml_accepts_clean_codex_model_with_providers() {
        let providers = codex_providers(&["exec", "-c", "sandbox=workspace-write"], None);
        let model = test_model("codex", &["-m", "gpt-5.5"]);

        let rendered = super::render_validated_model_toml(&model, Some(&providers)).unwrap();
        let reparsed = ModelConfig::from_toml_with_name("gpt-high", &rendered, None).unwrap();

        assert_eq!(
            reparsed.providers[0].args,
            vec!["-m".to_string(), "gpt-5.5".to_string()]
        );
    }

    #[test]
    fn render_validated_model_toml_rejects_duplicate_codex_args() {
        let providers = codex_providers(&["exec", "-c", "sandbox=workspace-write"], None);
        let model = test_model("codex", &["exec", "-m", "gpt-5.5"]);

        let err = super::render_validated_model_toml(&model, Some(&providers)).unwrap_err();

        assert!(err.contains("duplicates root [codex].args"), "{err}");
    }

    #[test]
    fn render_validated_model_toml_rejects_duplicate_codex_interactive_args() {
        let providers = codex_providers(
            &["exec"],
            Some(&["exec", "--dangerously-bypass-approvals-and-sandbox"]),
        );
        let mut model = test_model("codex", &["-m", "gpt-5.5"]);
        model.providers[0].interactive_args = Some(vec![
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
        ]);

        let err = super::render_validated_model_toml(&model, Some(&providers)).unwrap_err();

        assert!(
            err.contains("duplicates root [codex].interactive_args"),
            "{err}"
        );
    }

    #[test]
    fn render_validated_model_toml_without_providers_bypasses_overlap_check() {
        let model = test_model("codex", &["exec", "-m", "gpt-5.5"]);

        let rendered = super::render_validated_model_toml(&model, None).unwrap();
        let reparsed = ModelConfig::from_toml_with_name("gpt-high", &rendered, None).unwrap();

        assert_eq!(reparsed.providers[0].args[0], "exec");
    }

    #[test]
    fn load_models_accepts_age29_migrated_c_split() {
        load_temp_models(
            r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
"#,
            "gpt-high",
            r#"
[[providers]]
name = "codex"
args = ["-c", "model_reasoning_effort=high"]
"#,
        )
        .unwrap();
    }

    #[test]
    fn load_models_rejects_duplicate_c_pair() {
        let err = load_temp_models(
            r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
"#,
            "gpt-high",
            r#"
[[providers]]
name = "codex"
args = ["-c", "sandbox=workspace-write"]
"#,
        )
        .unwrap_err();

        assert!(err.contains("(-c, sandbox=workspace-write)"));
        assert!(err.contains("[codex].args"));
    }

    #[test]
    fn codex_overlap_guard_blocks_typed_policy_vs_model_arg_collision() {
        let providers = codex_providers_with_typed_policy(&["model_reasoning_effort=high"]);
        let model = test_model(
            "codex",
            &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
        );

        let err = validate_codex_model_arg_overlap("gpt-high", &model, &providers).unwrap_err();

        assert!(err.contains("gpt-high"), "{err}");
        assert!(err.contains("codex"), "{err}");
        assert!(err.contains("typed-policy"), "{err}");
        assert!(err.contains("model_reasoning_effort=high"), "{err}");
    }

    #[test]
    fn codex_overlap_guard_blocks_typed_policy_key_override() {
        let providers = codex_providers_with_typed_policy(&["sandbox=read-only"]);
        let model = test_model("codex", &["-m", "gpt-5.5", "-c", "sandbox=workspace-write"]);

        let err = validate_codex_model_arg_overlap("gpt-high", &model, &providers).unwrap_err();

        assert!(err.contains("typed-policy"), "{err}");
        assert!(err.contains("sandbox=read-only"), "{err}");
    }

    #[test]
    fn codex_overlap_guard_does_not_regress_model_reasoning_effort() {
        let empty_policy = codex_providers_with_typed_policy(&[]);
        let model = test_model(
            "codex",
            &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
        );
        validate_codex_model_arg_overlap("gpt-high", &model, &empty_policy).unwrap();

        let unrelated_policy = codex_providers_with_typed_policy(&["other_key=val"]);
        validate_codex_model_arg_overlap("gpt-high", &model, &unrelated_policy).unwrap();

        load_temp_models(
            r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
"#,
            "gpt-high",
            r#"
[[providers]]
name = "codex"
args = ["-c", "model_reasoning_effort=high"]
"#,
        )
        .unwrap();
    }

    #[test]
    fn codex_overlap_guard_characterizes_sibling_alias_policy() {
        let mut entries = HashMap::new();
        entries.insert(
            "codex2".to_string(),
            ProviderEntry {
                command: Some("codex2".to_string()),
                args: vec!["exec".to_string()],
                tool_restrictions: Some(ToolRestrictions {
                    kind: ToolRestrictionKind::Codex,
                    claude: ClaudeRestrictions::default(),
                    codex: CodexRestrictions {
                        config_pairs: vec!["model_reasoning_effort=high".to_string()],
                        disabled_features: Vec::new(),
                    },
                }),
                ..ProviderEntry::default()
            },
        );
        let providers = ProvidersConfig { entries };
        let model = test_model(
            "codex2",
            &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
        );

        let err = validate_codex_model_arg_overlap("gpt-high", &model, &providers).unwrap_err();

        assert!(err.contains("codex2"), "{err}");
        assert!(err.contains("typed-policy"), "{err}");
        assert!(err.contains("model_reasoning_effort=high"), "{err}");
    }

    #[test]
    fn codex_overlap_guard_uses_prefixed_root_command_string() {
        let mut entries = HashMap::new();
        entries.insert(
            "primary".to_string(),
            ProviderEntry {
                command: Some("env -u CODEX_ENV codex".to_string()),
                args: vec!["exec".to_string()],
                tool_restrictions: Some(ToolRestrictions {
                    kind: ToolRestrictionKind::Codex,
                    claude: ClaudeRestrictions::default(),
                    codex: CodexRestrictions {
                        config_pairs: vec!["model_reasoning_effort=high".to_string()],
                        disabled_features: Vec::new(),
                    },
                }),
                ..ProviderEntry::default()
            },
        );
        let providers = ProvidersConfig { entries };
        let model = test_model(
            "primary",
            &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
        );

        let err = validate_codex_model_arg_overlap("gpt-high", &model, &providers).unwrap_err();

        assert!(err.contains("primary"), "{err}");
        assert!(err.contains("typed-policy"), "{err}");
        assert!(err.contains("model_reasoning_effort=high"), "{err}");
    }

    #[test]
    fn split_codex_arg_parts_groups_c_and_config_pairs() {
        assert_eq!(
            split_codex_arg_parts(&[
                "exec".to_string(),
                "-c".to_string(),
                "sandbox=workspace-write".to_string(),
                "--config".to_string(),
                "model_reasoning_effort=high".to_string(),
            ]),
            vec![
                CodexArgPart::Standalone("exec".to_string()),
                CodexArgPart::Pair {
                    flag: "-c".to_string(),
                    value: "sandbox=workspace-write".to_string(),
                },
                CodexArgPart::Pair {
                    flag: "--config".to_string(),
                    value: "model_reasoning_effort=high".to_string(),
                },
            ]
        );

        assert_eq!(
            codex_arg_overlap(
                &[
                    "exec".to_string(),
                    "--config".to_string(),
                    "a=1".to_string()
                ],
                &["--config".to_string(), "a=1".to_string()]
            ),
            Some("(--config, a=1)".to_string())
        );
        assert_eq!(
            codex_arg_overlap(
                &[
                    "exec".to_string(),
                    "--config".to_string(),
                    "a=1".to_string()
                ],
                &["--config".to_string(), "b=2".to_string()]
            ),
            None
        );
    }

    #[test]
    fn load_models_accepts_age29_migrated_config_split() {
        load_temp_models(
            r#"
[codex]
command = "codex"
args = ["exec", "--config", "sandbox=workspace-write"]
"#,
            "gpt-high",
            r#"
[[providers]]
name = "codex"
args = ["--config", "model_reasoning_effort=high"]
"#,
        )
        .unwrap();
    }

    #[test]
    fn load_models_rejects_duplicate_config_pair() {
        let err = load_temp_models(
            r#"
[codex]
command = "codex"
args = ["exec", "--config", "sandbox=workspace-write"]
"#,
            "gpt-high",
            r#"
[[providers]]
name = "codex"
args = ["--config", "sandbox=workspace-write"]
"#,
        )
        .unwrap_err();

        assert!(err.contains("(--config, sandbox=workspace-write)"));
        assert!(err.contains("[codex].args"));
    }

    #[test]
    fn load_models_ignores_codex_root_overlap_for_non_codex_model_provider() {
        load_temp_models(
            r#"
[codex]
command = "codex"
args = ["exec", "--config", "sandbox=workspace-write"]
"#,
            "claude-sonnet",
            r#"
[[providers]]
name = "claude"
args = ["exec", "--config", "sandbox=workspace-write"]
"#,
        )
        .unwrap();
    }

    #[test]
    fn validator_error_message_names_token_and_repair_path() {
        let err = load_temp_models(
            r#"
[codex]
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
            "gpt-high",
            r#"
[[providers]]
name = "codex"
args = ["--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.5"]
"#,
        )
        .unwrap_err();

        assert!(err.contains("gpt-high"));
        assert!(err.contains("codex"));
        assert!(err.contains("args"));
        assert!(err.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(err.contains("[codex].args"));
        assert!(err.contains("agents migrate-config"));
    }

    #[test]
    fn load_models_rejects_codex_interactive_args_overlap() {
        let err = load_temp_models(
            r#"
[codex]
command = "codex"
interactive_args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
            "gpt-high",
            r#"
[[providers]]
name = "codex"
args = ["-m", "gpt-5.5"]
interactive_args = ["--dangerously-bypass-approvals-and-sandbox"]
"#,
        )
        .unwrap_err();

        assert!(err.contains("interactive_args"));
        assert!(err.contains("[codex].interactive_args"));
    }

    #[test]
    fn overlap_predicate_boundary_table() {
        let cases = [
            (
                "exact standalone duplicate",
                "codex",
                vec!["foo", "bar"],
                vec!["bar"],
                false,
            ),
            (
                "non-equal substring",
                "codex",
                vec!["foobar"],
                vec!["foo"],
                true,
            ),
            (
                "different -c key=value",
                "codex",
                vec!["-c", "a=1"],
                vec!["-c", "b=2"],
                true,
            ),
            (
                "identical -c key=value",
                "codex",
                vec!["-c", "a=1"],
                vec!["-c", "a=1"],
                false,
            ),
            (
                "different provider name",
                "claude",
                vec!["foo"],
                vec!["foo"],
                true,
            ),
            (
                "non-codex provider",
                "gemini",
                vec!["foo"],
                vec!["foo"],
                true,
            ),
        ];

        for (name, provider_name, root_args, model_args, expect_ok) in cases {
            let providers = test_providers(provider_name, &root_args);
            let model = test_model(provider_name, &model_args);

            // AGE-40 Step 6c adds `validate_codex_model_arg_overlap`.
            let result = validate_codex_model_arg_overlap("gpt-high", &model, &providers);

            assert_eq!(
                result.is_ok(),
                expect_ok,
                "{name}: expected ok={expect_ok}, got {result:?}"
            );
        }
    }

    #[test]
    fn provider_config_constructors_default_invocation_mode_to_headless() {
        let direct = ProviderConfig::new("claude", vec!["-p".to_string()]);
        let model = ProviderConfig::model_provider("claude", vec!["--model".to_string()]);

        assert_eq!(direct.invocation_mode, InvocationMode::Headless);
        assert_eq!(model.invocation_mode, InvocationMode::Headless);
    }

    #[test]
    fn model_provider_rejects_invocation_mode_as_root_only() {
        let err = ModelConfig::from_toml_with_name(
            "claude-opus",
            r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
invocation_mode = "proxy"
"#,
            None,
        )
        .unwrap_err();

        match err {
            ModelError::InvocationModeIsRootOnly { model, provider } => {
                assert_eq!(model, "claude-opus");
                assert_eq!(provider, "claude");
            }
            other => panic!("expected ModelError::InvocationModeIsRootOnly, got {other:?}"),
        }
    }

    #[test]
    fn load_models_rejects_proxy_claude_tools_mcp_filter_from_model_args_when_effective() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let models_dir = root.join("models");
        write_providers_toml(
            root,
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
        );
        write_model_toml(
            &models_dir,
            "claude-opus",
            r#"
[[providers]]
name = "claude"
args = ["--tools", "mcp__age104p2__Task"]
"#,
        );
        let providers = ProvidersConfig::load(&root.join("providers.toml")).unwrap();

        let err = load_models(&models_dir, Some(&providers)).unwrap_err();

        assert!(err.contains("claude-opus"), "{err}");
        assert!(err.contains("proxy-mode Claude"), "{err}");
        assert!(err.contains("--tools mcp__"), "{err}");
    }

    #[test]
    fn render_validated_model_toml_omits_root_only_invocation_mode() {
        let mut model = test_model("claude", &["--model", "opus"]);
        model.providers[0].invocation_mode = InvocationMode::Proxy;

        let rendered = render_validated_model_toml(&model, None).unwrap();

        assert!(!rendered.contains("invocation_mode"), "{rendered}");
        let reparsed = ModelConfig::from_toml_with_name("claude-opus", &rendered, None).unwrap();
        assert_eq!(
            reparsed.providers[0].invocation_mode,
            InvocationMode::Headless
        );
    }

    #[test]
    fn model_roundtrip_rejects_provider_invocation_mode_as_root_only() {
        let rendered = r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
invocation_mode = "proxy"
"#;

        let err = ModelConfig::from_toml_with_name("claude-opus", rendered, None).unwrap_err();

        match err {
            ModelError::InvocationModeIsRootOnly { model, provider } => {
                assert_eq!(model, "claude-opus");
                assert_eq!(provider, "claude");
            }
            other => panic!("expected ModelError::InvocationModeIsRootOnly, got {other:?}"),
        }
    }

    #[test]
    fn parse_model_toml_returns_raw_struct_for_valid_text() {
        let raw = parse_model_toml(
            r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
        )
        .unwrap();

        assert_eq!(raw.providers.unwrap().len(), 1);
    }

    #[test]
    fn parse_model_toml_returns_err_for_malformed_text() {
        let err = parse_model_toml("not = [").unwrap_err();

        match err {
            ModelError::Toml(model, message) => {
                assert_eq!(model, "<unknown>");
                assert!(!message.is_empty());
            }
            other => panic!("expected ModelError::Toml, got {other:?}"),
        }
    }

    #[test]
    fn validate_model_toml_against_providers_rejects_root_only_mode() {
        let raw = parse_model_toml(
            r#"
[[providers]]
name = "claude"
invocation_mode = "proxy"
"#,
        )
        .unwrap();

        let err = validate_model_toml_against_providers(&raw, None).unwrap_err();

        match err {
            ModelError::InvocationModeIsRootOnly { model, provider } => {
                assert_eq!(model, "<unknown>");
                assert_eq!(provider, "claude");
            }
            other => panic!("expected ModelError::InvocationModeIsRootOnly, got {other:?}"),
        }
    }

    #[test]
    fn validate_model_toml_against_providers_passes_for_legal_model() {
        let raw = parse_model_toml(
            r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
        )
        .unwrap();

        assert_eq!(validate_model_toml_against_providers(&raw, None), Ok(()));
    }

    #[test]
    fn construct_model_config_from_raw_preserves_legal_fields() {
        let raw = parse_model_toml(
            r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
        )
        .unwrap();

        let model = construct_model_config_from_raw(raw);

        assert_eq!(model.providers[0].name, "claude");
        assert_eq!(model.providers[0].args, ["--model", "opus"]);
        assert_eq!(model.providers[0].invocation_mode, InvocationMode::Headless);
    }

    #[test]
    fn emit_model_toml_omits_invocation_mode_per_providers_block() {
        let mut model = test_model("claude", &["--model", "opus"]);
        model.providers[0].invocation_mode = InvocationMode::Proxy;

        let rendered = emit_model_toml(&model);

        assert!(rendered.contains("[[providers]]"), "{rendered}");
        assert!(!rendered.contains("invocation_mode"), "{rendered}");
    }

    #[test]
    fn read_model_files_returns_paths_and_text_pairs() {
        let temp = TempDir::new().unwrap();
        let models_dir = temp.path().join("models");
        write_model_toml(
            &models_dir,
            "claude-opus",
            r#"
[[providers]]
name = "claude"
"#,
        );

        let files = read_model_files(&models_dir).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].0.ends_with("claude-opus.toml"));
        assert!(files[0].1.contains("[[providers]]"));
    }

    #[test]
    fn parse_model_files_returns_paths_and_raw_pairs() {
        let path = PathBuf::from("claude-opus.toml");
        let parsed = parse_model_files(vec![(
            path.clone(),
            r#"
[[providers]]
name = "claude"
"#
            .to_string(),
        )])
        .unwrap();

        assert_eq!(parsed[0].0, path);
        assert_eq!(parsed[0].1.providers.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn validate_models_against_providers_rejects_unsafe_proxy_claude_shape_via_effective_merge() {
        let temp = TempDir::new().unwrap();
        write_providers_toml(
            temp.path(),
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
        );
        let providers = ProvidersConfig::load(&temp.path().join("providers.toml")).unwrap();
        let raw = parse_model_toml(
            r#"
[[providers]]
name = "claude"
args = ["--tools", "mcp__age104p2__Task"]
"#,
        )
        .unwrap();
        let raws = vec![(PathBuf::from("claude-opus.toml"), raw)];

        let err = validate_models_against_providers(&raws, Some(&providers)).unwrap_err();

        assert!(err.contains("proxy-mode Claude"), "{err}");
        assert!(err.contains("--tools mcp__"), "{err}");
    }
}
