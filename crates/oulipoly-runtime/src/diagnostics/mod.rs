//! ## Declared roles
//!
//! `accessor`, `mapper`, `parser`, `formatter`, `predicate`, `orchestration`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/diagnostics/mod.rs
//!     role: intrinsic-surface
//!     Domain: diagnostic_classification
//!     Owns:
//!       - error category vocabulary
//!       - non-authoritative quota/rate-limit classification fallback (`classify_exhaustion` always false post-PR #126)
//!       - diagnostic model port + prompt structure
//!       - service DTO + request/error shape
//! ```

use crate::executor;
use crate::services::{
    DiagnosticsServiceOutput, DiagnosticsServicePort, DiagnosticsServiceRequest, ServiceError,
};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use std::collections::HashMap;
use std::path::Path;

const MAX_STDERR_LEN: usize = 2000;

#[derive(Debug)]
pub struct Diagnosis {
    pub category: ErrorCategory,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    RateLimit,
    QuotaExhausted,
    AuthExpired,
    CliVersionMismatch,
    NetworkError,
    HungSubprocess,
    ResumeSessionMismatch,
    Unknown,
}

impl ErrorCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCategory::RateLimit => "rate_limit",
            ErrorCategory::QuotaExhausted => "quota_exhausted",
            ErrorCategory::AuthExpired => "auth_expired",
            ErrorCategory::CliVersionMismatch => "cli_version_mismatch",
            ErrorCategory::NetworkError => "network_error",
            ErrorCategory::HungSubprocess => "hung_subprocess",
            ErrorCategory::ResumeSessionMismatch => "resume_session_mismatch",
            ErrorCategory::Unknown => "unknown",
        }
    }
}

pub struct RuntimeDiagnosticsService;

impl DiagnosticsServicePort for RuntimeDiagnosticsService {
    fn diagnose(
        &self,
        request: DiagnosticsServiceRequest,
    ) -> Result<DiagnosticsServiceOutput, ServiceError> {
        match request {
            DiagnosticsServiceRequest::ClassifyExhaustion { stderr } => {
                Ok(DiagnosticsServiceOutput::ExhaustionClassification {
                    is_exhausted: classify_exhaustion(&stderr),
                })
            }
            DiagnosticsServiceRequest::DiagnoseError {
                diagnostics_model,
                effective_provider,
                provider_index,
                prompt_mode,
                exit_code,
                stderr,
                working_dir,
            } => diagnose_error(
                &diagnostics_model,
                &effective_provider,
                provider_index,
                prompt_mode,
                exit_code,
                &stderr,
                working_dir.as_deref(),
            )
            .map(|diagnosis| DiagnosticsServiceOutput::Diagnosis { diagnosis })
            .map_err(|message| ServiceError::Dependency { message }),
        }
    }
}

pub fn classify_exhaustion(stderr: &str) -> bool {
    quota_exhaustion_text(&stderr.to_lowercase())
}

fn truncate_stderr_for_prompt(stderr: &str) -> String {
    stderr.chars().take(MAX_STDERR_LEN).collect()
}

fn format_diagnosis_prompt(exit_code: i32, truncated: &str) -> String {
    format!(
        "Analyze this CLI error and classify it into exactly one category.\n\
         \n\
         Exit code: {exit_code}\n\
         Provider output:\n```\n{truncated}\n```\n\
         \n\
         Categories:\n\
         - rate_limit: HTTP 429, too many requests, rate limited\n\
         - quota_exhausted: Quota exceeded, billing limit, usage cap\n\
         - auth_expired: Authentication failed, token expired, unauthorized\n\
         - cli_version_mismatch: Command not found, unknown flag, version incompatible\n\
         - network_error: Connection refused, timeout, DNS failure\n\
         - unknown: None of the above\n\
         \n\
         Respond with ONLY the category name on the first line, then a brief explanation on the second line.\n\
         Example:\n\
         rate_limit\n\
         The API returned HTTP 429 indicating too many requests."
    )
}

pub fn diagnose_error(
    diagnostics_model: &ModelConfig,
    effective_provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    exit_code: i32,
    stderr: &str,
    working_dir: Option<&Path>,
) -> Result<Diagnosis, String> {
    let truncated = truncate_stderr_for_prompt(stderr);
    let prompt = format_diagnosis_prompt(exit_code, &truncated);

    let extra_inputs = HashMap::new();
    let result =
        executor::execute_effective_with_inputs_and_env(executor::cli::EffectiveExecuteRequest {
            model: diagnostics_model,
            provider: effective_provider,
            provider_index,
            prompt_mode,
            prompt: &prompt,
            working_dir,
            extra_inputs: &extra_inputs,
            parent_invocation_env: None,
        })?;

    if result.exit_code != 0 {
        return Ok(heuristic_diagnosis(stderr, exit_code));
    }

    let stdout_str = String::from_utf8_lossy(&result.stdout);
    parse_diagnosis(&stdout_str, stderr, exit_code)
}

fn parse_diagnosis_output_lines(output: &str) -> Vec<&str> {
    output.trim().lines().collect()
}

fn map_error_category_token(token: &str) -> ErrorCategory {
    match token {
        "rate_limit" => ErrorCategory::RateLimit,
        "quota_exhausted" => ErrorCategory::QuotaExhausted,
        "auth_expired" => ErrorCategory::AuthExpired,
        "cli_version_mismatch" => ErrorCategory::CliVersionMismatch,
        "network_error" => ErrorCategory::NetworkError,
        "hung_subprocess" => ErrorCategory::HungSubprocess,
        "resume_session_mismatch" => ErrorCategory::ResumeSessionMismatch,
        _ => ErrorCategory::Unknown,
    }
}

fn diagnosis_summary_text(lines: &[&str]) -> String {
    if lines.len() > 1 {
        lines[1..].join("\n")
    } else {
        String::new()
    }
}

fn parse_diagnosis(output: &str, stderr: &str, exit_code: i32) -> Result<Diagnosis, String> {
    let lines = parse_diagnosis_output_lines(output);
    if lines.is_empty() {
        return Ok(heuristic_diagnosis(stderr, exit_code));
    }

    let category = map_error_category_token(lines[0].trim());

    if category == ErrorCategory::Unknown {
        let heuristic = heuristic_diagnosis(stderr, exit_code);
        if heuristic.category != ErrorCategory::Unknown {
            return Ok(heuristic);
        }
    }

    let summary = diagnosis_summary_text(&lines);
    Ok(Diagnosis { category, summary })
}

fn normalize_stderr_for_heuristic(stderr: &str) -> String {
    stderr.to_lowercase()
}

fn heuristic_error_category(lower: &str) -> ErrorCategory {
    if quota_exhaustion_text(lower) {
        ErrorCategory::QuotaExhausted
    } else if rate_limit_text(lower) {
        ErrorCategory::RateLimit
    } else if lower.contains("unauthorized") || lower.contains("token expired") {
        ErrorCategory::AuthExpired
    } else if lower.contains("no conversation found")
        || lower.contains("no session found")
        || lower.contains("no rollout found")
        || lower.contains("thread/resume failed")
    {
        ErrorCategory::ResumeSessionMismatch
    } else if lower.contains("not found")
        || lower.contains("unknown flag")
        || lower.contains("unrecognized")
    {
        ErrorCategory::CliVersionMismatch
    } else if lower.contains("connection") || lower.contains("timeout") || lower.contains("dns") {
        ErrorCategory::NetworkError
    } else {
        ErrorCategory::Unknown
    }
}

fn heuristic_diagnosis(stderr: &str, _exit_code: i32) -> Diagnosis {
    let lower = normalize_stderr_for_heuristic(stderr);
    let category = heuristic_error_category(&lower);
    Diagnosis {
        category,
        summary: "Heuristic classification based on stderr content".to_string(),
    }
}

fn quota_exhaustion_text(_lower: &str) -> bool {
    // Per AGE-166 / PR #126: stderr substring classification is no longer
    // authoritative for quota detection. Returns false unconditionally so
    // diagnostics falls back to the LLM diagnostics path and never marks
    // a provider exhausted from heuristic stderr matching.
    false
}

fn rate_limit_text(_lower: &str) -> bool {
    false
}

// Characterization test for AGE-8 — pins current behavior of diagnostics model-backed subprocess execution in this inline test module.
#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::path::{Path, PathBuf};

    #[cfg(unix)]
    fn write_executable(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(unix)]
    fn migrated_diagnostic_model() -> ModelConfig {
        ModelConfig {
            name: "diagnostic".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(
                "diagnostic-provider",
                vec!["--raw-model-arg".to_string()],
            )],
            inputs: vec![],
            provider: None,
        }
    }

    #[cfg(unix)]
    fn effective_diagnostic_provider(script_path: PathBuf) -> ProviderConfig {
        ProviderConfig {
            name: "diagnostic-provider".to_string(),
            command: script_path.to_string_lossy().into_owned(),
            args: vec!["--effective-provider-arg".to_string()],
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        }
    }

    #[test]
    fn diagnostics_classify_exhaustion_remains_non_authoritative() {
        for stderr in [
            "error: QUOTA exceeded for this account",
            "Billing limit reached for the workspace",
            "USAGE LIMIT has been hit; try again later",
            "You've hit your limit · resets 9:50pm (America/Los_Angeles)",
            r#"{"api_error_status":429,"result":"You've hit your limit · resets 9:50pm"}"#,
            "Error: 429 Too Many Requests",
            "rate limit reached",
        ] {
            assert!(
                !classify_exhaustion(stderr),
                "quota/rate-looking stderr must not be authoritative: {stderr:?}"
            );
            let d = heuristic_diagnosis(stderr, 1);
            assert!(
                !matches!(
                    d.category,
                    ErrorCategory::QuotaExhausted | ErrorCategory::RateLimit
                ),
                "heuristic fallback must not classify quota/rate text for {stderr:?}"
            );
        }
    }

    #[test]
    fn classify_exhaustion_ignores_non_quota_errors() {
        for stderr in [
            "authentication failed: token expired",
            "network error: connection timed out",
            "compile error: expected expression before token",
            "unknown flag: --definitely-not-real",
            "process exited with status 1",
        ] {
            assert!(
                !classify_exhaustion(stderr),
                "did not expect quota exhaustion classification for {stderr:?}"
            );
        }
    }

    #[test]
    fn heuristic_rate_limit() {
        let d = heuristic_diagnosis("Error: 429 Too Many Requests", 1);
        assert_eq!(d.category, ErrorCategory::Unknown);
    }

    #[test]
    fn heuristic_claude_limit_json_is_not_quota_exhausted() {
        let d = heuristic_diagnosis(
            r#"{"type":"result","is_error":true,"api_error_status":429,"result":"You've hit your limit · resets 9:50pm"}"#,
            1,
        );
        assert_eq!(d.category, ErrorCategory::Unknown);
    }

    #[test]
    fn heuristic_codex_missing_rollout_is_resume_session_mismatch() {
        let d = heuristic_diagnosis(
            "Error: thread/resume: thread/resume failed: no rollout found for thread id 019e14d4 (code -32600)",
            1,
        );
        assert_eq!(d.category, ErrorCategory::ResumeSessionMismatch);
    }

    #[test]
    fn heuristic_auth() {
        for stderr in [
            "Error: Unauthorized",
            "authentication failed: token expired",
        ] {
            let d = heuristic_diagnosis(stderr, 1);
            assert_eq!(
                d.category,
                ErrorCategory::AuthExpired,
                "expected auth-expired classification for {stderr:?}"
            );
        }
    }

    #[test]
    fn heuristic_bare_auth_substring_is_unknown_not_auth_expired() {
        // AGE-175: auth-bearing stderr without unauthorized/token-expired
        // language must stay unknown so redaction coverage remains reachable.
        for stderr in ["Authorization: Bearer abc123", "authentication probe"] {
            let lower = normalize_stderr_for_heuristic(stderr);
            assert_eq!(
                heuristic_error_category(&lower),
                ErrorCategory::Unknown,
                "bare auth substring should not classify as auth-expired for {stderr:?}"
            );

            let d = heuristic_diagnosis(stderr, 1);
            assert_eq!(
                d.category,
                ErrorCategory::Unknown,
                "heuristic diagnosis should keep bare auth substring unknown for {stderr:?}"
            );
        }
    }

    #[test]
    fn heuristic_unknown() {
        let d = heuristic_diagnosis("Something weird happened", 1);
        assert_eq!(d.category, ErrorCategory::Unknown);
    }

    #[test]
    fn parse_llm_output() {
        let output = "rate_limit\nThe API returned HTTP 429";
        let d = parse_diagnosis(output, "", 1).unwrap();
        assert_eq!(d.category, ErrorCategory::RateLimit);
        assert!(d.summary.contains("429"));
    }

    #[test]
    fn parse_empty_output_falls_back() {
        let d = parse_diagnosis("", "429 error", 1).unwrap();
        assert_eq!(d.category, ErrorCategory::Unknown);
    }

    #[test]
    fn parse_unknown_model_output_falls_back_without_quota_heuristic() {
        let d = parse_diagnosis(
            "unknown\nnot enough context",
            "You've hit your limit · resets 9:50pm",
            1,
        )
        .unwrap();
        assert_eq!(d.category, ErrorCategory::Unknown);
    }

    #[cfg(unix)]
    #[test]
    fn diagnose_error_uses_effective_provider_for_migrated_diagnostic_model() {
        let dir = tempfile::tempdir().unwrap();
        let prompt_dump = dir.path().join("diagnostic-prompt.txt");
        let script = dir.path().join("diagnostic-provider.sh");
        write_executable(
            &script,
            &format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
cat > "{prompt_dump}"
printf 'network_error\nDiagnostic model saw network trouble\n'
"#,
                prompt_dump = prompt_dump.display()
            ),
        );
        let model = migrated_diagnostic_model();
        let provider = effective_diagnostic_provider(script);

        let diagnosis = diagnose_error(
            &model,
            &provider,
            0,
            PromptMode::Stdin,
            7,
            "opaque child failure from primary provider",
            Some(dir.path()),
        )
        .unwrap();

        assert_eq!(diagnosis.category, ErrorCategory::NetworkError);
        assert_eq!(diagnosis.summary, "Diagnostic model saw network trouble");
        let prompt = std::fs::read_to_string(prompt_dump).unwrap();
        assert!(prompt.contains("Exit code: 7"), "{prompt}");
        assert!(
            prompt.contains("opaque child failure from primary provider"),
            "{prompt}"
        );
        assert!(
            prompt.contains("- network_error: Connection refused"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Respond with ONLY the category name on the first line"),
            "{prompt}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn diagnose_error_effective_provider_nonzero_exit_uses_heuristic_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("diagnostic-provider.sh");
        write_executable(
            &script,
            r#"#!/usr/bin/env bash
set -euo pipefail
printf 'diagnostic subprocess failed\n' >&2
exit 12
"#,
        );
        let model = migrated_diagnostic_model();
        let provider = effective_diagnostic_provider(script);

        let diagnosis = diagnose_error(
            &model,
            &provider,
            0,
            PromptMode::Stdin,
            7,
            "network timeout while connecting to upstream",
            Some(dir.path()),
        )
        .unwrap();

        assert_eq!(diagnosis.category, ErrorCategory::NetworkError);
        assert_eq!(
            diagnosis.summary,
            "Heuristic classification based on stderr content"
        );
    }

    // Characterization test originally added by AGE-8 (pre-AGE-27 signature) — migrated to the
    // AGE-27 effective-provider signature so the AGE-8 prompt-content + auth_expired assertions
    // continue to characterize the diagnostics path. The AGE-8 test was `#[ignore]`d on main
    // pending AGE-27; AGE-27's signature change requires it to be migrated rather than just
    // un-ignored. See `DECISIONS.md` § AGE-27 Decision 6.
    #[cfg(unix)]
    #[test]
    fn diagnose_error_invokes_configured_model_subprocess_and_parses_output() {
        let dir = tempfile::tempdir().unwrap();
        let prompt_dump = dir.path().join("diagnostic-prompt.txt");
        let script_path = dir.path().join("diagnostic-model.sh");
        write_executable(
            &script_path,
            &format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
cat > "{prompt_dump}"
printf 'auth_expired\nDiagnostic model saw expired credentials\n'
"#,
                prompt_dump = prompt_dump.display()
            ),
        );

        let model = migrated_diagnostic_model();
        let provider = effective_diagnostic_provider(script_path);

        let diagnosis = diagnose_error(
            &model,
            &provider,
            0,
            PromptMode::Stdin,
            7,
            "opaque provider stderr",
            Some(dir.path()),
        )
        .unwrap();

        assert_eq!(diagnosis.category, ErrorCategory::AuthExpired);
        assert_eq!(
            diagnosis.summary,
            "Diagnostic model saw expired credentials"
        );

        let prompt = std::fs::read_to_string(prompt_dump).unwrap();
        assert!(prompt.contains("Exit code: 7"), "{prompt}");
        assert!(prompt.contains("opaque provider stderr"), "{prompt}");
    }
}
