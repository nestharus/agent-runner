use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub prompt_mode: PromptMode,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    Stdin,
    Arg,
}

// Raw TOML structures for deserialization

#[derive(Deserialize)]
struct RawModelToml {
    command: Option<String>,
    args: Option<Vec<String>>,
    prompt_mode: Option<String>,
    providers: Option<Vec<RawProvider>>,
}

#[derive(Deserialize)]
struct RawProvider {
    command: String,
    args: Option<Vec<String>>,
}

fn parse_prompt_mode(s: &str) -> PromptMode {
    match s {
        "arg" => PromptMode::Arg,
        _ => PromptMode::Stdin,
    }
}

impl ModelConfig {
    pub fn to_toml(&self) -> String {
        let mode_str = match self.prompt_mode {
            PromptMode::Stdin => "stdin",
            PromptMode::Arg => "arg",
        };

        if self.providers.len() == 1 {
            let p = &self.providers[0];
            let args_toml: Vec<String> = p.args.iter().map(|a| format!("\"{}\"", a)).collect();
            format!(
                "command = \"{}\"\nargs = [{}]\nprompt_mode = \"{}\"\n",
                p.command,
                args_toml.join(", "),
                mode_str
            )
        } else {
            let mut out = format!("prompt_mode = \"{}\"\n", mode_str);
            for p in &self.providers {
                let args_toml: Vec<String> = p.args.iter().map(|a| format!("\"{}\"", a)).collect();
                out.push_str(&format!(
                    "\n[[providers]]\ncommand = \"{}\"\nargs = [{}]\n",
                    p.command,
                    args_toml.join(", ")
                ));
            }
            out
        }
    }

    pub fn from_toml(name: &str, content: &str) -> Result<Self, String> {
        let raw: RawModelToml =
            toml::from_str(content).map_err(|e| format!("TOML parse error for {name}: {e}"))?;

        let prompt_mode = parse_prompt_mode(raw.prompt_mode.as_deref().unwrap_or("stdin"));

        let providers = if let Some(providers) = raw.providers {
            // Multi-provider format: [[providers]]
            providers
                .into_iter()
                .map(|p| ProviderConfig {
                    command: p.command,
                    args: p.args.unwrap_or_default(),
                })
                .collect()
        } else if let Some(command) = raw.command {
            // Single-provider format: command + args at top level
            vec![ProviderConfig {
                command,
                args: raw.args.unwrap_or_default(),
            }]
        } else {
            return Err(format!(
                "Model {name}: must have either 'command' or '[[providers]]'"
            ));
        };

        if providers.is_empty() {
            return Err(format!("Model {name}: no providers defined"));
        }

        Ok(ModelConfig {
            name: name.to_string(),
            prompt_mode,
            providers,
        })
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
        assert_eq!(config.providers[0].command, "codex");
        assert_eq!(config.providers[1].command, "codex2");
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
        let result = ModelConfig::from_toml("test", toml);
        assert!(result.is_err());
    }

    #[test]
    fn roundtrip_single_provider() {
        let original = r#"
command = "codex"
args = ["exec", "-m", "gpt-5.3"]
prompt_mode = "arg"
"#;
        let config1 = ModelConfig::from_toml("test", original).unwrap();
        let serialized = config1.to_toml();
        let config2 = ModelConfig::from_toml("test", &serialized).unwrap();
        assert_eq!(config1.providers.len(), config2.providers.len());
        assert_eq!(config1.providers[0].command, config2.providers[0].command);
        assert_eq!(config1.providers[0].args, config2.providers[0].args);
        assert_eq!(config1.prompt_mode, config2.prompt_mode);
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
        let config1 = ModelConfig::from_toml("test", original).unwrap();
        let serialized = config1.to_toml();
        let config2 = ModelConfig::from_toml("test", &serialized).unwrap();
        assert_eq!(config1.providers.len(), config2.providers.len());
        assert_eq!(config1.providers[0].command, config2.providers[0].command);
        assert_eq!(config1.providers[1].command, config2.providers[1].command);
        assert_eq!(config1.prompt_mode, config2.prompt_mode);
    }
}
