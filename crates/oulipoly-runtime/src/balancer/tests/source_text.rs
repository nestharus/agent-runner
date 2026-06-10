//! ## Declared roles
//!
//! `parser, predicate, filter, accessor`.

pub(in crate::balancer::tests) fn assert_age225_b4_balancer_modules_are_declared(
    guard_name: &str,
    guard_body: &str,
) {
    for module in ["migration.rs", "working_set.rs"] {
        assert!(
            guard_body.contains(&format!("src/balancer/{module}")),
            "{guard_name} must name crates/oulipoly-runtime/src/balancer/{module}"
        );
        assert!(
            guard_body.contains(&format!("include_str!(\"../{module}\")"))
                || guard_body.contains(&format!("include_str!(\"{module}\")")),
            "{guard_name} must compile-time include balancer/{module}"
        );
    }
}

pub(in crate::balancer::tests) fn balancer_source_list_body<'a>(
    source: &'a str,
    start: &str,
    _end: &str,
) -> &'a str {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing source-list start marker {start}"));
    let after_start = &source[start_index..];
    rust_function_source(after_start)
}

pub(in crate::balancer::tests) fn rust_function_source(source: &str) -> &str {
    let body_start = source
        .find('{')
        .unwrap_or_else(|| panic!("missing function body"));
    let mut depth = 0usize;
    for (offset, ch) in source[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[..body_start + offset + ch.len_utf8()];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated function body")
}

pub(in crate::balancer::tests) fn contains_identifier_token(source: &str, token: &str) -> bool {
    identifier_tokens(source).any(|identifier| identifier == token)
}

pub(in crate::balancer::tests) fn contains_terminal_signal_use_import(source: &str) -> bool {
    source.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("use ")
            && (trimmed.contains("terminal_signal") || trimmed.contains("TerminalSignal"))
    })
}

pub(in crate::balancer::tests) fn contains_provider_output_parser_identifier(source: &str) -> bool {
    identifier_tokens(source).any(is_provider_output_parser_identifier)
}

pub(in crate::balancer::tests) fn is_provider_output_parser_identifier(identifier: &str) -> bool {
    identifier == "parse_provider_output"
        || identifier.starts_with("parse_terminal_status_from_")
        || identifier.starts_with("provider_recognizer_for_")
        || ((identifier.starts_with("parse_") || identifier.starts_with("recognize_"))
            && ["stdout", "stderr", "stream", "output"]
                .iter()
                .any(|needle| identifier.contains(needle)))
}

pub(in crate::balancer::tests) fn identifier_tokens(source: &str) -> impl Iterator<Item = &str> {
    source
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|token| !token.is_empty())
}

pub(in crate::balancer::tests) fn source_without_comments(source: &str) -> String {
    let spans = rust_comment_spans(source);
    source_excluding_spans(source, spans.as_slice())
}

#[derive(Clone, Copy)]
pub(in crate::balancer::tests) enum CommentDelimiter {
    Line,
    Block,
}

#[derive(Clone, Copy)]
pub(in crate::balancer::tests) struct CommentStart {
    index: usize,
    delimiter: CommentDelimiter,
}

pub(in crate::balancer::tests) fn rust_comment_spans(source: &str) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let mut cursor = 0;
    while let Some(start) = next_comment_start(source, cursor) {
        let end = comment_end(source, start);
        spans.push(start.index..end);
        cursor = end;
    }
    spans
}

pub(in crate::balancer::tests) fn next_comment_start(
    source: &str,
    cursor: usize,
) -> Option<CommentStart> {
    nearest_comment_start(
        line_comment_start(source, cursor),
        block_comment_start(source, cursor),
    )
}

pub(in crate::balancer::tests) fn line_comment_start(
    source: &str,
    cursor: usize,
) -> Option<CommentStart> {
    source[cursor..].find("//").map(|offset| CommentStart {
        index: cursor + offset,
        delimiter: CommentDelimiter::Line,
    })
}

pub(in crate::balancer::tests) fn block_comment_start(
    source: &str,
    cursor: usize,
) -> Option<CommentStart> {
    source[cursor..].find("/*").map(|offset| CommentStart {
        index: cursor + offset,
        delimiter: CommentDelimiter::Block,
    })
}

pub(in crate::balancer::tests) fn nearest_comment_start(
    line: Option<CommentStart>,
    block: Option<CommentStart>,
) -> Option<CommentStart> {
    match (line, block) {
        (Some(line), Some(block)) => Some(earlier_comment_start(line, block)),
        (Some(line), None) => Some(line),
        (None, Some(block)) => Some(block),
        (None, None) => None,
    }
}

pub(in crate::balancer::tests) fn earlier_comment_start(
    left: CommentStart,
    right: CommentStart,
) -> CommentStart {
    if left.index <= right.index {
        left
    } else {
        right
    }
}

pub(in crate::balancer::tests) fn comment_end(source: &str, start: CommentStart) -> usize {
    match start.delimiter {
        CommentDelimiter::Line => line_comment_end(source, start.index),
        CommentDelimiter::Block => block_comment_end(source, start.index),
    }
}

pub(in crate::balancer::tests) fn line_comment_end(source: &str, start: usize) -> usize {
    source[start..]
        .find('\n')
        .map(|offset| start + offset)
        .unwrap_or(source.len())
}

pub(in crate::balancer::tests) fn block_comment_end(source: &str, start: usize) -> usize {
    source[start + 2..]
        .find("*/")
        .map(|offset| start + 2 + offset + 2)
        .unwrap_or(source.len())
}

pub(in crate::balancer::tests) fn source_excluding_spans(
    source: &str,
    spans: &[std::ops::Range<usize>],
) -> String {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    for span in spans {
        output.push_str(&source[cursor..span.start]);
        cursor = span.end;
    }
    output.push_str(&source[cursor..]);
    output
}
