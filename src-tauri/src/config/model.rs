use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable identifier for this provider (the CLI account / binary).
    /// Used as the key for per-account state like quota tracking.
    /// If omitted in TOML, derived from command+args via `derive_provider_name`.
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<ResumeStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_capture: Option<SessionCapture>,
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
        }
    }

    fn validate_interactive_args(&self) -> Result<(), String> {
        if matches!(self.interactive_args.as_ref(), Some(args) if args.is_empty()) {
            return Err("interactive_args must not be empty when provided".into());
        }
        Ok(())
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
    providers: Option<Vec<RawProvider>>,
    inputs: Option<Vec<RawInput>>,
}

#[derive(Deserialize)]
struct RawProvider {
    #[serde(default)]
    name: Option<String>,
    command: String,
    args: Option<Vec<String>>,
    interactive_args: Option<Vec<String>>,
    resume: Option<ResumeStrategy>,
    session_capture: Option<SessionCapture>,
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

fn parse_prompt_mode(s: &str) -> PromptMode {
    match s {
        "arg" => PromptMode::Arg,
        _ => PromptMode::Stdin,
    }
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
            out.push_str(&format!(
                "command = \"{}\"\nargs = [{}]\nprompt_mode = \"{}\"\n",
                p.command,
                format_string_list(&p.args),
                mode_str
            ));
            append_optional_string_list(
                &mut out,
                "interactive_args",
                p.interactive_args.as_deref(),
            );
            append_resume_toml(&mut out, "resume", p.resume.as_ref());
            append_session_capture_toml(&mut out, "session_capture", p.session_capture.as_ref());
        } else {
            out.push_str(&format!("prompt_mode = \"{}\"\n", mode_str));
            for p in &self.providers {
                out.push_str("\n[[providers]]\n");
                // Only emit an explicit name line when the stored name differs
                // from what would be auto-derived — keeps round-trips stable
                // while preserving any user-set disambiguation.
                if p.name != derive_provider_name(&p.command, &p.args) {
                    out.push_str(&format!("name = \"{}\"\n", p.name));
                }
                out.push_str(&format!(
                    "command = \"{}\"\nargs = [{}]\n",
                    p.command,
                    format_string_list(&p.args)
                ));
                append_optional_string_list(
                    &mut out,
                    "interactive_args",
                    p.interactive_args.as_deref(),
                );
                append_resume_toml(&mut out, "providers.resume", p.resume.as_ref());
                append_session_capture_toml(
                    &mut out,
                    "providers.session_capture",
                    p.session_capture.as_ref(),
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

        let prompt_mode = parse_prompt_mode(raw.prompt_mode.as_deref().unwrap_or("stdin"));

        let inputs = if let Some(raw_inputs) = raw.inputs {
            parse_inputs(raw_inputs)?
        } else {
            vec![]
        };

        let providers = if let Some(providers) = raw.providers {
            providers
                .into_iter()
                .map(|p| {
                    let args = p.args.unwrap_or_default();
                    let name = p
                        .name
                        .unwrap_or_else(|| derive_provider_name(&p.command, &args));
                    ProviderConfig {
                        name,
                        command: p.command,
                        args,
                        interactive_args: p.interactive_args,
                        resume: p.resume,
                        session_capture: p.session_capture,
                    }
                })
                .collect()
        } else if let Some(command) = raw.command {
            let args = raw.args.unwrap_or_default();
            let name = derive_provider_name(&command, &args);
            vec![ProviderConfig {
                name,
                command,
                args,
                interactive_args: raw.interactive_args,
                resume: raw.resume,
                session_capture: raw.session_capture,
            }]
        } else {
            return Err(format!(
                "Model {name}: must have either 'command' or '[[providers]]'"
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
                if p.interactive_args.is_none() {
                    return Err(format!(
                        "Model {name} provider {}: [providers.resume] requires interactive_args",
                        p.name
                    ));
                }
            }
            if let Some(capture) = &p.session_capture {
                capture
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

fn append_resume_toml(out: &mut String, table_name: &str, resume: Option<&ResumeStrategy>) {
    let Some(resume) = resume else {
        return;
    };

    out.push('\n');
    out.push_str(&format!("[{table_name}]\n"));
    out.push_str(&format!(
        "kind = \"{}\"\n",
        match resume.kind {
            ResumeKind::Flag => "flag",
            ResumeKind::Subcommand => "subcommand",
        }
    ));
    append_optional_string(out, "flag", resume.flag.as_deref());
    append_optional_string_list(out, "subcommand", resume.subcommand.as_deref());
}

fn append_session_capture_toml(
    out: &mut String,
    table_name: &str,
    capture: Option<&SessionCapture>,
) {
    let Some(capture) = capture else {
        return;
    };

    out.push('\n');
    out.push_str(&format!("[{table_name}]\n"));
    out.push_str(&format!(
        "kind = \"{}\"\n",
        match capture.kind {
            SessionCaptureKind::None => "none",
            SessionCaptureKind::ForcedFlagVerified => "forced_flag_verified",
            SessionCaptureKind::StdoutJsonEvent => "stdout_json_event",
        }
    ));
    append_optional_string(out, "flag", capture.flag.as_deref());
    append_optional_string_list(out, "readback_args", capture.readback_args.as_deref());
    append_optional_string(out, "event_type", capture.event_type.as_deref());
    append_optional_string(out, "event_id_path", capture.event_id_path.as_deref());
    append_optional_string(out, "json_flag", capture.json_flag.as_deref());
    append_optional_string(
        out,
        "last_message_flag",
        capture.last_message_flag.as_deref(),
    );
}

/// Render a Rust string as a TOML basic-string literal, escaping
/// backslashes, quotes, and control characters per the TOML spec.
/// Defers to the `toml` crate's serializer to avoid hand-rolling
/// escape rules that drift from the spec.
fn toml_string_literal(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn append_optional_string(out: &mut String, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        out.push_str(&format!("{key} = {}\n", toml_string_literal(value)));
    }
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
    fn parse_single_provider() {
        let toml = r#"
command = "codex"
args = ["exec", "-m", "gpt-5.3"]
prompt_mode = "arg"
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].command, "codex");
        assert_eq!(config.providers[0].args, vec!["exec", "-m", "gpt-5.3"]);
        assert_eq!(config.prompt_mode, PromptMode::Arg);
        assert!(config.inputs.is_empty());
    }

    #[test]
    fn parse_multi_provider() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec", "-m", "gpt-5.3-codex"]

[[providers]]
command = "codex2"
args = ["exec", "-m", "gpt-5.3-codex"]
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        assert_eq!(config.providers.len(), 2);
    }

    #[test]
    fn parse_defaults_to_stdin() {
        let toml = r#"
command = "claude"
args = ["-p"]
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        assert_eq!(config.prompt_mode, PromptMode::Stdin);
    }

    #[test]
    fn rejects_no_providers() {
        let toml = r#"
prompt_mode = "arg"
"#;
        assert!(ModelConfig::from_toml("test", toml).is_err());
    }

    /// `validate()` for ForcedFlagVerified must reject configs missing the
    /// required `flag` field — otherwise `build_capture_plan` would silently
    /// fall through to capture failure at execution time (V10).
    #[test]
    fn rejects_forced_flag_verified_without_flag() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "claude"
args = ["-p"]

[providers.session_capture]
kind = "forced_flag_verified"
"#;
        let err = ModelConfig::from_toml("test", toml).unwrap_err();
        assert!(err.contains("`flag`"), "{err}");
    }

    /// Same V10 reasoning for StdoutJsonEvent's required fields.
    #[test]
    fn rejects_stdout_json_event_without_required_fields() {
        let cases = [
            (
                "missing json_flag",
                "`json_flag`",
                r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec"]

[providers.session_capture]
kind = "stdout_json_event"
last_message_flag = "-o"
event_type = "thread.started"
event_id_path = "thread_id"
"#,
            ),
            (
                "missing last_message_flag",
                "`last_message_flag`",
                r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec"]

[providers.session_capture]
kind = "stdout_json_event"
json_flag = "--json"
event_type = "thread.started"
event_id_path = "thread_id"
"#,
            ),
            (
                "missing event_type",
                "`event_type`",
                r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec"]

[providers.session_capture]
kind = "stdout_json_event"
json_flag = "--json"
last_message_flag = "-o"
event_id_path = "thread_id"
"#,
            ),
            (
                "missing event_id_path",
                "`event_id_path`",
                r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec"]

[providers.session_capture]
kind = "stdout_json_event"
json_flag = "--json"
last_message_flag = "-o"
event_type = "thread.started"
"#,
            ),
        ];
        for (label, expected_substr, toml) in cases {
            match ModelConfig::from_toml("test", toml) {
                Ok(_) => panic!("{label}: expected validation failure, got Ok"),
                Err(e) => assert!(
                    e.contains(expected_substr),
                    "{label}: error must mention {expected_substr}, got: {e}"
                ),
            }
        }
    }

    #[test]
    fn roundtrip_single_provider() {
        let original = r#"
command = "codex"
args = ["exec", "-m", "gpt-5.3"]
prompt_mode = "arg"
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();
        assert_eq!(c1.providers[0].command, c2.providers[0].command);
        assert_eq!(c1.providers[0].args, c2.providers[0].args);
        assert_eq!(c1.prompt_mode, c2.prompt_mode);
    }

    #[test]
    fn roundtrip_multi_provider() {
        let original = r#"
prompt_mode = "stdin"

[[providers]]
command = "codex"
args = ["exec", "-m", "gpt-5.3-codex"]

[[providers]]
command = "codex2"
args = ["exec", "-m", "gpt-5.3-codex"]
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();
        assert_eq!(c1.providers.len(), c2.providers.len());
        assert_eq!(c1.providers[0].command, c2.providers[0].command);
        assert_eq!(c1.providers[1].command, c2.providers[1].command);
    }

    #[test]
    fn roundtrip_model_with_interactive_args() {
        let original = r#"
prompt_mode = "arg"

[[providers]]
name = "claude-interactive"
command = "env"
args = ["-u", "CLAUDECODE", "claude2", "-p", "--model", "opus"]
interactive_args = ["-u", "CLAUDECODE", "claude2", "--model", "opus"]
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();

        assert_eq!(
            c2.providers[0].interactive_args.as_deref(),
            Some(
                &[
                    "-u".to_string(),
                    "CLAUDECODE".to_string(),
                    "claude2".to_string(),
                    "--model".to_string(),
                    "opus".to_string(),
                ][..]
            )
        );
    }

    #[test]
    fn roundtrip_model_without_interactive_args_keeps_none() {
        let original = r#"
prompt_mode = "arg"

[[providers]]
name = "claude-interactive"
command = "claude"
args = ["-p"]
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();

        assert_eq!(c1.providers[0].interactive_args, None);
        assert_eq!(c2.providers[0].interactive_args, None);
    }

    #[test]
    fn rejects_empty_interactive_args_at_load_time() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
name = "claude-interactive"
command = "claude"
args = ["-p"]
interactive_args = []
"#;
        let err = ModelConfig::from_toml("test", toml).unwrap_err();
        assert!(err.contains("interactive_args"), "{err}");
    }

    #[test]
    fn parse_canonical_claude_interactive_shape() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
name = "claude2"
command = "env"
args = ["-u", "CLAUDECODE", "claude2", "-p", "--model", "opus", "--dangerously-skip-permissions"]
interactive_args = ["-u", "CLAUDECODE", "claude2", "--model", "opus", "--dangerously-skip-permissions"]
"#;
        let config = ModelConfig::from_toml("claude", toml).unwrap();
        let provider = &config.providers[0];

        assert_eq!(provider.name, "claude2");
        assert_eq!(provider.command, "env");
        assert_eq!(
            provider.args,
            vec![
                "-u",
                "CLAUDECODE",
                "claude2",
                "-p",
                "--model",
                "opus",
                "--dangerously-skip-permissions",
            ]
        );
        assert_eq!(
            provider.interactive_args.as_deref(),
            Some(
                &[
                    "-u".to_string(),
                    "CLAUDECODE".to_string(),
                    "claude2".to_string(),
                    "--model".to_string(),
                    "opus".to_string(),
                    "--dangerously-skip-permissions".to_string(),
                ][..]
            )
        );
    }

    #[test]
    fn parse_canonical_codex_interactive_shape() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
name = "codex"
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.4", "-c", "model_reasoning_effort=high"]
interactive_args = ["--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.4", "-c", "model_reasoning_effort=high"]
"#;
        let config = ModelConfig::from_toml("codex", toml).unwrap();
        let provider = &config.providers[0];

        assert_eq!(provider.name, "codex");
        assert_eq!(provider.command, "codex");
        assert_eq!(
            provider.args,
            vec![
                "exec",
                "--dangerously-bypass-approvals-and-sandbox",
                "-m",
                "gpt-5.4",
                "-c",
                "model_reasoning_effort=high",
            ]
        );
        assert_eq!(
            provider.interactive_args.as_deref(),
            Some(
                &[
                    "--dangerously-bypass-approvals-and-sandbox".to_string(),
                    "-m".to_string(),
                    "gpt-5.4".to_string(),
                    "-c".to_string(),
                    "model_reasoning_effort=high".to_string(),
                ][..]
            )
        );
    }

    #[test]
    fn roundtrip_model_with_flag_resume_strategy() {
        let original = r#"
prompt_mode = "arg"

[[providers]]
name = "claude-account"
command = "claude"
args = ["-p"]
interactive_args = ["--model", "opus"]

[providers.resume]
kind = "flag"
flag = "--resume"
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();

        let resume = c2.providers[0].resume.as_ref().unwrap();
        assert_eq!(resume.kind, ResumeKind::Flag);
        assert_eq!(resume.flag.as_deref(), Some("--resume"));
        assert_eq!(resume.subcommand, None);
    }

    #[test]
    fn roundtrip_model_with_subcommand_resume_strategy() {
        let original = r#"
prompt_mode = "arg"

[[providers]]
name = "codex"
command = "codex"
args = ["exec", "-m", "gpt-5.4"]
interactive_args = ["-m", "gpt-5.4"]

[providers.resume]
kind = "subcommand"
subcommand = ["resume"]
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();

        let resume = c2.providers[0].resume.as_ref().unwrap();
        assert_eq!(resume.kind, ResumeKind::Subcommand);
        assert_eq!(resume.flag, None);
        assert_eq!(
            resume.subcommand.as_deref(),
            Some(&["resume".to_string()][..])
        );
    }

    #[test]
    fn rejects_flag_resume_without_flag() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "claude"
args = ["-p"]
interactive_args = ["--model", "opus"]

[providers.resume]
kind = "flag"
"#;
        let err = ModelConfig::from_toml("test", toml).unwrap_err();
        assert!(err.contains("`flag`"), "{err}");
    }

    #[test]
    fn rejects_flag_resume_with_subcommand() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "claude"
args = ["-p"]
interactive_args = ["--model", "opus"]

[providers.resume]
kind = "flag"
flag = "--resume"
subcommand = ["resume"]
"#;
        let err = ModelConfig::from_toml("test", toml).unwrap_err();
        assert!(err.contains("subcommand"), "{err}");
    }

    #[test]
    fn rejects_subcommand_resume_without_subcommand() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec"]
interactive_args = ["-m", "gpt-5.4"]

[providers.resume]
kind = "subcommand"
"#;
        let err = ModelConfig::from_toml("test", toml).unwrap_err();
        assert!(err.contains("subcommand"), "{err}");
    }

    #[test]
    fn rejects_subcommand_resume_with_empty_subcommand() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec"]
interactive_args = ["-m", "gpt-5.4"]

[providers.resume]
kind = "subcommand"
subcommand = []
"#;
        let err = ModelConfig::from_toml("test", toml).unwrap_err();
        assert!(err.contains("subcommand"), "{err}");
    }

    #[test]
    fn rejects_subcommand_resume_with_flag() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec"]
interactive_args = ["-m", "gpt-5.4"]

[providers.resume]
kind = "subcommand"
flag = "--resume"
subcommand = ["resume"]
"#;
        let err = ModelConfig::from_toml("test", toml).unwrap_err();
        assert!(err.contains("flag"), "{err}");
    }

    #[test]
    fn parse_canonical_claude_resume_shape() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
name = "claude2"
command = "env"
args = ["-u", "CLAUDECODE", "claude2", "-p", "--model", "opus", "--dangerously-skip-permissions"]
interactive_args = ["-u", "CLAUDECODE", "claude2", "--model", "opus", "--dangerously-skip-permissions"]

[providers.resume]
kind = "flag"
flag = "--resume"
"#;
        let config = ModelConfig::from_toml("claude", toml).unwrap();
        let provider = &config.providers[0];
        let resume = provider.resume.as_ref().unwrap();

        assert_eq!(provider.name, "claude2");
        assert_eq!(resume.kind, ResumeKind::Flag);
        assert_eq!(resume.flag.as_deref(), Some("--resume"));
        assert_eq!(resume.subcommand, None);
    }

    #[test]
    fn parse_canonical_codex_resume_shape() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
name = "codex"
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.4", "-c", "model_reasoning_effort=high"]
interactive_args = ["--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.4", "-c", "model_reasoning_effort=high"]

[providers.resume]
kind = "subcommand"
subcommand = ["resume"]
"#;
        let config = ModelConfig::from_toml("codex", toml).unwrap();
        let provider = &config.providers[0];
        let resume = provider.resume.as_ref().unwrap();

        assert_eq!(provider.name, "codex");
        assert_eq!(resume.kind, ResumeKind::Subcommand);
        assert_eq!(resume.flag, None);
        assert_eq!(
            resume.subcommand.as_deref(),
            Some(&["resume".to_string()][..])
        );
    }

    #[test]
    fn parse_provider_with_forced_flag_verified_session_capture() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "claude"
args = ["-p"]

[providers.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"
readback_args = ["--verbose", "--output-format", "stream-json"]
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        let capture = config.providers[0].session_capture.as_ref().unwrap();

        assert_eq!(capture.kind, SessionCaptureKind::ForcedFlagVerified);
        assert_eq!(capture.flag.as_deref(), Some("--session-id"));
        assert_eq!(
            capture.readback_args.as_deref(),
            Some(
                [
                    "--verbose".to_string(),
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                ]
                .as_slice()
            )
        );
        assert_eq!(capture.event_type, None);
        assert_eq!(capture.event_id_path, None);
    }

    #[test]
    fn parse_provider_with_stdout_json_event_session_capture() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec"]

[providers.session_capture]
kind = "stdout_json_event"
json_flag = "--json"
last_message_flag = "-o"
event_type = "thread.started"
event_id_path = "thread_id"
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        let capture = config.providers[0].session_capture.as_ref().unwrap();

        assert_eq!(capture.kind, SessionCaptureKind::StdoutJsonEvent);
        assert_eq!(capture.json_flag.as_deref(), Some("--json"));
        assert_eq!(capture.last_message_flag.as_deref(), Some("-o"));
        assert_eq!(capture.event_type.as_deref(), Some("thread.started"));
        assert_eq!(capture.event_id_path.as_deref(), Some("thread_id"));
        assert_eq!(capture.flag, None);
        assert_eq!(capture.readback_args, None);
    }

    #[test]
    fn roundtrip_model_with_session_capture() {
        let original = r#"
prompt_mode = "arg"

[[providers]]
name = "codex-account"
command = "codex"
args = ["exec"]

[providers.session_capture]
kind = "stdout_json_event"
json_flag = "--json"
last_message_flag = "-o"
event_type = "thread.started"
event_id_path = "thread_id"
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();

        assert_eq!(
            c2.providers[0].session_capture.as_ref().unwrap().kind,
            SessionCaptureKind::StdoutJsonEvent
        );
        assert_eq!(
            c2.providers[0]
                .session_capture
                .as_ref()
                .unwrap()
                .event_type
                .as_deref(),
            Some("thread.started")
        );
        assert_eq!(
            c2.providers[0]
                .session_capture
                .as_ref()
                .unwrap()
                .last_message_flag
                .as_deref(),
            Some("-o")
        );
    }

    /// kind = "none" round-trip: parses cleanly, requires no extra
    /// fields, validate() accepts it.
    #[test]
    fn parse_and_roundtrip_kind_none() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "claude"
args = ["-p"]

[providers.session_capture]
kind = "none"
"#;
        let c1 = ModelConfig::from_toml("test", toml).unwrap();
        let capture = c1.providers[0].session_capture.as_ref().unwrap();
        assert_eq!(capture.kind, SessionCaptureKind::None);
        assert!(capture.flag.is_none());

        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();
        assert_eq!(
            c2.providers[0].session_capture.as_ref().unwrap().kind,
            SessionCaptureKind::None
        );
    }

    /// Forced-flag round-trip — the existing tests only round-trip the
    /// stdout_json_event variant. Cover the FFV variant explicitly.
    #[test]
    fn roundtrip_model_with_forced_flag_verified_session_capture() {
        let original = r#"
prompt_mode = "arg"

[[providers]]
name = "claude-account"
command = "claude"
args = ["-p"]

[providers.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"
readback_args = ["--verbose", "--output-format", "stream-json"]
"#;
        let c1 = ModelConfig::from_toml("test", original).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();

        let capture = c2.providers[0].session_capture.as_ref().unwrap();
        assert_eq!(capture.kind, SessionCaptureKind::ForcedFlagVerified);
        assert_eq!(capture.flag.as_deref(), Some("--session-id"));
        assert_eq!(
            capture.readback_args.as_deref(),
            Some(
                &[
                    "--verbose".to_string(),
                    "--output-format".to_string(),
                    "stream-json".to_string()
                ][..]
            )
        );
    }

    /// Unknown `kind` must be rejected — protects against typos like
    /// `forced_flag` or `json_event` that would silently disable
    /// capture if accepted as None.
    #[test]
    fn rejects_unknown_session_capture_kind() {
        let toml = r#"
prompt_mode = "arg"

[[providers]]
command = "claude"
args = ["-p"]

[providers.session_capture]
kind = "magic_capture"
flag = "--session-id"
"#;
        assert!(
            ModelConfig::from_toml("test", toml).is_err(),
            "unknown session_capture.kind must be rejected"
        );
    }

    /// Provider without a `session_capture` block has `None` — the
    /// behavior `pr-a` callers depend on for the no-capture path.
    #[test]
    fn absent_session_capture_is_none() {
        let toml = r#"
command = "claude"
args = ["-p"]
"#;
        let cfg = ModelConfig::from_toml("test", toml).unwrap();
        assert_eq!(cfg.providers.len(), 1);
        assert!(cfg.providers[0].session_capture.is_none());
    }

    #[test]
    fn parse_model_with_flagged_inputs() {
        let toml = r#"
command = "atlas-image"
prompt_mode = "stdin"

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true

[[inputs]]
name = "size"
type = "enum"
flag = "--size"
options = ["2048*2048", "1024*1024"]
default = "2048*2048"
description = "Image size"

[[inputs]]
name = "duration"
type = "integer"
flag = "--duration"
min = 5.0
max = 10.0
default = 5
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        assert_eq!(config.inputs.len(), 3);
        assert!(config.default_input().is_some());

        let size = &config.inputs[1];
        assert_eq!(size.flag.as_deref(), Some("--size"));
        assert!(matches!(&size.input_type, InputType::Enum { options } if options.len() == 2));

        let duration = &config.inputs[2];
        assert_eq!(duration.flag.as_deref(), Some("--duration"));
        match &duration.input_type {
            InputType::Integer { min, max } => {
                assert_eq!(*min, Some(5));
                assert_eq!(*max, Some(10));
            }
            _ => panic!("Expected integer type"),
        }
    }

    #[test]
    fn parse_model_with_array_input() {
        let toml = r#"
command = "atlas-edit"
prompt_mode = "stdin"

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true

[[inputs]]
name = "images"
type = "array"
flag = "--image"
item_type = "string"
required = true
min_items = 1
max_items = 14
"#;
        let config = ModelConfig::from_toml("test", toml).unwrap();
        let images = &config.inputs[1];
        assert_eq!(images.flag.as_deref(), Some("--image"));
        match &images.input_type {
            InputType::Array {
                min_items,
                max_items,
                ..
            } => {
                assert_eq!(*min_items, Some(1));
                assert_eq!(*max_items, Some(14));
            }
            _ => panic!("Expected array type"),
        }
    }

    #[test]
    fn rejects_duplicate_default_input() {
        let toml = r#"
command = "test"

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
command = "test"

[[inputs]]
name = "format"
type = "enum"
"#;
        let result = ModelConfig::from_toml("test", toml);
        assert!(result.unwrap_err().contains("requires 'options'"));
    }

    #[test]
    fn roundtrip_model_with_inputs() {
        let toml = r#"
command = "atlas-image"
prompt_mode = "stdin"

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true

[[inputs]]
name = "size"
type = "enum"
flag = "--size"
options = ["2048*2048", "1024*1024"]
default = "2048*2048"
"#;
        let c1 = ModelConfig::from_toml("test", toml).unwrap();
        let c2 = ModelConfig::from_toml("test", &c1.to_toml()).unwrap();
        assert_eq!(c1.inputs.len(), c2.inputs.len());
        assert_eq!(c1.inputs[0].name, c2.inputs[0].name);
        assert_eq!(c1.inputs[1].flag, c2.inputs[1].flag);
    }
}
