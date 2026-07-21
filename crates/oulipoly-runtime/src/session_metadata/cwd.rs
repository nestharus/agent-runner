//! ## Declared roles
//! accessor, formatter, mapper, orchestration, parser, validator
//!
//! Workspace-root resolution from the configured session storage cwd script.
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/session_metadata/cwd.rs
//!     role: intrinsic-surface
//!     Domain: session storage cwd script resolution
//!     Owns:
//!       - cwd script command construction and execution
//!       - cwd script stdout parsing and validation
//! ```

use oulipoly_config::SessionStorage;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

#[derive(Debug, Deserialize)]
struct CwdScriptResponse {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    found: bool,
    #[serde(default)]
    error: Option<String>,
}

pub(super) fn resolve_workspace_root(
    session_storage: Option<&SessionStorage>,
    session_id: &str,
) -> Result<PathBuf, String> {
    let session_storage = require_session_storage(session_storage)?;
    let script = cwd_script(session_storage);
    let stdout = run_cwd_script(&script, session_id)?;
    let line = single_cwd_stdout_line(&stdout)?;
    let response = parse_cwd_response(line)?;
    validate_cwd_response(&response)?;
    cwd_path_from_response(&response)
}

fn require_session_storage(
    session_storage: Option<&SessionStorage>,
) -> Result<&SessionStorage, String> {
    session_storage.ok_or_else(no_workspace_root_reason)
}

fn no_workspace_root_reason() -> String {
    "no_workspace_root_for_other_storage: storage_type other".to_string()
}

fn cwd_script(session_storage: &SessionStorage) -> String {
    session_storage.cwd_script()
}

fn single_cwd_stdout_line(stdout: &str) -> Result<&str, String> {
    let trimmed = trim_stdout_lines(stdout);
    let lines = nonempty_lines(trimmed);
    validate_single_cwd_line_count(&lines)
}

fn trim_stdout_lines(stdout: &str) -> Vec<&str> {
    stdout.lines().map(str::trim).collect()
}

fn nonempty_lines(lines: Vec<&str>) -> Vec<&str> {
    lines.into_iter().filter(|line| !line.is_empty()).collect()
}

fn validate_single_cwd_line_count<'a>(lines: &[&'a str]) -> Result<&'a str, String> {
    match lines {
        [] => Err(cwd_empty_stdout_reason()),
        [line] => Ok(line),
        _ => Err(cwd_multiline_stdout_reason()),
    }
}

fn cwd_empty_stdout_reason() -> String {
    "cwd_script_empty_stdout".to_string()
}

fn cwd_multiline_stdout_reason() -> String {
    "cwd_script_stdout_not_single_line".to_string()
}

fn parse_cwd_response(line: &str) -> Result<CwdScriptResponse, String> {
    serde_json::from_str(line).map_err(|error| malformed_json_reason(error.to_string()))
}

fn malformed_json_reason(error: String) -> String {
    format!("cwd_script_malformed_json: {error}")
}

fn validate_cwd_response(response: &CwdScriptResponse) -> Result<(), String> {
    reject_response_error(response)?;
    reject_missing_response(response)
}

fn reject_response_error(response: &CwdScriptResponse) -> Result<(), String> {
    if let Some(error) = response.error.as_deref()
        && !error.trim().is_empty()
    {
        return Err(cwd_script_error_reason(error));
    }
    Ok(())
}

fn cwd_script_error_reason(error: &str) -> String {
    format!("cwd_script_error: {error}")
}

fn reject_missing_response(response: &CwdScriptResponse) -> Result<(), String> {
    if response.found {
        Ok(())
    } else {
        Err(cwd_not_found_reason())
    }
}

fn cwd_not_found_reason() -> String {
    "cwd_script_not_found".to_string()
}

fn cwd_path_from_response(response: &CwdScriptResponse) -> Result<PathBuf, String> {
    let cwd = response_cwd(response)?;
    let path = cwd_path(cwd);
    validate_absolute_cwd(path)
}

fn response_cwd(response: &CwdScriptResponse) -> Result<&str, String> {
    response.cwd.as_deref().ok_or_else(cwd_missing_cwd_reason)
}

fn cwd_missing_cwd_reason() -> String {
    "cwd_script_missing_cwd".to_string()
}

fn cwd_path(cwd: &str) -> PathBuf {
    PathBuf::from(cwd)
}

fn validate_absolute_cwd(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(cwd_not_absolute_reason(&path))
    }
}

fn cwd_not_absolute_reason(path: &std::path::Path) -> String {
    format!("cwd_script_cwd_not_absolute: {}", path.display())
}

fn run_cwd_script(script: &str, session_id: &str) -> Result<String, String> {
    run_session_id_script(script, session_id, "cwd_script")
}

pub(super) fn run_session_id_script(
    script: &str,
    session_id: &str,
    script_kind: &str,
) -> Result<String, String> {
    let mut command = session_id_command(script, session_id);
    let output = process_output(&mut command)
        .map_err(|error| script_spawn_failed_reason(script_kind, error.to_string()))?;
    validate_script_status(&output, script_kind)?;
    Ok(decode_stdout(&output))
}

fn session_id_command(script: &str, session_id: &str) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(session_id_command_script(script))
        .arg("oulipoly-session-script")
        .arg(session_id)
        .env("SESSION_ID", session_id)
        .stdin(Stdio::null());
    command
}

fn session_id_command_script(script: &str) -> String {
    format!("{script} \"$1\"")
}

fn process_output(command: &mut Command) -> std::io::Result<Output> {
    command.output()
}

fn validate_script_status(output: &Output, script_kind: &str) -> Result<(), String> {
    if output.status.success() {
        Ok(())
    } else {
        Err(script_exit_reason(script_kind, output))
    }
}

fn script_spawn_failed_reason(script_kind: &str, message: String) -> String {
    format!("{script_kind}_spawn_failed: {message}")
}

fn script_exit_reason(script_kind: &str, output: &Output) -> String {
    format!(
        "{script_kind}_exit_{}: {}",
        script_exit_code(output),
        decode_stderr(output).trim()
    )
}

fn script_exit_code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn decode_stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn decode_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
