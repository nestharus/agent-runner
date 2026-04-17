use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// One entry in `providers.toml`, keyed by the provider name.
#[derive(Debug, Clone)]
pub struct ProviderEntry {
    /// Shell command that prints JSON `{"used_percent": <0..1>, "resets_at": "..."}`
    /// on stdout. Empty if the provider has no quota check wired up.
    pub quota_script: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProvidersConfig {
    pub entries: HashMap<String, ProviderEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(default)]
    quota_script: Option<String>,
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
    fn missing_file_is_empty_config() {
        let cfg = ProvidersConfig::load(Path::new("/nonexistent/path/providers.toml")).unwrap();
        assert!(cfg.entries.is_empty());
    }
}
