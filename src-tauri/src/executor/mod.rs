use crate::config::{ModelConfig, PromptMode, ProviderConfig};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const LARGE_PROMPT_THRESHOLD: usize = 100 * 1024; // 100KB

#[allow(dead_code)]
pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub provider_index: usize,
}

pub fn execute(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
) -> Result<ExecutionResult, String> {
    let provider = model.providers.get(provider_index).ok_or_else(|| {
        format!(
            "Provider index {} out of range for model {}",
            provider_index, model.name
        )
    })?;

    let (result, temp_file) = execute_provider(provider, model.prompt_mode, prompt, working_dir)?;
    // Clean up temp file if one was created
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

struct RawResult {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

fn execute_provider(
    provider: &ProviderConfig,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
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

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let mut temp_path = None;

    match prompt_mode {
        PromptMode::Arg => {
            if prompt.len() > LARGE_PROMPT_THRESHOLD {
                // Write to temp file, pass reference
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
        // stdin is dropped here, closing the pipe
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for process: {e}"))?;

    let result = RawResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    };

    Ok((result, temp_path))
}

fn shell_split(s: &str) -> Vec<String> {
    // Basic shell-like splitting that handles double-quoted tokens.
    // e.g. `env -u CLAUDECODE "my cmd"` → ["env", "-u", "CLAUDECODE", "my cmd"]
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

/// Extract the provider name from a command string.
///
/// The provider is the last shell-split token (quote-aware) so commands like
/// `env -u CLAUDECODE claude` display as "claude" and commands like
/// `env -u FOO "my provider"` display as "my provider".
pub fn provider_name(command: &str) -> String {
    shell_split(command)
        .last()
        .cloned()
        .unwrap_or_else(|| command.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(unix)]
    #[test]
    fn execute_echo_arg_mode() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig {
                command: "echo".to_string(),
                args: vec![],
            }],
        };
        let result = execute(&model, 0, "hello world", None).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello world");
    }

    #[cfg(unix)]
    #[test]
    fn execute_cat_stdin_mode() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig {
                command: "cat".to_string(),
                args: vec![],
            }],
        };
        let result = execute(&model, 0, "piped input", None).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "piped input");
    }
}
