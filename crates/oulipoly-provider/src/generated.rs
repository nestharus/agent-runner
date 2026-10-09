//! Runner's provider/v1 wire DTOs.
//!
//! The DTOs are the SDK's projection of the shared contract schemas; Runner
//! does not redefine them. The host-selected extension identifiers and the
//! host-side selection checks below remain Runner-owned.

pub use agent_provider_contract::generated::*;

pub const PROMPT_ACCEPTANCE_V1: &str = "oulipoly.prompt_acceptance/v1";
pub const PROMPT_ACCEPTED_MARKER_V1: &str = "oulipoly.prompt_accepted/v1";
pub const HOST_PROMPT_ACCEPTANCE_V1_ENV: &str = "OULIPOLY_HOST_PROMPT_ACCEPTANCE_V1";
pub const HOST_PROMPT_ACCEPTANCE_V1_ENV_VALUE: &str = "1";
pub const LAUNCH_OUTPUT_V1: &str = "oulipoly.launch_output/v1";
pub const LAUNCH_OUTPUT_COMPLETE_MARKER_V1: &str = "oulipoly.launch_output_complete/v1";
pub const HOST_LAUNCH_OUTPUT_V1_ENV: &str = "OULIPOLY_HOST_LAUNCH_OUTPUT_V1";
pub const HOST_LAUNCH_OUTPUT_V1_ENV_VALUE: &str = "1";
pub const SESSION_TURN_PAGES_V1: &str = "oulipoly.session_turn_pages/v1";
pub const HOST_SESSION_TURN_PAGES_V1_ENV: &str = "OULIPOLY_HOST_SESSION_TURN_PAGES_V1";
pub const HOST_SESSION_TURN_PAGES_V1_ENV_VALUE: &str = "1";
pub const HOST_TERMINAL_UNAVAILABLE_V1_ENV: &str = "OULIPOLY_HOST_TERMINAL_UNAVAILABLE_V1";
pub const HOST_TERMINAL_UNAVAILABLE_V1_ENV_VALUE: &str = "1";

pub fn host_requested_prompt_acceptance_v1(host: &HostContext) -> bool {
    host.env
        .get(HOST_PROMPT_ACCEPTANCE_V1_ENV)
        .map(String::as_str)
        == Some(HOST_PROMPT_ACCEPTANCE_V1_ENV_VALUE)
}

pub fn host_requested_launch_output_v1(host: &HostContext) -> bool {
    host.env.get(HOST_LAUNCH_OUTPUT_V1_ENV).map(String::as_str)
        == Some(HOST_LAUNCH_OUTPUT_V1_ENV_VALUE)
}

pub fn host_requested_session_turn_pages_v1(host: &HostContext) -> bool {
    host.env
        .get(HOST_SESSION_TURN_PAGES_V1_ENV)
        .map(String::as_str)
        == Some(HOST_SESSION_TURN_PAGES_V1_ENV_VALUE)
}

pub fn host_requested_terminal_unavailable_v1(host: &HostContext) -> bool {
    host.env
        .get(HOST_TERMINAL_UNAVAILABLE_V1_ENV)
        .map(String::as_str)
        == Some(HOST_TERMINAL_UNAVAILABLE_V1_ENV_VALUE)
}

/// The wire spelling of a diagnostic severity.
pub fn diagnostic_severity_str(severity: &DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Info => "info",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Error => "error",
    }
}
