use super::model::{
    PromptMode, ProviderConfig, ResumeAcceptanceRules, ResumeStrategy, SessionCapture,
    SessionStorage,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProvidersConfig {
    pub entries: HashMap<String, ProviderEntry>,
}

#[derive(Deserialize, Serialize)]
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
}

impl ProvidersConfig {
    /// Parse a providers.toml, returning an empty config if the file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let raw: HashMap<String, RawEntry> = toml::from_str(&content)
            .map_err(|e| format!("TOML parse error in {}: {e}", path.display()))?;
        let entries = raw
            .into_iter()
            .map(|(k, v)| {
                let entry = ProviderEntry {
                    quota_script: v.quota_script,
                    auth_refresh_command: v.auth_refresh_command,
                    command: v.command,
                    args: v.args,
                    interactive_args: v.interactive_args,
                    prompt_mode: parse_prompt_mode(v.prompt_mode.as_deref().unwrap_or("stdin")),
                    resume: v.resume,
                    session_capture: v.session_capture,
                    resume_acceptance: v.resume_acceptance,
                    session_storage: v.session_storage.map(SessionStorage::expand_tilde),
                };
                entry.validate(&k).map(|()| (k, entry))
            })
            .collect::<Result<HashMap<_, _>, String>>()?;
        Ok(Self { entries })
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
        let cwd_script = match kind {
            "claude_code" => storage
                .get("projects_dir")
                .and_then(toml::Value::as_str)
                .map(|projects_dir| format!("claude-code-cwd {}", shell_word(projects_dir))),
            "codex" => storage
                .get("sessions_dir")
                .and_then(toml::Value::as_str)
                .map(|sessions_dir| format!("codex-cwd {}", shell_word(sessions_dir))),
            _ => None,
        };
        let Some(cwd_script) = cwd_script else {
            continue;
        };
        storage.insert(
            "kind".to_string(),
            toml::Value::String("script".to_string()),
        );
        storage.insert("cwd_script".to_string(), toml::Value::String(cwd_script));
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
            },
            self.prompt_mode,
        ))
    }
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
    use std::io::Write;

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
kind = "codex"
sessions_dir = "/tmp/codex-sessions"
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
        assert!(migrated);
        let migrated_content = std::fs::read_to_string(f.path()).unwrap();
        assert!(migrated_content.contains("kind = \"script\""));
        assert!(
            migrated_content.contains("cwd_script = \"claude-code-cwd /tmp/provider/projects\"")
        );
        assert!(!migrated_content.contains("projects_dir"));
    }
}
