//! Runner's provider/v1 wire DTOs.
//!
//! The DTOs are the SDK's projection of the shared contract schemas; Runner
//! does not redefine them. Base extension vocabulary also comes from the SDK;
//! Runner preserves its public aliases and owns host policy and semantic checks.

pub use agent_provider_contract::generated::*;

pub use agent_provider_contract::host_extensions::{
    OPT_IN_VALUE as HOST_PROMPT_ACCEPTANCE_V1_ENV_VALUE,
    OPT_IN_VALUE as HOST_LAUNCH_OUTPUT_V1_ENV_VALUE,
    OPT_IN_VALUE as HOST_SESSION_TURN_PAGES_V1_ENV_VALUE,
    launch_output::{
        MARKER_NAME as LAUNCH_OUTPUT_COMPLETE_MARKER_V1, PROTOCOL as LAUNCH_OUTPUT_V1,
        SELECTOR as HOST_LAUNCH_OUTPUT_V1_ENV,
    },
    prompt_acceptance::{
        MARKER_NAME as PROMPT_ACCEPTED_MARKER_V1, PROTOCOL as PROMPT_ACCEPTANCE_V1,
        SELECTOR as HOST_PROMPT_ACCEPTANCE_V1_ENV,
    },
    session_turn_pages::{
        PROTOCOL as SESSION_TURN_PAGES_V1, SELECTOR as HOST_SESSION_TURN_PAGES_V1_ENV,
    },
};
pub use agent_provider_contract::terminal_unavailable::{
    HOST_SELECTION_ENV as HOST_TERMINAL_UNAVAILABLE_V1_ENV,
    HOST_SELECTION_VALUE as HOST_TERMINAL_UNAVAILABLE_V1_ENV_VALUE,
};

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
    agent_provider_contract::terminal_unavailable::host_selected(host)
}

/// The wire spelling of a diagnostic severity.
pub fn diagnostic_severity_str(severity: &DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Info => "info",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Error => "error",
    }
}
