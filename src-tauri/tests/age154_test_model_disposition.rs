//! ## Declared roles
//! accessor, parser, validator, mapper, formatter
//!
//! Function role map:
//! - `lib_source`, `test_model_body`, `test_model_with_db_path_body`: accessor
//! - `source_bounds`: validator
//! - `source_between`: parser
//! - `combined_test_model_surface`: formatter
//! - `assert_contains`, `assert_not_contains`, and `age154_*` tests: validator

struct SourceBounds {
    body_start: usize,
    body_end: usize,
}

fn lib_source() -> &'static str {
    include_str!("../src/lib.rs")
}

fn source_bounds(source: &str, start: &str, end: &str) -> SourceBounds {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing source start marker {start}"))
        + start.len();
    let end_index = source[start_index..]
        .find(end)
        .unwrap_or_else(|| panic!("missing source end marker {end}"));

    SourceBounds {
        body_start: start_index,
        body_end: start_index + end_index,
    }
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let bounds = source_bounds(source, start, end);
    &source[bounds.body_start..bounds.body_end]
}

fn test_model_body() -> &'static str {
    source_between(
        lib_source(),
        "async fn test_model(",
        "struct TestModelServices",
    )
}

fn test_model_with_db_path_body() -> &'static str {
    source_between(
        lib_source(),
        "fn test_model_with_db_path(",
        "fn diagnostic_input(",
    )
}

fn combined_test_model_surface() -> String {
    format!("{}\n{}", test_model_body(), test_model_with_db_path_body())
}

fn assert_contains(haystack: &str, needle: &str, context: &str) {
    assert!(
        haystack.contains(needle),
        "{context}: expected to find `{needle}`"
    );
}

fn assert_not_contains(haystack: &str, needle: &str, context: &str) {
    assert!(
        !haystack.contains(needle),
        "{context}: must not contain `{needle}`"
    );
}

#[test]
fn age154_test_model_and_db_path_do_not_enter_lifecycle_modes_or_emit_markers() {
    // assumption-register: `test_model` remains outside the four supported invocation lifecycle modes.
    // residual-risk-not-verified: AGE-154 records accepted divergence; AGE-150 owns consolidation.
    let surface = combined_test_model_surface();
    for forbidden in [
        "run_with_balancing(",
        "run_resume(",
        "run_repl(",
        "dispatch_subcommand(Subcommands::Repl",
        "Subcommands::Repl",
        "default_provider",
        "OULIPOLY_INVOCATION",
        "OULIPOLY_RESULT",
    ] {
        assert_not_contains(
            &surface,
            forbidden,
            "expected observable signal: no lifecycle entrypoint or marker emission",
        );
    }
}

#[test]
fn age154_test_model_keeps_accepted_service_boundary_disposition() {
    // assumption-register: `DiagnosticsServiceRequest::ClassifyExhaustion` remains legacy fallback/GUI diagnostics.
    let body = test_model_with_db_path_body();
    for required in [
        "select_route(RoutingServiceRequest",
        "ctx: None",
        "ExecutorServiceRequest::Effective",
        "working_dir: None",
        "parent_invocation_env: None",
        "DiagnosticsServiceRequest::ClassifyExhaustion",
        "result.exit_code != 0",
        "if is_exhausted",
        "<StateDb as ProviderQuotaRepository>::mark_exhausted",
    ] {
        assert_contains(
            body,
            required,
            "expected observable signal: accepted test_model service boundary",
        );
    }
    for forbidden in [
        "start_invocation(",
        "finalize_invocation(",
        "record_returned_artifacts(",
        "increment_calls_since_refresh(",
        "db.mark_exhausted(",
    ] {
        assert_not_contains(
            body,
            forbidden,
            "test_model_with_db_path must stay outside lifecycle mutation paths",
        );
    }
}
