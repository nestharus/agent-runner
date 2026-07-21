//! ## Declared roles
//!
//! `mapper`, `orchestration`, `parser`, `validator`
//!
//! Provider-private session-storage membership probing.

use oulipoly_config::SessionStorage;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOwnership {
    Owned,
    NotOwned,
    Indeterminate(String),
}

#[derive(Debug, Deserialize)]
struct OwnershipResponse {
    owned: Option<bool>,
    #[serde(default)]
    error: Option<String>,
}

pub fn resolve_session_ownership(
    session_storage: Option<&SessionStorage>,
    session_id: &str,
) -> SessionOwnership {
    let session_storage = match map_session_storage(session_storage) {
        Ok(session_storage) => session_storage,
        Err(ownership) => return ownership,
    };
    let stdout = match map_ownership_script_result(super::cwd::run_session_id_script(
        &session_storage.cwd_script(),
        session_id,
        "ownership_script",
    )) {
        Ok(stdout) => stdout,
        Err(ownership) => return ownership,
    };
    parse_session_ownership(&stdout)
}

fn map_session_storage(
    session_storage: Option<&SessionStorage>,
) -> Result<&SessionStorage, SessionOwnership> {
    match session_storage {
        Some(session_storage) => Ok(session_storage),
        None => Err(SessionOwnership::Indeterminate(
            "session_storage_missing".to_string(),
        )),
    }
}

fn map_ownership_script_result(result: Result<String, String>) -> Result<String, SessionOwnership> {
    result.map_err(SessionOwnership::Indeterminate)
}

fn parse_session_ownership(stdout: &str) -> SessionOwnership {
    let response = match parse_ownership_response(stdout) {
        Ok(response) => response,
        Err(reason) => return SessionOwnership::Indeterminate(reason),
    };
    validate_ownership_response(&response)
}

fn parse_ownership_response(stdout: &str) -> Result<OwnershipResponse, String> {
    let line = parse_single_ownership_line(stdout)?;
    match serde_json::from_str(line) {
        Ok(response) => Ok(response),
        Err(error) => Err(format!("ownership_script_malformed_json: {error}")),
    }
}

fn parse_single_ownership_line(stdout: &str) -> Result<&str, String> {
    let lines = stdout.lines().collect::<Vec<_>>();
    match lines.as_slice() {
        [] => Err("ownership_script_empty_stdout".to_string()),
        [line] if !line.trim().is_empty() => Ok(line.trim()),
        [_] => Err("ownership_script_empty_stdout".to_string()),
        _ => Err("ownership_script_stdout_not_single_line".to_string()),
    }
}

fn validate_ownership_response(response: &OwnershipResponse) -> SessionOwnership {
    match response.owned {
        Some(true) => SessionOwnership::Owned,
        Some(false) => validate_negative_ownership(response.error.as_deref()),
        None => validate_missing_ownership(response.error.as_deref()),
    }
}

fn validate_negative_ownership(error: Option<&str>) -> SessionOwnership {
    match error {
        None => SessionOwnership::NotOwned,
        Some(error) if error.trim().is_empty() => SessionOwnership::NotOwned,
        Some(error) => {
            SessionOwnership::Indeterminate(format!("ownership_script_error: {}", error.trim()))
        }
    }
}

fn validate_missing_ownership(error: Option<&str>) -> SessionOwnership {
    match error {
        Some(error) if !error.trim().is_empty() => {
            SessionOwnership::Indeterminate(format!("ownership_not_reported: {}", error.trim()))
        }
        _ => SessionOwnership::Indeterminate("ownership_not_reported".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(stdout: &str) -> SessionStorage {
        SessionStorage::Script {
            cwd_script: format!("printf %s {}; :", shell_quote(stdout)),
            transcript_script: None,
            storage_type: None,
        }
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    #[test]
    fn session_ownership_is_independent_of_cwd_usability() {
        for response in [
            "{\"owned\":true,\"found\":true,\"cwd\":\"/tmp/workspace\"}\n",
            "{\"owned\":true,\"found\":false,\"error\":\"opencode_cwd_missing\"}\n",
        ] {
            assert_eq!(
                resolve_session_ownership(Some(&script(response)), "ses_owned"),
                SessionOwnership::Owned
            );
        }
    }

    #[test]
    fn session_ownership_requires_conclusive_negative_response() {
        assert_eq!(
            resolve_session_ownership(
                Some(&script("{\"owned\":false,\"found\":false}\n")),
                "ses_missing"
            ),
            SessionOwnership::NotOwned
        );
        assert!(matches!(
            resolve_session_ownership(
                Some(&script(
                    "{\"owned\":false,\"found\":false,\"error\":\"query failed\"}\n"
                )),
                "ses_unknown"
            ),
            SessionOwnership::Indeterminate(reason) if reason.contains("query failed")
        ));
    }

    #[test]
    fn session_ownership_rejects_legacy_malformed_and_multiline_output() {
        for response in [
            "{\"found\":true,\"cwd\":\"/tmp/workspace\"}\n",
            "not-json\n",
            "{\"owned\":true}\n{\"owned\":false}\n",
        ] {
            assert!(matches!(
                resolve_session_ownership(Some(&script(response)), "ses_unknown"),
                SessionOwnership::Indeterminate(_)
            ));
        }
    }

    #[test]
    fn session_ownership_reports_missing_storage_and_script_failure_as_indeterminate() {
        assert!(matches!(
            resolve_session_ownership(None, "ses_unknown"),
            SessionOwnership::Indeterminate(reason) if reason == "session_storage_missing"
        ));
        let failing = SessionStorage::Script {
            cwd_script: "exit 9".to_string(),
            transcript_script: None,
            storage_type: None,
        };
        assert!(matches!(
            resolve_session_ownership(Some(&failing), "ses_unknown"),
            SessionOwnership::Indeterminate(reason) if reason.starts_with("ownership_script_exit_9")
        ));
    }
}
