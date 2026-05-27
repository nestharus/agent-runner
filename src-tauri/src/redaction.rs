//! ## Declared roles
//!
//! `formatter`, `filter`, `mapper`, `parser`, `predicate`, `validator`

use regex::Regex;
use std::sync::OnceLock;

pub(crate) fn diagnostic_input(stderr: &str, stdout: &[u8]) -> String {
    let stdout = decode_stdout(stdout);
    let sources = diagnostic_sources(stderr, &stdout);
    render_diagnostic_sources(sources)
}

fn decode_stdout(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout).into_owned()
}

fn diagnostic_sources<'a>(stderr: &'a str, stdout: &'a str) -> DiagnosticSources<'a> {
    diagnostic_sources_from_parts(stderr.trim(), stdout.trim())
}

fn diagnostic_sources_from_parts<'a>(stderr: &'a str, stdout: &'a str) -> DiagnosticSources<'a> {
    DiagnosticSources { stderr, stdout }
}

struct DiagnosticSources<'a> {
    stderr: &'a str,
    stdout: &'a str,
}

fn render_diagnostic_sources(sources: DiagnosticSources<'_>) -> String {
    let stdout = sources.stdout;
    let stderr = sources.stderr;
    match (stderr.is_empty(), stdout.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stderr.to_string(),
        (true, false) => stdout.to_string(),
        (false, false) => format!("{stderr}\n{stdout}"),
    }
}

pub(crate) fn redacted_stderr_excerpt(stderr: &str) -> String {
    truncate_utf8_bytes(&first_nonempty_lines(&redact_sensitive(stderr), 4), 1024)
}

fn redact_sensitive(text: &str) -> String {
    let text = redact_authorization_headers(text);
    let text = redact_bearer_tokens(&text);
    redact_sensitive_key_values(&text)
}

fn redact_bearer_tokens(text: &str) -> String {
    bearer_token_regex()
        .replace_all(text, "Bearer [REDACTED]")
        .into_owned()
}

fn redact_sensitive_key_values(text: &str) -> String {
    sensitive_key_value_regex()
        .replace_all(text, "$1$2[REDACTED]")
        .into_owned()
}

fn bearer_token_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\bbearer\s+[^\s]+").expect("bearer redaction regex must compile")
    })
}

fn sensitive_key_value_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)(["']?\b(?:token|api_key|apikey|password|secret)\b["']?\s*[:=]\s*)(["']?)[^\s"']+"#,
        )
        .expect("key-value redaction regex must compile")
    })
}

fn redact_authorization_headers(text: &str) -> String {
    let mut redacted = String::with_capacity(text.len());
    for segment in text.split_inclusive('\n') {
        redacted.push_str(&redact_authorization_segment(
            segment,
            authorization_header_regex(),
        ));
    }
    redacted
}

fn authorization_header_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\bauthorization\s*:\s*").expect("authorization regex must compile")
    })
}

fn redact_authorization_segment(segment: &str, authorization: &Regex) -> String {
    let (line, newline) = split_line_newline(segment);
    let matches = authorization.find_iter(line).collect::<Vec<_>>();
    if matches.is_empty() {
        return render_line_with_newline(line, newline);
    }
    render_redacted_authorization_line(line, newline, &matches)
}

fn split_line_newline(segment: &str) -> (&str, &str) {
    segment
        .strip_suffix('\n')
        .map_or((segment, ""), |line| (line, "\n"))
}

fn render_line_with_newline(line: &str, newline: &str) -> String {
    format!("{line}{newline}")
}

fn render_redacted_authorization_line(
    line: &str,
    newline: &str,
    matches: &[regex::Match<'_>],
) -> String {
    let mut redacted = String::with_capacity(line.len());
    let mut cursor = 0;
    for (index, header) in matches.iter().enumerate() {
        redacted.push_str(&line[cursor..header.end()]);
        let value_start = header.end();
        let value_end = matches
            .get(index + 1)
            .map_or(line.len(), |next| next.start());
        if authorization_value_is_present(&line[value_start..value_end]) {
            redacted.push_str("[REDACTED]");
        }
        cursor = value_end;
    }
    redacted.push_str(&line[cursor..]);
    redacted.push_str(newline);
    redacted
}

fn authorization_value_is_present(value: &str) -> bool {
    value.chars().any(|character| !character.is_whitespace())
}

fn first_nonempty_lines(text: &str, max_lines: usize) -> String {
    join_lines(first_nonempty_line_slice(text, max_lines))
}

fn first_nonempty_line_slice(text: &str, max_lines: usize) -> Vec<&str> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .take(max_lines)
        .collect::<Vec<_>>()
}

fn join_lines(lines: Vec<&str>) -> String {
    lines.join("\n")
}

fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_input_includes_stdout_when_provider_reports_errors_there() {
        let stdout = br#"{"api_error_status":429,"result":"You've hit your limit"}"#;

        assert_eq!(
            diagnostic_input("", stdout),
            r#"{"api_error_status":429,"result":"You've hit your limit"}"#
        );
        assert_eq!(
            diagnostic_input("stderr line", stdout),
            "stderr line\n{\"api_error_status\":429,\"result\":\"You've hit your limit\"}"
        );
    }
}
