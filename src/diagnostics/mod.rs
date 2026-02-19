use crate::config::ModelConfig;
use crate::executor;
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
            ErrorCategory::Unknown => "unknown",
        }
    }
}

pub fn diagnose_error(
    stderr: &str,
    exit_code: i32,
    diagnostics_model: &ModelConfig,
    _models: &HashMap<String, ModelConfig>,
    working_dir: Option<&Path>,
) -> Result<Diagnosis, String> {
    // Truncate stderr for the diagnostic prompt
    let truncated: String = stderr.chars().take(MAX_STDERR_LEN).collect();

    let prompt = format!(
        "Analyze this CLI error and classify it into exactly one category.\n\
         \n\
         Exit code: {exit_code}\n\
         Stderr:\n```\n{truncated}\n```\n\
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
    );

    let result = executor::execute(diagnostics_model, 0, &prompt, working_dir)?;

    if result.exit_code != 0 {
        // Diagnostics model itself failed — use heuristic fallback
        return Ok(heuristic_diagnosis(stderr, exit_code));
    }

    parse_diagnosis(&result.stdout, stderr, exit_code)
}

fn parse_diagnosis(output: &str, stderr: &str, exit_code: i32) -> Result<Diagnosis, String> {
    let lines: Vec<&str> = output.trim().lines().collect();
    if lines.is_empty() {
        return Ok(heuristic_diagnosis(stderr, exit_code));
    }

    let category = match lines[0].trim() {
        "rate_limit" => ErrorCategory::RateLimit,
        "quota_exhausted" => ErrorCategory::QuotaExhausted,
        "auth_expired" => ErrorCategory::AuthExpired,
        "cli_version_mismatch" => ErrorCategory::CliVersionMismatch,
        "network_error" => ErrorCategory::NetworkError,
        _ => ErrorCategory::Unknown,
    };

    let summary = if lines.len() > 1 {
        lines[1..].join("\n")
    } else {
        String::new()
    };

    Ok(Diagnosis { category, summary })
}

fn heuristic_diagnosis(stderr: &str, _exit_code: i32) -> Diagnosis {
    let lower = stderr.to_lowercase();

    let category = if lower.contains("429") || lower.contains("rate limit") || lower.contains("too many requests") {
        ErrorCategory::RateLimit
    } else if lower.contains("quota") || lower.contains("billing") || lower.contains("usage limit") {
        ErrorCategory::QuotaExhausted
    } else if lower.contains("unauthorized") || lower.contains("auth") || lower.contains("token expired") {
        ErrorCategory::AuthExpired
    } else if lower.contains("not found") || lower.contains("unknown flag") || lower.contains("unrecognized") {
        ErrorCategory::CliVersionMismatch
    } else if lower.contains("connection") || lower.contains("timeout") || lower.contains("dns") {
        ErrorCategory::NetworkError
    } else {
        ErrorCategory::Unknown
    };

    Diagnosis {
        category,
        summary: format!("Heuristic classification based on stderr content"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_rate_limit() {
        let d = heuristic_diagnosis("Error: 429 Too Many Requests", 1);
        assert_eq!(d.category, ErrorCategory::RateLimit);
    }

    #[test]
    fn heuristic_auth() {
        let d = heuristic_diagnosis("Error: Unauthorized - token expired", 1);
        assert_eq!(d.category, ErrorCategory::AuthExpired);
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
        assert_eq!(d.category, ErrorCategory::RateLimit);
    }
}
