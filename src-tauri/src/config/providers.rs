use serde::Deserialize;
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
    /// Optional default model used when a UI-started provider session has no
    /// invocation history tying it to a model.
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProvidersConfig {
    pub entries: HashMap<String, ProviderEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(default)]
    quota_script: Option<String>,
    #[serde(default)]
    auth_refresh_command: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
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
                (
                    k,
                    ProviderEntry {
                        quota_script: v.quota_script,
                        auth_refresh_command: v.auth_refresh_command,
                        default_model: v.default_model,
                    },
                )
            })
            .collect();
        Ok(Self { entries })
    }

    pub fn get(&self, name: &str) -> Option<&ProviderEntry> {
        self.entries.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn default_model_providers_toml() -> &'static str {
        r#"
[claude]
quota_script = "anthropic-usage ~/.claude/.credentials.json"
default_model = "claude-opus"
"#
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
    fn missing_file_is_empty_config() {
        let cfg = ProvidersConfig::load(Path::new("/nonexistent/path/providers.toml")).unwrap();
        assert!(cfg.entries.is_empty());
    }

    // risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference / A8.
    #[test]
    fn parses_default_model() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "{}", default_model_providers_toml()).unwrap();

        let cfg = ProvidersConfig::load(f.path()).unwrap();

        assert_eq!(
            cfg.get("claude").unwrap().default_model.as_deref(),
            Some("claude-opus")
        );
    }
}
