use super::claude_tool_filter::{ClaudeToolFilterShape, validate_proxy_claude_filter_shape};
use super::model::{
    InvocationMode, PromptMode, ProviderConfig, ResumeAcceptanceRules, ResumeStrategy,
    SessionCapture, SessionStorage, ToolRestrictionKind, ToolRestrictions,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

/// One entry in `providers.toml`, keyed by the provider name.
#[derive(Debug, Clone)]
pub struct ProviderEntry {
    /// Shell command that prints JSON on stdout describing rolling-quota
    /// windows. Empty if the provider has no quota check wired up.
    pub quota_script: Option<String>,
    /// Optional shell command that hits the provider's API and triggers the
    /// CLI's own OAuth token refresh (e.g. `claude auth status`,
    /// `codex login status`). Run when `quota_script` fails or returns an
    /// empty windows list on a previously-populated provider, then
    /// `quota_script` is retried once. Provider-agnostic: the runner does
    /// not implement OAuth itself; it delegates to the CLI.
    pub auth_refresh_command: Option<String>,
    /// Base executable for this provider account. Model-specific flags are
    /// appended from the selected model TOML at spawn time.
    pub command: Option<String>,
    pub args: Vec<String>,
    pub interactive_args: Option<Vec<String>>,
    pub prompt_mode: PromptMode,
    pub resume: Option<ResumeStrategy>,
    pub session_capture: Option<SessionCapture>,
    pub resume_acceptance: Option<ResumeAcceptanceRules>,
    pub session_storage: Option<SessionStorage>,
    pub system_prompt_override: Option<String>,
    pub tool_restrictions: Option<ToolRestrictions>,
    pub invocation_mode: InvocationMode,
}

impl Default for ProviderEntry {
    fn default() -> Self {
        Self {
            quota_script: None,
            auth_refresh_command: None,
            command: None,
            args: Vec::new(),
            interactive_args: None,
            prompt_mode: PromptMode::Stdin,
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

type RawProvidersToml = HashMap<String, RawEntry>;

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Toml(String),
    InvocationMode { provider: String, value: String },
    UnsafeProxyClaudeToolsRestrict { provider: String },
    Other(String),
}

impl LoadError {
    pub fn contains(&self, needle: &str) -> bool {
        self.to_string().contains(needle)
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error reading providers.toml: {err}"),
            Self::Toml(err) => write!(f, "toml parse error in providers.toml: {err}"),
            Self::InvocationMode { provider, value } => write!(
                f,
                "provider {provider}: invalid invocation_mode `{value}`; valid modes are: `headless`, `proxy`"
            ),
            Self::UnsafeProxyClaudeToolsRestrict { provider } => write!(
                f,
                "provider {provider}: proxy-mode Claude must not use `--tools mcp__...`; use `--allowedTools` or omit the filter (AGE-104)"
            ),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<LoadError> for String {
    fn from(value: LoadError) -> Self {
        value.to_string()
    }
}

impl PartialEq for LoadError {
    fn eq(&self, other: &Self) -> bool {
        use LoadError::*;
        match (self, other) {
            (Io(left), Io(right)) => left.to_string() == right.to_string(),
            (Toml(left), Toml(right)) => left == right,
            (
                InvocationMode {
                    provider: left_provider,
                    value: left_value,
                },
                InvocationMode {
                    provider: right_provider,
                    value: right_value,
                },
            ) => left_provider == right_provider && left_value == right_value,
            (
                UnsafeProxyClaudeToolsRestrict { provider: left },
                UnsafeProxyClaudeToolsRestrict { provider: right },
            ) => left == right,
            (Other(left), Other(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for LoadError {}

#[derive(Debug, Clone, Default)]
pub struct ProvidersConfig {
    pub entries: HashMap<String, ProviderEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawEntry {
    #[serde(default)]
    quota_script: Option<String>,
    #[serde(default)]
    auth_refresh_command: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    interactive_args: Option<Vec<String>>,
    #[serde(default)]
    prompt_mode: Option<String>,
    #[serde(default)]
    resume: Option<ResumeStrategy>,
    #[serde(default)]
    session_capture: Option<SessionCapture>,
    #[serde(default)]
    resume_acceptance: Option<ResumeAcceptanceRules>,
    #[serde(default)]
    session_storage: Option<SessionStorage>,
    #[serde(default)]
    system_prompt_override: Option<String>,
    #[serde(default)]
    tool_restrictions: Option<ToolRestrictions>,
    #[serde(default)]
    invocation_mode: Option<String>,
}

#[allow(dead_code)]
fn parse_providers_toml(text: &str) -> Result<RawProvidersToml, LoadError> {
    toml::from_str(text).map_err(|err| LoadError::Toml(err.to_string()))
}

#[allow(dead_code)]
fn apply_defaults_to_raw_providers(mut raw: RawProvidersToml) -> RawProvidersToml {
    for entry in raw.values_mut() {
        entry
            .invocation_mode
            .get_or_insert_with(|| "headless".to_string());
    }
    raw
}

#[allow(dead_code)]
fn validate_providers_config(raw: &RawProvidersToml) -> Result<(), LoadError> {
    for (name, raw_entry) in raw {
        let invocation_mode = validate_provider_invocation_mode_values(name, raw_entry)?;
        validate_provider_argv_shapes(name, raw_entry, invocation_mode)?;
        let entry = raw_entry_to_entry(raw_entry, invocation_mode);
        validate_provider_entry_shape(name, &entry)?;
    }
    Ok(())
}

fn validate_provider_invocation_mode_values(
    name: &str,
    raw: &RawEntry,
) -> Result<InvocationMode, LoadError> {
    parse_invocation_mode(name, raw.invocation_mode.as_deref())
}

fn parse_invocation_mode(name: &str, raw_mode: Option<&str>) -> Result<InvocationMode, LoadError> {
    match raw_mode.unwrap_or("headless") {
        "headless" => Ok(InvocationMode::Headless),
        "proxy" => Ok(InvocationMode::Proxy),
        value => Err(LoadError::InvocationMode {
            provider: name.to_string(),
            value: value.to_string(),
        }),
    }
}

fn raw_entry_to_entry(raw: &RawEntry, invocation_mode: InvocationMode) -> ProviderEntry {
    ProviderEntry {
        quota_script: raw.quota_script.clone(),
        auth_refresh_command: raw.auth_refresh_command.clone(),
        command: raw.command.clone(),
        args: raw.args.clone(),
        interactive_args: raw.interactive_args.clone(),
        prompt_mode: raw
            .prompt_mode
            .as_deref()
            .map(parse_prompt_mode)
            .unwrap_or(PromptMode::Stdin),
        resume: raw.resume.clone(),
        session_capture: raw.session_capture.clone(),
        resume_acceptance: raw.resume_acceptance.clone(),
        session_storage: raw.session_storage.clone(),
        system_prompt_override: raw.system_prompt_override.clone(),
        tool_restrictions: raw.tool_restrictions.clone(),
        invocation_mode,
    }
}

fn validate_provider_entry_shape(name: &str, entry: &ProviderEntry) -> Result<(), LoadError> {
    entry.validate(name).map_err(LoadError::Other)
}

fn validate_provider_argv_shapes(
    name: &str,
    raw: &RawEntry,
    mode: InvocationMode,
) -> Result<(), LoadError> {
    validate_proxy_claude_argv_shape(name, raw, mode)
}

fn validate_proxy_claude_argv_shape(
    name: &str,
    raw: &RawEntry,
    mode: InvocationMode,
) -> Result<(), LoadError> {
    if !is_claude_proxy_provider(name, raw) {
        return Ok(());
    }

    let argv = build_provider_argv_tokens(raw);
    let shape =
        ClaudeToolFilterShape::detect_in_argv(&argv).unwrap_or(ClaudeToolFilterShape::NoFilter);
    validate_proxy_claude_filter_shape(mode, shape).map_err(|_| {
        LoadError::UnsafeProxyClaudeToolsRestrict {
            provider: name.to_string(),
        }
    })
}

fn is_claude_proxy_provider(name: &str, raw: &RawEntry) -> bool {
    matches!(
        provider_family(
            name,
            raw.command.as_deref(),
            &raw.args,
            raw.interactive_args.as_deref(),
        ),
        Some(ToolRestrictionKind::Claude)
    )
}

fn build_provider_argv_tokens(raw: &RawEntry) -> Vec<String> {
    let mut argv = Vec::new();
    if let Some(command) = raw.command.as_deref() {
        argv.extend(shell_split(command));
    }
    argv.extend(raw.args.iter().cloned());
    if let Some(interactive_args) = raw.interactive_args.as_ref() {
        argv.extend(interactive_args.iter().cloned());
    }
    argv
}

impl ProvidersConfig {
    /// Parse a providers.toml, returning an empty config if the file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, LoadError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => return Err(LoadError::Io(err)),
        };
        let raw = parse_providers_toml(&text)?;
        let raw = apply_defaults_to_raw_providers(raw);
        validate_providers_config(&raw)?;
        Ok(Self::from_validated_raw(raw))
    }

    fn from_validated_raw(raw: RawProvidersToml) -> Self {
        let entries = raw
            .into_iter()
            .map(|(name, raw_entry)| {
                let invocation_mode =
                    parse_invocation_mode(&name, raw_entry.invocation_mode.as_deref())
                        .expect("validated provider invocation_mode");
                (name, raw_entry_to_entry(&raw_entry, invocation_mode))
            })
            .collect();
        Self { entries }
    }

    pub fn get(&self, name: &str) -> Option<&ProviderEntry> {
        self.entries.get(name)
    }

    pub fn effective_provider(
        &self,
        model_provider: &ProviderConfig,
    ) -> Result<(ProviderConfig, PromptMode), String> {
        let runtime = self.get(&model_provider.name).ok_or_else(|| {
            format!(
                "provider {} is missing from providers.toml",
                model_provider.name
            )
        })?;
        runtime.effective_provider(&model_provider.name, Some(model_provider))
    }

    pub fn runtime_provider(&self, name: &str) -> Result<(ProviderConfig, PromptMode), String> {
        let runtime = self
            .get(name)
            .ok_or_else(|| format!("provider {name} is missing from providers.toml"))?;
        runtime.effective_provider(name, None)
    }
}

pub fn migrate_legacy_session_storage_file(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    migrate_legacy_session_storage_content(path, &content)
}

fn migrate_legacy_session_storage_content(path: &Path, content: &str) -> Result<bool, String> {
    let mut value: toml::Value = toml::from_str(content)
        .map_err(|e| format!("TOML parse error in {}: {e}", path.display()))?;
    let Some(root) = value.as_table_mut() else {
        return Ok(false);
    };

    let mut migrated = Vec::new();
    for (provider_name, provider_value) in root.iter_mut() {
        let Some(provider_table) = provider_value.as_table_mut() else {
            continue;
        };
        let Some(storage) = provider_table
            .get_mut("session_storage")
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        let Some(kind) = storage.get("kind").and_then(toml::Value::as_str) else {
            continue;
        };
        let migrated_scripts = match kind {
            "claude_code" => storage
                .get("projects_dir")
                .and_then(toml::Value::as_str)
                .map(|projects_dir| {
                    (
                        format!("claude-code-cwd {}", shell_word(projects_dir)),
                        format!("claude-code-locate-transcript {}", shell_word(projects_dir)),
                        "claude_code",
                    )
                }),
            "codex" => storage
                .get("sessions_dir")
                .and_then(toml::Value::as_str)
                .map(|sessions_dir| {
                    (
                        format!("codex-cwd {}", shell_word(sessions_dir)),
                        format!("codex-locate-transcript {}", shell_word(sessions_dir)),
                        "codex_session",
                    )
                }),
            _ => None,
        };
        let Some((cwd_script, transcript_script, storage_type)) = migrated_scripts else {
            continue;
        };
        storage.insert(
            "kind".to_string(),
            toml::Value::String("script".to_string()),
        );
        storage.insert("cwd_script".to_string(), toml::Value::String(cwd_script));
        storage.insert(
            "transcript_script".to_string(),
            toml::Value::String(transcript_script),
        );
        storage.insert(
            "storage_type".to_string(),
            toml::Value::String(storage_type.to_string()),
        );
        storage.remove("projects_dir");
        storage.remove("sessions_dir");
        migrated.push(provider_name.clone());
    }

    if migrated.is_empty() {
        return Ok(false);
    }

    let new_content = toml::to_string_pretty(&value)
        .map_err(|e| format!("Failed to serialize migrated {}: {e}", path.display()))?;
    fs::write(path, &new_content)
        .map_err(|e| format!("Failed to write migrated {}: {e}", path.display()))?;
    tracing::warn!(
        "[config] warning: migrated legacy session_storage for providers {} to kind = \"script\" + cwd_script in {}",
        migrated.join(", "),
        path.display()
    );
    Ok(true)
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

#[allow(dead_code)]
impl ProviderEntry {
    fn validate(&self, name: &str) -> Result<(), String> {
        if let Some(resume) = &self.resume {
            resume
                .validate()
                .map_err(|e| format!("providers.toml provider {name}: {e}"))?;
        }
        if let Some(capture) = &self.session_capture {
            capture
                .validate()
                .map_err(|e| format!("providers.toml provider {name}: {e}"))?;
        }
        if let Some(storage) = &self.session_storage {
            storage
                .validate()
                .map_err(|e| format!("providers.toml provider {name}: {e}"))?;
        }
        self.validate_tool_restrictions(name)?;
        Ok(())
    }

    fn validate_tool_restrictions(&self, name: &str) -> Result<(), String> {
        let Some(restrictions) = &self.tool_restrictions else {
            return Ok(());
        };

        if let Some(family) = provider_family(
            name,
            self.command.as_deref(),
            &self.args,
            self.interactive_args.as_deref(),
        ) && restrictions.kind != family
        {
            return Err(format!(
                "providers.toml provider {name}: tool_restrictions.kind mismatch: discovered provider family {family}, configured kind {}",
                restrictions.kind
            ));
        }

        if restrictions.kind == ToolRestrictionKind::Claude {
            if !restrictions.codex.is_empty() {
                return Err(format!(
                    "providers.toml provider {name}: tool_restrictions.codex must be empty when kind = \"claude\""
                ));
            }
            if !restrictions.claude.allowed_tools.is_empty()
                && !restrictions.claude.disallowed_tools.is_empty()
            {
                return Err(format!(
                    "providers.toml provider {name}: tool_restrictions.claude.allowed_tools and tool_restrictions.claude.disallowed_tools are mutually exclusive"
                ));
            }
            if let Some(command) = self.command.as_deref() {
                let command_tokens = shell_split(command);
                validate_claude_duplicates(name, &command_tokens, "command", restrictions)?;
            }
            validate_claude_duplicates(name, &self.args, "args", restrictions)?;
            if let Some(interactive_args) = self.interactive_args.as_deref() {
                validate_claude_duplicates(
                    name,
                    interactive_args,
                    "interactive_args",
                    restrictions,
                )?;
            }
        }

        if restrictions.kind == ToolRestrictionKind::Codex {
            if !restrictions.claude.is_empty() {
                return Err(format!(
                    "providers.toml provider {name}: tool_restrictions.claude must be empty when kind = \"codex\""
                ));
            }
            if let Some(command) = self.command.as_deref() {
                let command_tokens = shell_split(command);
                validate_codex_duplicates(name, &command_tokens, "command", restrictions)?;
            }
            validate_codex_duplicates(name, &self.args, "args", restrictions)?;
            if let Some(interactive_args) = self.interactive_args.as_deref() {
                validate_codex_duplicates(
                    name,
                    interactive_args,
                    "interactive_args",
                    restrictions,
                )?;
            }
            for pair in &restrictions.codex.config_pairs {
                let key = pair.split_once('=').map(|(key, _)| key).unwrap_or(pair);
                if !CODEX_CONFIG_PAIR_ALLOWLIST.contains(&key) {
                    return Err(format!(
                        "providers.toml provider {name}: tool_restrictions.codex.config_pairs key {key:?} has no allowlisted Codex config pair"
                    ));
                }
            }
            for feature in &restrictions.codex.disabled_features {
                if !CODEX_DISABLED_FEATURE_ALLOWLIST.contains(&feature.as_str()) {
                    return Err(format!(
                        "providers.toml provider {name}: tool_restrictions.codex.disabled_features feature {feature:?} is not allowlisted"
                    ));
                }
            }
        }

        Ok(())
    }

    pub fn effective_provider(
        &self,
        name: &str,
        model_provider: Option<&ProviderConfig>,
    ) -> Result<(ProviderConfig, PromptMode), String> {
        let command = self
            .command
            .clone()
            .ok_or_else(|| format!("provider {name} has no command in providers.toml"))?;
        let mut args = self.args.clone();
        if let Some(model_provider) = model_provider {
            args.extend(model_provider.args.clone());
        }
        let interactive_args = self.interactive_args.as_ref().map(|base_interactive_args| {
            let mut args = base_interactive_args.clone();
            if let Some(model_provider) = model_provider
                && let Some(model_args) = model_provider.interactive_args.as_ref()
            {
                args.extend(model_args.clone());
            }
            args
        });
        Ok((
            ProviderConfig {
                name: name.to_string(),
                command,
                args,
                interactive_args,
                resume: self.resume.clone(),
                session_capture: self.session_capture.clone(),
                resume_acceptance: self.resume_acceptance.clone(),
                session_storage: self.session_storage.clone(),
                system_prompt_override: self.system_prompt_override.clone(),
                tool_restrictions: self.tool_restrictions.clone(),
                invocation_mode: self.invocation_mode,
            },
            self.prompt_mode,
        ))
    }
}

#[allow(dead_code)]
const CODEX_CONFIG_PAIR_ALLOWLIST: &[&str] = &[];
#[allow(dead_code)]
const CODEX_DISABLED_FEATURE_ALLOWLIST: &[&str] = &["web_search"];

#[allow(dead_code)]
fn provider_family(
    name: &str,
    command: Option<&str>,
    args: &[String],
    interactive_args: Option<&[String]>,
) -> Option<ToolRestrictionKind> {
    let mut tokens = Vec::new();
    if let Some(command) = command {
        tokens.extend(shell_split(command));
    }
    tokens.extend(args.iter().cloned());
    if let Some(interactive_args) = interactive_args {
        tokens.extend(interactive_args.iter().cloned());
    }

    executable_token(tokens.iter().map(String::as_str))
        .or(Some(name))
        .and_then(|token| {
            let basename = token.rsplit('/').next().unwrap_or(token);
            if basename.starts_with("claude") {
                Some(ToolRestrictionKind::Claude)
            } else if basename.starts_with("codex") {
                Some(ToolRestrictionKind::Codex)
            } else {
                None
            }
        })
}

#[allow(dead_code)]
fn executable_token<'a>(tokens: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut tokens = tokens.peekable();
    while let Some(token) = tokens.next() {
        if token == "env" || token.ends_with("/env") {
            continue;
        }
        if let Some(rest) = token.strip_prefix("--") {
            if rest.is_empty() {
                continue;
            }
            continue;
        }
        if let Some(rest) = token.strip_prefix('-') {
            if matches!(rest, "u" | "e" | "S") {
                let _ = tokens.next();
            }
            continue;
        }
        if token.contains('=') && token.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            continue;
        }
        return Some(token);
    }
    None
}

pub(crate) fn shell_split(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[allow(dead_code)]
fn validate_claude_duplicates(
    name: &str,
    args: &[String],
    field: &str,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    validate_claude_tool_duplicates(
        name,
        args,
        field,
        "tool_restrictions.claude.allowed_tools",
        &["--allowedTools", "--allowed-tools"],
        &restrictions.claude.allowed_tools,
    )?;
    validate_claude_tool_duplicates(
        name,
        args,
        field,
        "tool_restrictions.claude.disallowed_tools",
        &["--disallowedTools", "--disallowed-tools"],
        &restrictions.claude.disallowed_tools,
    )
}

#[allow(dead_code)]
fn validate_claude_tool_duplicates(
    name: &str,
    args: &[String],
    arg_field: &str,
    policy_field: &str,
    flags: &[&str],
    policy_tools: &[String],
) -> Result<(), String> {
    for (flag, raw_value) in claude_flag_values(args, flags) {
        for raw_tool in raw_value
            .split(',')
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
        {
            if policy_tools.iter().any(|tool| tool == raw_tool) {
                return Err(format!(
                    "providers.toml provider {name}: {policy_field} contains duplicate tool {raw_tool:?} already present in {arg_field} flag {flag}"
                ));
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn claude_flag_values<'a>(args: &'a [String], flags: &[&str]) -> Vec<(String, &'a str)> {
    let mut values = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(flag) = flags.iter().find(|flag| arg == **flag)
            && let Some(value) = args.get(i + 1)
        {
            values.push(((*flag).to_string(), value.as_str()));
            i += 2;
            continue;
        }
        if let Some((flag, value)) = arg.split_once('=')
            && flags.contains(&flag)
        {
            values.push((flag.to_string(), value));
        }
        i += 1;
    }
    values
}

#[allow(dead_code)]
fn validate_codex_duplicates(
    name: &str,
    args: &[String],
    field: &str,
    restrictions: &ToolRestrictions,
) -> Result<(), String> {
    validate_codex_config_duplicates(name, args, field, &restrictions.codex.config_pairs)?;
    validate_codex_disabled_feature_duplicates(
        name,
        args,
        field,
        &restrictions.codex.disabled_features,
    )
}

#[allow(dead_code)]
fn validate_codex_config_duplicates(
    name: &str,
    args: &[String],
    field: &str,
    policy_pairs: &[String],
) -> Result<(), String> {
    for (flag, raw_value) in codex_flag_values(args, &["-c", "--config"]) {
        let raw_key = raw_value
            .split_once('=')
            .map(|(key, _)| key)
            .unwrap_or(raw_value);
        for policy_pair in policy_pairs {
            let policy_key = policy_pair
                .split_once('=')
                .map(|(key, _)| key)
                .unwrap_or(policy_pair);
            if policy_key == raw_key {
                return Err(format!(
                    "providers.toml provider {name}: tool_restrictions.codex.config_pairs contains duplicate key {raw_key:?} already present in {field} flag {flag}"
                ));
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn validate_codex_disabled_feature_duplicates(
    name: &str,
    args: &[String],
    field: &str,
    policy_features: &[String],
) -> Result<(), String> {
    for (flag, raw_value) in codex_flag_values(args, &["--disable"]) {
        for raw_feature in raw_value
            .split(',')
            .map(str::trim)
            .filter(|feature| !feature.is_empty())
        {
            if policy_features.iter().any(|feature| feature == raw_feature) {
                return Err(format!(
                    "providers.toml provider {name}: tool_restrictions.codex.disabled_features contains duplicate feature {raw_feature:?} already present in {field} flag {flag}"
                ));
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn codex_flag_values<'a>(args: &'a [String], flags: &[&str]) -> Vec<(String, &'a str)> {
    let mut values = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(flag) = flags.iter().find(|flag| arg == **flag)
            && let Some(value) = args.get(i + 1)
        {
            values.push(((*flag).to_string(), value.as_str()));
            i += 2;
            continue;
        }
        if let Some((flag, value)) = arg.split_once('=')
            && flags.contains(&flag)
        {
            values.push((flag.to_string(), value));
        }
        i += 1;
    }
    values
}

pub fn parse_prompt_mode(s: &str) -> PromptMode {
    match s {
        "arg" => PromptMode::Arg,
        _ => PromptMode::Stdin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ClaudeRestrictions, CodexRestrictions, ToolRestrictionKind, ToolRestrictions,
        derive_provider_name,
    };
    use std::io::Write;

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

    #[test]
    fn providers_config_load_orchestrates_parse_default_validate_construct() {
        let body = function_body(
            include_str!("providers.rs"),
            "pub fn load(path: &Path) -> Result<Self, LoadError>",
        );

        assert_contains_in_order(
            body,
            &[
                "fs::read_to_string",
                "parse_providers_toml",
                "apply_defaults_to_raw_providers",
                "validate_providers_config",
                "Self::from_validated_raw",
            ],
        );
        assert_forbidden_absent(
            body,
            &[
                "toml::from_str",
                "ProviderEntry {",
                "parse_prompt_mode",
                "InvocationMode::",
                "entry.validate",
            ],
        );
    }

    #[test]
    fn parses_quota_scripts() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
quota_script = "anthropic-usage ~/.claude/.credentials.json"

[claude2]
quota_script = "anthropic-usage ~/.claude2/.credentials.json"
"#
        )
        .unwrap();
        let cfg = ProvidersConfig::load(f.path()).unwrap();
        assert_eq!(cfg.entries.len(), 2);
        assert!(
            cfg.get("claude")
                .unwrap()
                .quota_script
                .as_deref()
                .unwrap()
                .contains("anthropic-usage")
        );
    }

    #[test]
    fn parses_auth_refresh_command() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
quota_script         = "anthropic-usage ~/.claude/.credentials.json"
auth_refresh_command = "claude auth status"

[claude2]
quota_script = "anthropic-usage ~/.claude2/.credentials.json"
"#
        )
        .unwrap();
        let cfg = ProvidersConfig::load(f.path()).unwrap();
        assert_eq!(
            cfg.get("claude").unwrap().auth_refresh_command.as_deref(),
            Some("claude auth status")
        );
        assert!(cfg.get("claude2").unwrap().auth_refresh_command.is_none());
    }

    #[test]
    fn parses_runtime_provider_config() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude2]
command = "env"
args = ["-u", "CLAUDECODE", "claude2"]
interactive_args = ["-u", "CLAUDECODE", "claude2"]
prompt_mode = "stdin"

[claude2.resume]
kind = "flag"
flag = "--resume"

[claude2.session_storage]
kind = "claude_code"
projects_dir = "/tmp/claude2/projects"
"#
        )
        .unwrap();
        let cfg = ProvidersConfig::load(f.path()).unwrap();
        let model_provider = ProviderConfig {
            name: "claude2".to_string(),
            command: String::new(),
            args: vec!["--model".to_string(), "opus".to_string()],
            interactive_args: Some(vec!["--model".to_string(), "opus".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let (provider, prompt_mode) = cfg.effective_provider(&model_provider).unwrap();
        assert_eq!(prompt_mode, PromptMode::Stdin);
        assert_eq!(provider.command, "env");
        assert_eq!(
            provider.args,
            ["-u", "CLAUDECODE", "claude2", "--model", "opus"]
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
                ][..]
            )
        );
        assert!(provider.resume.is_some());
        assert!(provider.session_storage.is_some());
    }

    #[test]
    fn providers_toml_parses_age28_policy_fields() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
command = "env -u CLAUDECODE claude"
args = ["-p"]
system_prompt_override = """
Do not use the Task tool.
Use agents -m <model> -f <prompt-file>.
"""

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
disallowed_tools = ["Task", "Task tool"]
disable_slash_commands = true
"#
        )
        .unwrap();

        let cfg = ProvidersConfig::load(f.path()).unwrap();
        let entry = cfg.get("claude").unwrap();

        assert_eq!(
            entry.system_prompt_override.as_deref(),
            Some("Do not use the Task tool.\nUse agents -m <model> -f <prompt-file>.\n")
        );
        assert_eq!(
            entry.tool_restrictions,
            Some(ToolRestrictions {
                kind: ToolRestrictionKind::Claude,
                claude: ClaudeRestrictions {
                    disallowed_tools: vec!["Task".to_string(), "Task tool".to_string()],
                    allowed_tools: Vec::new(),
                    disable_slash_commands: true,
                },
                codex: CodexRestrictions::default(),
            })
        );
    }

    #[test]
    fn providers_toml_age28_fields_default_to_none() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
command = "claude"
args = ["-p"]
"#
        )
        .unwrap();

        let cfg = ProvidersConfig::load(f.path()).unwrap();
        let entry = cfg.get("claude").unwrap();

        assert!(entry.system_prompt_override.is_none());
        assert!(entry.tool_restrictions.is_none());
    }

    #[test]
    fn missing_file_is_empty_config() {
        let cfg = ProvidersConfig::load(Path::new("/nonexistent/path/providers.toml")).unwrap();
        assert!(cfg.entries.is_empty());
    }

    #[test]
    fn effective_provider_missing_provider_returns_named_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[other-provider]
command = "other"
"#
        )
        .unwrap();
        let cfg = ProvidersConfig::load(f.path()).unwrap();
        let model_provider = ProviderConfig::model_provider("missing-provider", vec![]);

        let err = cfg.effective_provider(&model_provider).unwrap_err();

        assert!(
            err.contains("provider missing-provider is missing from providers.toml"),
            "{err}"
        );
    }

    #[test]
    fn effective_provider_carries_runtime_fields() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[provider]
command = "/bin/echo"
args = ["--provider"]
interactive_args = ["interactive"]
prompt_mode = "arg"

[provider.resume]
kind = "flag"
flag = "--resume"

[provider.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[provider.resume_acceptance]
accepted_output_patterns = ["accepted"]
rejected_output_patterns = ["rejected"]

[provider.session_storage]
kind = "script"
cwd_script = "codex-cwd /tmp/codex-sessions"
"#
        )
        .unwrap();
        let cfg = ProvidersConfig::load(f.path()).unwrap();
        let model_provider = ProviderConfig {
            name: "provider".to_string(),
            command: String::new(),
            args: vec!["--model".to_string()],
            interactive_args: Some(vec!["interactive-model".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };

        let (provider, prompt_mode) = cfg.effective_provider(&model_provider).unwrap();

        assert_eq!(prompt_mode, PromptMode::Arg);
        assert_eq!(provider.name, "provider");
        assert_eq!(provider.command, "/bin/echo");
        assert_eq!(provider.args, ["--provider", "--model"]);
        assert_eq!(
            provider.interactive_args.as_deref(),
            Some(&["interactive".to_string(), "interactive-model".to_string()][..])
        );
        assert_eq!(
            provider.resume.as_ref().unwrap().flag.as_deref(),
            Some("--resume")
        );
        assert_eq!(
            provider.session_capture.as_ref().unwrap().flag.as_deref(),
            Some("--session-id")
        );
        assert_eq!(
            provider
                .resume_acceptance
                .as_ref()
                .unwrap()
                .accepted_output_patterns
                .as_deref(),
            Some(&["accepted".to_string()][..])
        );
        assert!(provider.session_storage.is_some());
    }

    #[test]
    fn parses_script_session_storage() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[provider]
command = "/bin/echo"

[provider.session_storage]
kind = "script"
cwd_script = "fixture-cwd ~/.fixture/sessions"
"#
        )
        .unwrap();

        let migrated = migrate_legacy_session_storage_file(f.path()).unwrap();
        assert!(!migrated);
        let cfg = ProvidersConfig::load(f.path()).unwrap();

        let storage = cfg
            .get("provider")
            .unwrap()
            .session_storage
            .as_ref()
            .unwrap();
        assert_eq!(storage.cwd_script(), "fixture-cwd ~/.fixture/sessions");
        assert_eq!(storage.transcript_script(), None);
        assert_eq!(storage.script_storage_type(), None);
    }

    #[test]
    fn migrates_legacy_claude_code_storage_to_script_storage() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[provider]
command = "/bin/echo"

[provider.session_storage]
kind = "claude_code"
projects_dir = "/tmp/provider/projects"
"#
        )
        .unwrap();

        let migrated = migrate_legacy_session_storage_file(f.path()).unwrap();
        let cfg = ProvidersConfig::load(f.path()).unwrap();

        let storage = cfg
            .get("provider")
            .unwrap()
            .session_storage
            .as_ref()
            .unwrap();
        assert_eq!(
            storage.cwd_script(),
            "claude-code-cwd /tmp/provider/projects"
        );
        assert_eq!(
            storage.transcript_script(),
            Some("claude-code-locate-transcript /tmp/provider/projects")
        );
        assert_eq!(
            storage.script_storage_type(),
            Some(crate::ScriptSessionStorageType::ClaudeCode)
        );
        assert!(migrated);
        let migrated_content = std::fs::read_to_string(f.path()).unwrap();
        assert!(migrated_content.contains("kind = \"script\""));
        assert!(
            migrated_content.contains("cwd_script = \"claude-code-cwd /tmp/provider/projects\"")
        );
        assert!(migrated_content.contains(
            "transcript_script = \"claude-code-locate-transcript /tmp/provider/projects\""
        ));
        assert!(migrated_content.contains("storage_type = \"claude_code\""));
        assert!(!migrated_content.contains("projects_dir"));
    }

    #[test]
    fn effective_provider_carries_age28_policy_fields() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
command = "env -u CLAUDECODE claude"
args = ["-p", "--provider-root"]
interactive_args = ["--provider-interactive"]
system_prompt_override = "root override"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#
        )
        .unwrap();
        let cfg = ProvidersConfig::load(f.path()).unwrap();
        let model_provider = ProviderConfig::model_provider(
            "claude",
            vec!["--model".to_string(), "opus".to_string()],
        );

        let (provider, prompt_mode) = cfg.effective_provider(&model_provider).unwrap();

        assert_eq!(prompt_mode, PromptMode::Stdin);
        assert_eq!(provider.args, ["-p", "--provider-root", "--model", "opus"]);
        assert_eq!(
            provider.system_prompt_override.as_deref(),
            Some("root override")
        );
        assert_eq!(
            provider.tool_restrictions,
            Some(ToolRestrictions {
                kind: ToolRestrictionKind::Claude,
                claude: ClaudeRestrictions {
                    disallowed_tools: vec!["Task".to_string()],
                    allowed_tools: Vec::new(),
                    disable_slash_commands: false,
                },
                codex: CodexRestrictions::default(),
            })
        );
    }

    #[test]
    fn provider_validate_rejects_kind_mismatch() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
command = "env -u CLAUDECODE claude"
args = ["-p"]

[claude.tool_restrictions]
kind = "codex"
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(f.path()).unwrap_err();

        assert!(err.contains("providers.toml provider claude"), "{err}");
        assert!(err.contains("kind"), "{err}");
        assert!(err.contains("claude"), "{err}");
        assert!(err.contains("codex"), "{err}");
    }

    #[test]
    fn provider_validate_rejects_interactive_only_kind_mismatch() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[primary]
command = "env"
interactive_args = ["-u", "FOO", "codex", "exec"]

[primary.tool_restrictions]
kind = "claude"
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(f.path()).unwrap_err();

        assert!(err.contains("providers.toml provider primary"), "{err}");
        assert!(err.contains("kind"), "{err}");
        assert!(err.contains("claude"), "{err}");
        assert!(err.contains("codex"), "{err}");
    }

    #[test]
    fn provider_validate_rejects_duplicate_claude_tool_flag() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
command = "claude"
args = ["-p", "--allowedTools=Bash"]

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
allowed_tools = ["Bash"]
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(f.path()).unwrap_err();

        assert!(err.contains("providers.toml provider claude"), "{err}");
        assert!(
            err.contains("tool_restrictions.claude.allowed_tools"),
            "{err}"
        );
        assert!(err.contains("Bash"), "{err}");
        assert!(err.contains("--allowedTools"), "{err}");
    }

    #[test]
    fn provider_validate_rejects_duplicate_claude_tool_flag_in_command() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
command = "claude --allowed-tools Bash"
args = ["-p"]

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
allowed_tools = ["Bash"]
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(f.path()).unwrap_err();

        assert!(err.contains("providers.toml provider claude"), "{err}");
        assert!(
            err.contains("tool_restrictions.claude.allowed_tools"),
            "{err}"
        );
        assert!(err.contains("command"), "{err}");
        assert!(err.contains("Bash"), "{err}");
        assert!(err.contains("--allowed-tools"), "{err}");
    }

    #[test]
    fn provider_validate_rejects_mutually_exclusive_claude_tool_lists() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
command = "claude"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
allowed_tools = ["Bash"]
disallowed_tools = ["Task"]
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(f.path()).unwrap_err();

        assert!(err.contains("providers.toml provider claude"), "{err}");
        assert!(err.contains("mutually exclusive"), "{err}");
        assert!(
            err.contains("tool_restrictions.claude.allowed_tools"),
            "{err}"
        );
        assert!(
            err.contains("tool_restrictions.claude.disallowed_tools"),
            "{err}"
        );
    }

    #[test]
    fn provider_validate_rejects_inactive_restriction_branch() {
        let mut claude_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            claude_file,
            r#"
[claude]
command = "claude"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.codex]
disabled_features = ["web_search"]
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(claude_file.path()).unwrap_err();

        assert!(err.contains("providers.toml provider claude"), "{err}");
        assert!(
            err.contains("tool_restrictions.codex must be empty"),
            "{err}"
        );

        let mut codex_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            codex_file,
            r#"
[codex]
command = "codex"

[codex.tool_restrictions]
kind = "codex"

[codex.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(codex_file.path()).unwrap_err();

        assert!(err.contains("providers.toml provider codex"), "{err}");
        assert!(
            err.contains("tool_restrictions.claude must be empty"),
            "{err}"
        );
    }

    #[test]
    fn provider_validate_rejects_non_allowlisted_codex_config_key() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[codex]
command = "codex"
args = ["exec"]
prompt_mode = "arg"

[codex.tool_restrictions]
kind = "codex"

[codex.tool_restrictions.codex]
config_pairs = ["model_reasoning_effort=high"]
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(f.path()).unwrap_err();

        assert!(err.contains("providers.toml provider codex"), "{err}");
        assert!(
            err.contains("tool_restrictions.codex.config_pairs"),
            "{err}"
        );
        assert!(err.contains("model_reasoning_effort"), "{err}");
        assert!(err.contains("no allowlisted Codex config pair"), "{err}");
    }

    #[test]
    fn provider_validate_rejects_duplicate_codex_policy_flags() {
        let mut config_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            config_file,
            r#"
[codex]
command = "codex -c sandbox=workspace-write"

[codex.tool_restrictions]
kind = "codex"

[codex.tool_restrictions.codex]
config_pairs = ["sandbox=read-only"]
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(config_file.path()).unwrap_err();

        assert!(err.contains("providers.toml provider codex"), "{err}");
        assert!(
            err.contains("tool_restrictions.codex.config_pairs"),
            "{err}"
        );
        assert!(err.contains("sandbox"), "{err}");
        assert!(err.contains("command"), "{err}");
        assert!(err.contains("-c"), "{err}");

        let mut feature_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            feature_file,
            r#"
[codex]
command = "codex"
args = ["--disable", "web_search"]

[codex.tool_restrictions]
kind = "codex"

[codex.tool_restrictions.codex]
disabled_features = ["web_search"]
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(feature_file.path()).unwrap_err();

        assert!(err.contains("providers.toml provider codex"), "{err}");
        assert!(
            err.contains("tool_restrictions.codex.disabled_features"),
            "{err}"
        );
        assert!(err.contains("web_search"), "{err}");
        assert!(err.contains("args"), "{err}");
        assert!(err.contains("--disable"), "{err}");
    }

    fn load_inline_providers(text: &str) -> Result<ProvidersConfig, LoadError> {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{text}").unwrap();
        ProvidersConfig::load(file.path())
    }

    #[test]
    fn providers_toml_defaults_invocation_mode_to_headless() {
        let cfg = load_inline_providers(
            r#"
[claude]
command = "claude"
"#,
        )
        .unwrap();

        let entry = cfg.get("claude").unwrap();
        assert_eq!(entry.invocation_mode, InvocationMode::Headless);
        let (runtime, _) = cfg.runtime_provider("claude").unwrap();
        assert_eq!(runtime.invocation_mode, InvocationMode::Headless);
    }

    #[test]
    fn providers_toml_parses_explicit_proxy_invocation_mode() {
        let cfg = load_inline_providers(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
        )
        .unwrap();

        assert_eq!(
            cfg.get("claude").unwrap().invocation_mode,
            InvocationMode::Proxy
        );
    }

    #[test]
    fn providers_toml_rejects_unknown_invocation_mode_with_actionable_error() {
        let err = load_inline_providers(
            r#"
[claude]
command = "claude"
invocation_mode = "bogus"
"#,
        )
        .unwrap_err();

        match err {
            LoadError::InvocationMode { provider, value } => {
                assert_eq!(provider, "claude");
                assert_eq!(value, "bogus");
            }
            other => panic!("expected LoadError::InvocationMode, got {other:?}"),
        }
    }

    #[test]
    fn proxy_claude_rejects_tools_mcp_filter_in_args() {
        let err = load_inline_providers(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
args = ["--tools", "mcp__age104p2__Task"]
"#,
        )
        .unwrap_err();

        assert!(err.contains("proxy-mode Claude"), "{err}");
        assert!(err.contains("--tools mcp__"), "{err}");
    }

    #[test]
    fn proxy_claude_rejects_tools_mcp_filter_in_interactive_args() {
        let err = load_inline_providers(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
interactive_args = ["--tools", "mcp__age104p2__Task"]
"#,
        )
        .unwrap_err();

        assert!(err.contains("proxy-mode Claude"), "{err}");
        assert!(err.contains("--tools mcp__"), "{err}");
    }

    #[test]
    fn proxy_claude_rejects_tools_mcp_filter_in_command() {
        let err = load_inline_providers(
            r#"
[claude]
command = "env -u CLAUDECODE claude --tools mcp__age104p2__Task"
invocation_mode = "proxy"
"#,
        )
        .unwrap_err();

        assert!(err.contains("proxy-mode Claude"), "{err}");
        assert!(err.contains("--tools mcp__"), "{err}");
    }

    #[test]
    fn proxy_claude_accepts_allowed_tools_mcp_filter() {
        let cfg = load_inline_providers(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
args = ["--allowedTools", "mcp__age104p2__Task"]
"#,
        )
        .unwrap();

        assert_eq!(
            cfg.get("claude").unwrap().invocation_mode,
            InvocationMode::Proxy
        );
    }

    #[test]
    fn proxy_claude_accepts_allowed_tools_kebab_mcp_filter() {
        let cfg = load_inline_providers(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
args = ["--allowed-tools", "mcp__age104p2__Task"]
"#,
        )
        .unwrap();

        assert_eq!(
            cfg.get("claude").unwrap().invocation_mode,
            InvocationMode::Proxy
        );
    }

    #[test]
    fn proxy_claude_accepts_no_tool_filter() {
        let cfg = load_inline_providers(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
        )
        .unwrap();

        assert_eq!(
            cfg.get("claude").unwrap().invocation_mode,
            InvocationMode::Proxy
        );
    }

    #[test]
    fn effective_provider_carries_invocation_mode() {
        let cfg = load_inline_providers(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
        )
        .unwrap();
        let model_provider = ProviderConfig::model_provider("claude", vec!["--model".into()]);

        let (provider, _) = cfg.effective_provider(&model_provider).unwrap();

        assert_eq!(provider.invocation_mode, InvocationMode::Proxy);
    }

    #[test]
    fn runtime_provider_carries_invocation_mode() {
        let cfg = load_inline_providers(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
        )
        .unwrap();

        let (provider, _) = cfg.runtime_provider("claude").unwrap();

        assert_eq!(provider.invocation_mode, InvocationMode::Proxy);
    }

    #[test]
    fn providers_toml_prefixed_command_proxy_mode_preserves_provider_family_and_name() {
        let cfg = load_inline_providers(
            r#"
[claude]
command = "env -u CLAUDECODE claude"
invocation_mode = "proxy"
args = ["--allowedTools", "mcp__age104p2__Task"]
"#,
        )
        .unwrap();

        let (provider, _) = cfg.runtime_provider("claude").unwrap();

        assert_eq!(provider.name, "claude");
        assert_eq!(
            derive_provider_name(&provider.command, &provider.args),
            "claude"
        );
        assert_eq!(provider.invocation_mode, InvocationMode::Proxy);
    }

    #[test]
    fn parse_providers_toml_returns_raw_struct_for_valid_text() {
        let raw = parse_providers_toml(
            r#"
[claude]
command = "claude"
"#,
        )
        .unwrap();

        assert!(raw.contains_key("claude"));
    }

    #[test]
    fn parse_providers_toml_returns_err_for_malformed_text() {
        let err = parse_providers_toml("not = [").unwrap_err();

        match err {
            LoadError::Toml(message) => assert!(!message.is_empty()),
            other => panic!("expected LoadError::Toml, got {other:?}"),
        }
    }

    #[test]
    fn apply_defaults_to_raw_providers_sets_headless_for_absent_mode() {
        let mut raw = RawProvidersToml::new();
        raw.insert(
            "claude".to_string(),
            RawEntry {
                quota_script: None,
                auth_refresh_command: None,
                command: Some("claude".to_string()),
                args: vec![],
                interactive_args: None,
                prompt_mode: None,
                resume: None,
                session_capture: None,
                resume_acceptance: None,
                session_storage: None,
                system_prompt_override: None,
                tool_restrictions: None,
                invocation_mode: None,
            },
        );

        let defaulted = apply_defaults_to_raw_providers(raw);

        assert_eq!(
            defaulted.get("claude").unwrap().invocation_mode.as_deref(),
            Some("headless")
        );
    }

    #[test]
    fn apply_defaults_to_raw_providers_preserves_explicit_mode() {
        let raw = parse_providers_toml(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
        )
        .unwrap();

        let defaulted = apply_defaults_to_raw_providers(raw);

        assert_eq!(
            defaulted.get("claude").unwrap().invocation_mode.as_deref(),
            Some("proxy")
        );
    }

    #[test]
    fn validate_providers_config_passes_for_valid_input() {
        let raw = apply_defaults_to_raw_providers(
            parse_providers_toml(
                r#"
[claude]
command = "claude"
invocation_mode = "proxy"
args = ["--allowedTools", "mcp__age104p2__Task"]
"#,
            )
            .unwrap(),
        );

        assert_eq!(validate_providers_config(&raw), Ok(()));
    }

    #[test]
    fn validate_providers_config_rejects_bad_invocation_mode() {
        let raw = parse_providers_toml(
            r#"
[claude]
command = "claude"
invocation_mode = "bogus"
"#,
        )
        .unwrap();

        let err = validate_providers_config(&raw).unwrap_err();

        match err {
            LoadError::InvocationMode { provider, value } => {
                assert_eq!(provider, "claude");
                assert_eq!(value, "bogus");
            }
            other => panic!("expected LoadError::InvocationMode, got {other:?}"),
        }
    }
}
