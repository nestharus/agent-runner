use crate::state::{CliMapping, DiscoveredModel, ModelParameter, ParamType};
use std::process::Command;

/// Result of a model discovery attempt for a single CLI.
#[derive(Debug)]
pub struct DiscoveryResult {
    pub cli_name: String,
    pub cli_version: String,
    pub models: Vec<DiscoveredModel>,
    pub parameters: Vec<(String, ModelParameter)>, // (model_name, param)
}

/// Known CLI discovery strategies. Each entry maps a CLI name to a function
/// that tries to extract model names from that CLI's output.
struct CliDiscoveryStrategy {
    name: &'static str,
    commands: &'static [&'static [&'static str]],
}

const STRATEGIES: &[CliDiscoveryStrategy] = &[
    CliDiscoveryStrategy {
        name: "claude",
        commands: &[&["models", "list"], &["--help"]],
    },
    CliDiscoveryStrategy {
        name: "codex",
        commands: &[&["models"], &["--help"]],
    },
    CliDiscoveryStrategy {
        name: "gemini",
        commands: &[&["models", "list"], &["--help"]],
    },
    CliDiscoveryStrategy {
        name: "opencode",
        commands: &[&["models"], &["--help"]],
    },
];

/// Run model discovery for a specific CLI tool.
///
/// Tries known discovery commands in order, returning the first successful parse.
/// Returns an error only if the CLI is not found at all; empty results are OK.
pub fn discover_models(cli_name: &str) -> Result<DiscoveryResult, String> {
    let cli_version = get_cli_version(cli_name)?;

    let strategy = STRATEGIES.iter().find(|s| s.name == cli_name);

    let commands: &[&[&str]] = match strategy {
        Some(s) => s.commands,
        // Unknown CLI: try generic approaches
        None => &[&["models", "list"], &["models"], &["--help"]],
    };

    let now = chrono::Utc::now().to_rfc3339();

    for cmd_args in commands {
        match run_cli_command(cli_name, cmd_args) {
            Ok(output) => {
                let model_names = parse_model_names(cli_name, &output);
                if !model_names.is_empty() {
                    let models: Vec<DiscoveredModel> = model_names
                        .iter()
                        .map(|name| DiscoveredModel {
                            canonical_name: name.clone(),
                            provider: cli_name.to_string(),
                            discovered_at: now.clone(),
                            cli_version: cli_version.clone(),
                        })
                        .collect();

                    let parameters = build_default_parameters(cli_name, &model_names);

                    return Ok(DiscoveryResult {
                        cli_name: cli_name.to_string(),
                        cli_version,
                        models,
                        parameters,
                    });
                }
            }
            Err(_) => continue,
        }
    }

    // No discovery command succeeded, return empty result (not an error)
    Ok(DiscoveryResult {
        cli_name: cli_name.to_string(),
        cli_version,
        models: vec![],
        parameters: vec![],
    })
}

/// Get the version string from a CLI tool.
fn get_cli_version(cli_name: &str) -> Result<String, String> {
    let output = Command::new(cli_name)
        .arg("--version")
        .output()
        .map_err(|e| format!("CLI '{}' not found or not executable: {}", cli_name, e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        // Some CLIs print version to stderr or use a different flag
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !stderr.is_empty() {
            Ok(stderr)
        } else {
            Ok("unknown".to_string())
        }
    }
}

/// Run a CLI command and capture stdout.
fn run_cli_command(cli_name: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(cli_name)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run {} {:?}: {}", cli_name, args, e))?;

    // Accept both success and some failure codes (help often returns non-zero)
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Use whichever has more content
    if stdout.len() >= stderr.len() && !stdout.is_empty() {
        Ok(stdout)
    } else if !stderr.is_empty() {
        Ok(stderr)
    } else if output.status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "{} {:?} failed with exit code {:?}",
            cli_name,
            args,
            output.status.code()
        ))
    }
}

/// Parse model names from CLI output.
///
/// Uses heuristics to extract model identifiers from various output formats:
/// - One model name per line (most common for `models list`)
/// - Model names in help text (fallback)
fn parse_model_names(cli_name: &str, output: &str) -> Vec<String> {
    let mut models = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Skip header/decorative lines
        if trimmed.starts_with('-') || trimmed.starts_with('=') || trimmed.starts_with('#') {
            continue;
        }

        // Skip common non-model lines
        let lower = trimmed.to_lowercase();
        if lower.starts_with("available")
            || lower.starts_with("usage")
            || lower.starts_with("options")
            || lower.starts_with("commands")
            || lower.starts_with("flags")
            || lower.contains("--help")
        {
            continue;
        }

        // Try to extract a model-like identifier from the line
        if let Some(model_name) = extract_model_name(cli_name, trimmed) {
            if is_valid_model_name(&model_name) {
                models.push(model_name);
            }
        }
    }

    models.sort();
    models.dedup();
    models
}

/// Try to extract a model name from a single line of output.
fn extract_model_name(cli_name: &str, line: &str) -> Option<String> {
    // If the line looks like "model-name  description text", take the first token
    let first_token = line.split_whitespace().next()?;

    // Strip leading bullet points, numbers, asterisks
    let cleaned = first_token
        .trim_start_matches(|c: char| c == '*' || c == '>' || c == '|')
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
        .trim();

    if cleaned.is_empty() {
        return None;
    }

    // Model names typically contain: letters, digits, hyphens, dots, underscores, colons, slashes
    // They often follow patterns like:
    //   claude-opus-4, gpt-5.3, gemini-2.0-flash, models/gemini-pro
    let candidate = cleaned.to_string();

    // Provider-specific normalization
    match cli_name {
        "gemini" => {
            // Gemini models may be listed as "models/gemini-pro" - keep as-is
            Some(candidate)
        }
        _ => Some(candidate),
    }
}

/// Check if a string looks like a valid model name.
fn is_valid_model_name(name: &str) -> bool {
    if name.len() < 2 || name.len() > 100 {
        return false;
    }

    // Must contain at least one letter
    if !name.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }

    // Must not be a common non-model word
    let lower = name.to_lowercase();
    let stop_words = [
        "name",
        "id",
        "type",
        "model",
        "models",
        "list",
        "help",
        "version",
        "the",
        "and",
        "for",
        "with",
        "from",
        "this",
        "that",
        "description",
        "status",
        "created",
        "updated",
        "default",
        "none",
        "true",
        "false",
    ];
    if stop_words.contains(&lower.as_str()) {
        return false;
    }

    // Should only contain valid model-name characters
    name.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ':' || c == '/'
    })
}

/// Build default parameter definitions for known CLIs.
/// These represent common parameters that most models of a given CLI support.
fn build_default_parameters(
    cli_name: &str,
    model_names: &[String],
) -> Vec<(String, ModelParameter)> {
    let mut params = Vec::new();

    let common_params = match cli_name {
        "claude" => vec![ModelParameter {
            name: "max_tokens".to_string(),
            display_name: "Max Tokens".to_string(),
            param_type: ParamType::Number {
                min: Some(1.0),
                max: Some(200000.0),
            },
            description: "Maximum number of tokens to generate".to_string(),
            cli_mapping: CliMapping {
                flag: "--max-tokens".to_string(),
                value_template: "{value}".to_string(),
            },
        }],
        "codex" => vec![ModelParameter {
            name: "model".to_string(),
            display_name: "Model".to_string(),
            param_type: ParamType::Enum {
                options: model_names.to_vec(),
            },
            description: "Model to use for generation".to_string(),
            cli_mapping: CliMapping {
                flag: "-m".to_string(),
                value_template: "{value}".to_string(),
            },
        }],
        "gemini" => vec![ModelParameter {
            name: "temperature".to_string(),
            display_name: "Temperature".to_string(),
            param_type: ParamType::Number {
                min: Some(0.0),
                max: Some(2.0),
            },
            description: "Controls randomness of output".to_string(),
            cli_mapping: CliMapping {
                flag: "--temperature".to_string(),
                value_template: "{value}".to_string(),
            },
        }],
        _ => vec![],
    };

    // Apply common params to all discovered models for this CLI
    for model_name in model_names {
        for param in &common_params {
            params.push((model_name.clone(), param.clone()));
        }
    }

    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_names_simple_list() {
        let output = "claude-opus-4\nclaude-sonnet-4\nclaude-haiku-3.5\n";
        let models = parse_model_names("claude", output);
        assert_eq!(
            models,
            vec!["claude-haiku-3.5", "claude-opus-4", "claude-sonnet-4"]
        );
    }

    #[test]
    fn parse_model_names_with_descriptions() {
        let output = "\
gpt-5.3           Latest GPT model
gpt-5.3-mini      Smaller, faster variant
o3                Reasoning model
";
        let models = parse_model_names("codex", output);
        assert_eq!(models, vec!["gpt-5.3", "gpt-5.3-mini", "o3"]);
    }

    #[test]
    fn parse_model_names_skips_headers() {
        let output = "\
Available Models:
-----------------
claude-opus-4
claude-sonnet-4
";
        let models = parse_model_names("claude", output);
        assert_eq!(models, vec!["claude-opus-4", "claude-sonnet-4"]);
    }

    #[test]
    fn parse_model_names_empty_input() {
        let models = parse_model_names("claude", "");
        assert!(models.is_empty());
    }

    #[test]
    fn parse_model_names_gemini_prefixed() {
        let output = "models/gemini-2.0-flash\nmodels/gemini-pro\n";
        let models = parse_model_names("gemini", output);
        assert_eq!(models, vec!["models/gemini-2.0-flash", "models/gemini-pro"]);
    }

    #[test]
    fn is_valid_model_name_accepts_good_names() {
        assert!(is_valid_model_name("claude-opus-4"));
        assert!(is_valid_model_name("gpt-5.3"));
        assert!(is_valid_model_name("models/gemini-pro"));
        assert!(is_valid_model_name("o3"));
    }

    #[test]
    fn is_valid_model_name_rejects_bad_names() {
        assert!(!is_valid_model_name(""));
        assert!(!is_valid_model_name("a")); // too short
        assert!(!is_valid_model_name("help"));
        assert!(!is_valid_model_name("the"));
        assert!(!is_valid_model_name("123")); // no letters
        assert!(!is_valid_model_name("model with spaces"));
    }

    #[test]
    fn build_default_parameters_claude() {
        let models = vec!["claude-opus-4".to_string()];
        let params = build_default_parameters("claude", &models);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "claude-opus-4");
        assert_eq!(params[0].1.name, "max_tokens");
    }

    #[test]
    fn build_default_parameters_unknown_cli() {
        let models = vec!["some-model".to_string()];
        let params = build_default_parameters("unknown-cli", &models);
        assert!(params.is_empty());
    }

    #[test]
    fn build_default_parameters_multiple_models() {
        let models = vec!["m1".to_string(), "m2".to_string()];
        let params = build_default_parameters("codex", &models);
        // Each model gets the same set of params
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].0, "m1");
        assert_eq!(params[1].0, "m2");
    }

    #[test]
    fn parse_deduplicates() {
        let output = "model-a\nmodel-b\nmodel-a\n";
        let models = parse_model_names("test", output);
        assert_eq!(models, vec!["model-a", "model-b"]);
    }
}
