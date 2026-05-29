use chrono::{Duration, Utc};
use oulipoly_config::{ModelConfig, ProviderConfig, model::PromptMode};
use oulipoly_runtime::services::{
    ProductionRoutingService, RoutingServicePort, RoutingServiceRequest,
};
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::path::Path;

fn model_with(names: &[&str]) -> ModelConfig {
    ModelConfig {
        name: "age153-balancer".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: names
            .iter()
            .map(|name| ProviderConfig::new(*name, vec![]))
            .collect(),
        inputs: vec![],
        provider: None,
    }
}

fn in_memory_state() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

fn seed_live_window(db: &StateDb, provider_name: &str) {
    db.upsert_quota_refresh(
        provider_name,
        &[QuotaWindowInput {
            used_percent: 0.20,
            resets_at: Utc::now() + Duration::hours(5),
        }],
    )
    .unwrap();
}

#[test]
fn balancer_mod_has_no_terminal_signal_or_provider_output_parser_references() {
    for (module_path, source) in balancer_production_sources() {
        let code = source_without_comments(source);
        for forbidden in ["TerminalSignal", "TerminalSignalKind", "terminal_signal"] {
            assert!(
                !contains_identifier_token(&code, forbidden),
                "{module_path} must not reference terminal-signal identifier token {forbidden:?}"
            );
        }
        assert!(
            !contains_terminal_signal_use_import(&code),
            "{module_path} must not import terminal_signal modules or TerminalSignal types"
        );
        assert!(
            !contains_provider_output_parser_identifier(&code),
            "{module_path} must not call provider-output parser functions as routing authority"
        );
    }
}

fn balancer_production_sources() -> [(&'static str, &'static str); 6] {
    [
        (
            "crates/oulipoly-runtime/src/balancer/mod.rs",
            production_balancer_source(
                "crates/oulipoly-runtime/src/balancer/mod.rs",
                include_str!("../src/balancer/mod.rs"),
            ),
        ),
        (
            "crates/oulipoly-runtime/src/balancer/eligibility.rs",
            production_balancer_source(
                "crates/oulipoly-runtime/src/balancer/eligibility.rs",
                include_str!("../src/balancer/eligibility.rs"),
            ),
        ),
        (
            "crates/oulipoly-runtime/src/balancer/context.rs",
            production_balancer_source(
                "crates/oulipoly-runtime/src/balancer/context.rs",
                include_str!("../src/balancer/context.rs"),
            ),
        ),
        (
            "crates/oulipoly-runtime/src/balancer/snapshot.rs",
            production_balancer_source(
                "crates/oulipoly-runtime/src/balancer/snapshot.rs",
                include_str!("../src/balancer/snapshot.rs"),
            ),
        ),
        (
            "crates/oulipoly-runtime/src/balancer/refresh_inputs.rs",
            production_balancer_source(
                "crates/oulipoly-runtime/src/balancer/refresh_inputs.rs",
                include_str!("../src/balancer/refresh_inputs.rs"),
            ),
        ),
        (
            "crates/oulipoly-runtime/src/balancer/topology.rs",
            production_balancer_source(
                "crates/oulipoly-runtime/src/balancer/topology.rs",
                include_str!("../src/balancer/topology.rs"),
            ),
        ),
    ]
}

fn production_balancer_source(module_path: &str, source: &'static str) -> &'static str {
    if module_path.ends_with("/mod.rs") {
        return source
            .split("mod tests")
            .next()
            .expect("production balancer source before tests");
    }
    source
}

fn contains_identifier_token(source: &str, token: &str) -> bool {
    identifier_tokens(source).any(|identifier| identifier == token)
}

fn contains_terminal_signal_use_import(source: &str) -> bool {
    source.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("use ")
            && (trimmed.contains("terminal_signal") || trimmed.contains("TerminalSignal"))
    })
}

fn contains_provider_output_parser_identifier(source: &str) -> bool {
    identifier_tokens(source).any(is_provider_output_parser_identifier)
}

fn is_provider_output_parser_identifier(identifier: &str) -> bool {
    identifier == "parse_provider_output"
        || identifier.starts_with("parse_terminal_status_from_")
        || identifier.starts_with("provider_recognizer_for_")
        || ((identifier.starts_with("parse_") || identifier.starts_with("recognize_"))
            && ["stdout", "stderr", "stream", "output"]
                .iter()
                .any(|needle| identifier.contains(needle)))
}

fn identifier_tokens(source: &str) -> impl Iterator<Item = &str> {
    source
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|token| !token.is_empty())
}

fn source_without_comments(source: &str) -> String {
    let spans = rust_comment_spans(source);
    source_excluding_spans(source, spans.as_slice())
}

#[derive(Clone, Copy)]
enum CommentDelimiter {
    Line,
    Block,
}

#[derive(Clone, Copy)]
struct CommentStart {
    index: usize,
    delimiter: CommentDelimiter,
}

fn rust_comment_spans(source: &str) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let mut cursor = 0;
    while let Some(start) = next_comment_start(source, cursor) {
        let end = comment_end(source, start);
        spans.push(start.index..end);
        cursor = end;
    }
    spans
}

fn next_comment_start(source: &str, cursor: usize) -> Option<CommentStart> {
    nearest_comment_start(
        line_comment_start(source, cursor),
        block_comment_start(source, cursor),
    )
}

fn line_comment_start(source: &str, cursor: usize) -> Option<CommentStart> {
    source[cursor..].find("//").map(|offset| CommentStart {
        index: cursor + offset,
        delimiter: CommentDelimiter::Line,
    })
}

fn block_comment_start(source: &str, cursor: usize) -> Option<CommentStart> {
    source[cursor..].find("/*").map(|offset| CommentStart {
        index: cursor + offset,
        delimiter: CommentDelimiter::Block,
    })
}

fn nearest_comment_start(
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

fn earlier_comment_start(left: CommentStart, right: CommentStart) -> CommentStart {
    if left.index <= right.index {
        left
    } else {
        right
    }
}

fn comment_end(source: &str, start: CommentStart) -> usize {
    match start.delimiter {
        CommentDelimiter::Line => line_comment_end(source, start.index),
        CommentDelimiter::Block => block_comment_end(source, start.index),
    }
}

fn line_comment_end(source: &str, start: usize) -> usize {
    source[start..]
        .find('\n')
        .map(|offset| start + offset)
        .unwrap_or(source.len())
}

fn block_comment_end(source: &str, start: usize) -> usize {
    source[start + 2..]
        .find("*/")
        .map(|offset| start + 2 + offset + 2)
        .unwrap_or(source.len())
}

fn source_excluding_spans(source: &str, spans: &[std::ops::Range<usize>]) -> String {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    for span in spans {
        output.push_str(&source[cursor..span.start]);
        cursor = span.end;
    }
    output.push_str(&source[cursor..]);
    output
}

#[test]
fn production_routing_service_skips_provider_after_existing_exhausted_write_path() {
    let db = in_memory_state();
    let model = model_with(&["claude-age153-a", "claude-age153-b"]);
    seed_live_window(&db, "claude-age153-a");
    seed_live_window(&db, "claude-age153-b");
    db.mark_exhausted("claude-age153-a").unwrap();

    let route = ProductionRoutingService
        .select_route(RoutingServiceRequest {
            model: &model,
            state: &db,
            ctx: None,
        })
        .unwrap();

    assert_eq!(route.provider_index, 1);
}

#[test]
fn production_routing_service_reports_all_exhausted_after_existing_exhausted_writes() {
    let db = in_memory_state();
    let model = model_with(&["claude-age153-a", "claude-age153-b"]);
    for provider in ["claude-age153-a", "claude-age153-b"] {
        seed_live_window(&db, provider);
        db.mark_exhausted(provider).unwrap();
    }

    let err = ProductionRoutingService
        .select_route(RoutingServiceRequest {
            model: &model,
            state: &db,
            ctx: None,
        })
        .unwrap_err();

    assert!(err.to_string().contains("quota-exhausted"), "{err}");
}
