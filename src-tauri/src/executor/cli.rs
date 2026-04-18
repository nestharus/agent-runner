use crate::config::{InputType, ModelConfig, PromptMode, ProviderConfig};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::ExecutionResult;

const LARGE_PROMPT_THRESHOLD: usize = 100 * 1024; // 100KB

pub fn execute(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    parent_invocation_env: Option<&str>,
) -> Result<ExecutionResult, String> {
    let provider = model.providers.get(provider_index).ok_or_else(|| {
        format!(
            "Provider index {} out of range for model {}",
            provider_index, model.name
        )
    })?;

    // Resolve inputs to flag args
    let input_args = resolve_input_flags(model, extra_inputs)?;

    let (result, temp_file) = execute_provider(
        provider,
        model.prompt_mode,
        prompt,
        working_dir,
        &input_args,
        parent_invocation_env,
    )?;
    if let Some(path) = temp_file {
        let _ = std::fs::remove_file(path);
    }

    Ok(ExecutionResult {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        provider_index,
    })
}

/// Map user-provided inputs to CLI flag arguments based on the model's input schema.
fn resolve_input_flags(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, String> {
    let mut args = Vec::new();

    for (key, values) in extra_inputs {
        // Find the matching input definition
        let input_def = model.inputs.iter().find(|i| i.name == *key);

        let flag = match input_def {
            Some(def) => {
                // Validate the value(s) against the schema
                validate_input_values(values, def)?;
                // Use declared flag, or fall back to --name
                def.flag.clone().unwrap_or_else(|| format!("--{}", key))
            }
            None => {
                // Unknown input — pass through as --name (user knows what they're doing)
                format!("--{}", key)
            }
        };

        // For array types or repeated values, emit the flag once per value
        for val in values {
            args.push(flag.clone());
            args.push(val.clone());
        }
    }

    // Apply defaults for inputs not provided by the user
    for input_def in &model.inputs {
        if input_def.default_input {
            continue; // Default input is the prompt, handled separately
        }
        if extra_inputs.contains_key(&input_def.name) {
            continue; // Already provided
        }
        if let Some(ref default) = input_def.default {
            if let Some(ref flag) = input_def.flag {
                let val = toml_value_to_string(default);
                args.push(flag.clone());
                args.push(val);
            }
        } else if input_def.required {
            return Err(format!("Required input '{}' not provided", input_def.name));
        }
    }

    Ok(args)
}

fn validate_input_values(
    values: &[String],
    input_def: &crate::config::InputDef,
) -> Result<(), String> {
    match &input_def.input_type {
        InputType::Enum { options } => {
            for val in values {
                if !options.contains(val) {
                    return Err(format!(
                        "Input '{}': '{}' is not a valid option. Valid: {:?}",
                        input_def.name, val, options
                    ));
                }
            }
        }
        InputType::Integer { min, max } => {
            for val in values {
                let n: i64 = val.parse().map_err(|_| {
                    format!(
                        "Input '{}': '{}' is not a valid integer",
                        input_def.name, val
                    )
                })?;
                if let Some(min_val) = min
                    && n < *min_val
                {
                    return Err(format!(
                        "Input '{}': {} is below minimum {}",
                        input_def.name, n, min_val
                    ));
                }
                if let Some(max_val) = max
                    && n > *max_val
                {
                    return Err(format!(
                        "Input '{}': {} exceeds maximum {}",
                        input_def.name, n, max_val
                    ));
                }
            }
        }
        InputType::Number { min, max } => {
            for val in values {
                let n: f64 = val.parse().map_err(|_| {
                    format!(
                        "Input '{}': '{}' is not a valid number",
                        input_def.name, val
                    )
                })?;
                if let Some(min_val) = min
                    && n < *min_val
                {
                    return Err(format!(
                        "Input '{}': {} is below minimum {}",
                        input_def.name, n, min_val
                    ));
                }
                if let Some(max_val) = max
                    && n > *max_val
                {
                    return Err(format!(
                        "Input '{}': {} exceeds maximum {}",
                        input_def.name, n, max_val
                    ));
                }
            }
        }
        InputType::Array {
            min_items,
            max_items,
            ..
        } => {
            if let Some(min) = min_items
                && values.len() < *min
            {
                return Err(format!(
                    "Input '{}': need at least {} items, got {}",
                    input_def.name,
                    min,
                    values.len()
                ));
            }
            if let Some(max) = max_items
                && values.len() > *max
            {
                return Err(format!(
                    "Input '{}': maximum {} items, got {}",
                    input_def.name,
                    max,
                    values.len()
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn toml_value_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}

struct RawResult {
    stdout: Vec<u8>,
    stderr: String,
    exit_code: i32,
}

fn execute_provider(
    provider: &ProviderConfig,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    input_args: &[String],
    parent_invocation_env: Option<&str>,
) -> Result<(RawResult, Option<PathBuf>), String> {
    let parts = shell_split(&provider.command);
    if parts.is_empty() {
        return Err("Empty command".to_string());
    }

    let mut cmd = Command::new(&parts[0]);
    for part in &parts[1..] {
        cmd.arg(part);
    }
    for arg in &provider.args {
        cmd.arg(arg);
    }

    // Append input flags
    for arg in input_args {
        cmd.arg(arg);
    }

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    if let Some(parent_invocation_env) = parent_invocation_env {
        cmd.env("OULIPOLY_PARENT_INVOCATION", parent_invocation_env);
    }

    let mut temp_path = None;

    match prompt_mode {
        PromptMode::Arg => {
            if prompt.len() > LARGE_PROMPT_THRESHOLD {
                let dir = working_dir.unwrap_or(Path::new("."));
                let filename = format!("_agent_prompt_{}.md", uuid::Uuid::new_v4());
                let path = dir.join(&filename);
                std::fs::write(&path, prompt)
                    .map_err(|e| format!("Failed to write temp prompt file: {e}"))?;
                cmd.arg(format!("Follow the instructions in {filename}"));
                temp_path = Some(path);
            } else {
                cmd.arg(prompt);
            }
            cmd.stdin(Stdio::null());
        }
        PromptMode::Stdin => {
            cmd.stdin(Stdio::piped());
        }
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}': {e}", provider.command))?;

    if prompt_mode == PromptMode::Stdin
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| format!("Failed to write to stdin: {e}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for process: {e}"))?;

    let result = RawResult {
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    };

    Ok((result, temp_path))
}

pub fn shell_split(s: &str) -> Vec<String> {
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

pub fn provider_name(command: &str) -> String {
    shell_split(command)
        .last()
        .cloned()
        .unwrap_or_else(|| command.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputDef, InputType, ProviderConfig};

    #[test]
    fn shell_split_simple() {
        assert_eq!(shell_split("echo hello"), vec!["echo", "hello"]);
    }

    #[test]
    fn shell_split_single() {
        assert_eq!(shell_split("codex"), vec!["codex"]);
    }

    #[test]
    fn shell_split_quoted_token() {
        assert_eq!(
            shell_split(r#"env -u FOO "my cmd""#),
            vec!["env", "-u", "FOO", "my cmd"]
        );
    }

    #[test]
    fn provider_name_simple() {
        assert_eq!(provider_name("claude"), "claude");
    }

    #[test]
    fn provider_name_with_prefix() {
        assert_eq!(provider_name("env -u CLAUDECODE claude"), "claude");
    }

    #[test]
    fn provider_name_quoted() {
        assert_eq!(provider_name(r#"env -u FOO "claude""#), "claude");
    }

    #[test]
    fn provider_name_with_spaces() {
        assert_eq!(provider_name(r#"env -u FOO "my provider""#), "my provider");
    }

    #[test]
    fn resolve_flags_with_schema() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![
                InputDef {
                    name: "prompt".to_string(),
                    input_type: InputType::String,
                    required: true,
                    default_input: true,
                    default: None,
                    description: None,
                    flag: None,
                },
                InputDef {
                    name: "size".to_string(),
                    input_type: InputType::Enum {
                        options: vec!["2048*2048".to_string(), "1024*1024".to_string()],
                    },
                    required: false,
                    default_input: false,
                    default: Some(toml::Value::String("2048*2048".to_string())),
                    description: None,
                    flag: Some("--size".to_string()),
                },
            ],
        };
        let mut inputs = HashMap::new();
        inputs.insert("size".to_string(), vec!["1024*1024".to_string()]);
        let flags = resolve_input_flags(&model, &inputs).unwrap();
        assert_eq!(flags, vec!["--size", "1024*1024"]);
    }

    #[test]
    fn resolve_flags_applies_defaults() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![InputDef {
                name: "format".to_string(),
                input_type: InputType::Enum {
                    options: vec!["jpeg".to_string(), "png".to_string()],
                },
                required: false,
                default_input: false,
                default: Some(toml::Value::String("jpeg".to_string())),
                description: None,
                flag: Some("--format".to_string()),
            }],
        };
        let flags = resolve_input_flags(&model, &HashMap::new()).unwrap();
        assert_eq!(flags, vec!["--format", "jpeg"]);
    }

    #[test]
    fn resolve_flags_rejects_invalid_enum() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![InputDef {
                name: "size".to_string(),
                input_type: InputType::Enum {
                    options: vec!["small".to_string(), "large".to_string()],
                },
                required: false,
                default_input: false,
                default: None,
                description: None,
                flag: Some("--size".to_string()),
            }],
        };
        let mut inputs = HashMap::new();
        inputs.insert("size".to_string(), vec!["huge".to_string()]);
        let result = resolve_input_flags(&model, &inputs);
        assert!(result.unwrap_err().contains("not a valid option"));
    }

    #[test]
    fn resolve_flags_rejects_missing_required() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![InputDef {
                name: "image".to_string(),
                input_type: InputType::String,
                required: true,
                default_input: false,
                default: None,
                description: None,
                flag: Some("--image".to_string()),
            }],
        };
        let result = resolve_input_flags(&model, &HashMap::new());
        assert!(result.unwrap_err().contains("Required input 'image'"));
    }

    #[test]
    fn resolve_flags_unknown_passthrough() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![],
        };
        let mut inputs = HashMap::new();
        inputs.insert("custom".to_string(), vec!["value".to_string()]);
        let flags = resolve_input_flags(&model, &inputs).unwrap();
        assert_eq!(flags, vec!["--custom", "value"]);
    }

    #[cfg(unix)]
    #[test]
    fn execute_echo_arg_mode() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::new("echo", vec![])],
            inputs: vec![],
        };
        let result = execute(&model, 0, "hello world", None, &HashMap::new(), None).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            String::from_utf8_lossy(&result.stdout).trim(),
            "hello world"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_cat_stdin_mode() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("cat", vec![])],
            inputs: vec![],
        };
        let result = execute(&model, 0, "piped input", None, &HashMap::new(), None).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, b"piped input");
    }

    #[cfg(unix)]
    #[test]
    fn execute_with_input_flags() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("echo", vec![])],
            inputs: vec![InputDef {
                name: "greeting".to_string(),
                input_type: InputType::String,
                required: false,
                default_input: false,
                default: None,
                description: None,
                flag: Some("--greet".to_string()),
            }],
        };
        let mut inputs = HashMap::new();
        inputs.insert("greeting".to_string(), vec!["hello".to_string()]);
        // echo ignores stdin, prints its args
        let result = execute(&model, 0, "", None, &inputs, None).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&result.stdout).trim(),
            "--greet hello"
        );
    }
}
