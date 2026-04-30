use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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
        }
    }

    fn validate_interactive_args(&self) -> Result<(), String> {
        if matches!(self.interactive_args.as_ref(), Some(args) if args.is_empty()) {
            return Err("interactive_args must not be empty when provided".into());
        }
        Ok(())
    }

    fn validate_resume_interactive_args(&self) -> Result<(), String> {
        if matches!(
            self.resume.as_ref().map(|resume| resume.kind),
            Some(ResumeKind::Flag | ResumeKind::Subcommand)
        ) && self.interactive_args.is_none()
        {
            return Err(
                "[providers.resume] requires interactive_args for resumable providers".into(),
            );
        }
        Ok(())
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
        }
    }
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
    ClaudeCode { projects_dir: PathBuf },
    Codex { sessions_dir: PathBuf },
}

impl SessionStorage {
    pub fn expand_tilde(self) -> Self {
        match self {
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
    let mut all: Vec<&str> = Vec::with_capacity(1 + args.len());
    all.push(command);
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

#[derive(Deserialize)]
struct RawModelToml {
    command: Option<String>,
    args: Option<Vec<String>>,
    interactive_args: Option<Vec<String>>,
    resume: Option<ResumeStrategy>,
    prompt_mode: Option<String>,
    session_capture: Option<SessionCapture>,
    resume_acceptance: Option<ResumeAcceptanceRules>,
    session_storage: Option<SessionStorage>,
    providers: Option<Vec<RawProvider>>,
    inputs: Option<Vec<RawInput>>,
}

#[derive(Deserialize)]
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
}

#[derive(Deserialize)]
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

    pub fn from_toml(name: &str, content: &str) -> Result<Self, String> {
        let raw: RawModelToml =
            toml::from_str(content).map_err(|e| format!("TOML parse error for {name}: {e}"))?;

        if raw.command.is_some()
            || raw.args.is_some()
            || raw.interactive_args.is_some()
            || raw.resume.is_some()
            || raw.session_capture.is_some()
            || raw.session_storage.is_some()
            || raw.resume_acceptance.is_some()
            || raw.prompt_mode.is_some()
        {
            return Err(format!(
                "Old per-provider config detected in {name}.toml; run `agents migrate-config` to migrate."
            ));
        }

        let prompt_mode = PromptMode::Stdin;

        let inputs = if let Some(raw_inputs) = raw.inputs {
            parse_inputs(raw_inputs)?
        } else {
            vec![]
        };

        let providers = if let Some(providers) = raw.providers {
            providers
                .into_iter()
                .map(|p| {
                    if p.command.is_some()
                        || p.resume.is_some()
                        || p.session_capture.is_some()
                        || p.session_storage.is_some()
                        || p.resume_acceptance.is_some()
                        || p.prompt_mode.is_some()
                    {
                        return Err(format!(
                            "Old per-provider config detected in {name}.toml; run `agents migrate-config` to migrate."
                        ));
                    }
                    let args = p.args.unwrap_or_default();
                    let name = p
                        .name
                        .ok_or_else(|| format!("Model {name}: provider entry missing `name`"))?;
                    Ok(ProviderConfig {
                        name,
                        command: String::new(),
                        args,
                        interactive_args: p.interactive_args,
                        resume: None,
                        session_capture: None,
                        resume_acceptance: None,
                        session_storage: None,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?
        } else {
            return Err(format!(
                "Model {name}: must define at least one [[providers]] entry"
            ));
        };

        // Validate per-kind required fields on session_capture before
        // returning. Catching malformed configs at load time means the
        // executor never sees a kind=ForcedFlagVerified missing `flag`
        // (silent capture failure → silent transcript_state=missing).
        for p in &providers {
            p.validate_interactive_args()
                .map_err(|e| format!("Model {name} provider {}: {e}", p.name))?;
            if let Some(resume) = &p.resume {
                resume
                    .validate()
                    .map_err(|e| format!("Model {name} provider {}: {e}", p.name))?;
                p.validate_resume_interactive_args()
                    .map_err(|e| format!("Model {name} provider {}: {e}", p.name))?;
            }
            if let Some(capture) = &p.session_capture {
                capture
                    .validate()
                    .map_err(|e| format!("Model {name} provider {}: {e}", p.name))?;
            }
            if let Some(storage) = &p.session_storage {
                storage
                    .validate()
                    .map_err(|e| format!("Model {name} provider {}: {e}", p.name))?;
            }
        }

        if providers.is_empty() {
            return Err(format!("Model {name}: no providers defined"));
        }

        Ok(ModelConfig {
            name: name.to_string(),
            prompt_mode,
            providers,
            inputs,
        })
    }
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

pub fn load_models(models_dir: &Path) -> Result<HashMap<String, ModelConfig>, String> {
    let mut models = HashMap::new();

    if !models_dir.is_dir() {
        return Ok(models);
    }

    let entries =
        fs::read_dir(models_dir).map_err(|e| format!("Failed to read models directory: {e}"))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("Invalid filename: {}", path.display()))?
            .to_string();

        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

        let config = ModelConfig::from_toml(&name, &content)?;
        models.insert(name, config);
    }

    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let config = ModelConfig::from_toml("claude-opus", toml).unwrap();
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
        let c1 = ModelConfig::from_toml("claude-opus", original).unwrap();
        let rendered = c1.to_toml();
        assert!(!rendered.contains("command ="));
        assert!(!rendered.contains("prompt_mode"));
        assert!(!rendered.contains("session_storage"));

        let c2 = ModelConfig::from_toml("claude-opus", &rendered).unwrap();
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
        assert!(ModelConfig::from_toml("test", toml).is_err());
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
        let result = ModelConfig::from_toml("test", toml);
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
        let result = ModelConfig::from_toml("test", toml);
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
        let err = ModelConfig::from_toml("claude-opus", toml).unwrap_err();
        assert!(err.contains("Old per-provider config detected in claude-opus.toml"));
        assert!(err.contains("agents migrate-config"));
    }

    #[test]
    fn old_top_level_model_config_is_rejected() {
        let toml = r#"
command = "claude"
args = ["-p", "--model", "opus"]
prompt_mode = "stdin"
"#;
        let err = ModelConfig::from_toml("claude-opus", toml).unwrap_err();
        assert!(err.contains("agents migrate-config"));
    }
}
