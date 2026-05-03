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
    fn provider_loading_rejects_invalid_session_capture_before_runtime_use() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[claude]
command = "claude"

[claude.session_capture]
kind = "forced_flag_verified"
"#
        )
        .unwrap();

        let err = ProvidersConfig::load(f.path()).unwrap_err();
        assert!(err.contains("providers.toml provider claude"));
        assert!(err.contains("requires `flag`"));
    }
}
