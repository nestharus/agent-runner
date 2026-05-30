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

fn orchestration_source() -> &'static str {
    include_str!("../src/commands/test_model/orchestration.rs")
}

fn dispatch_source() -> &'static str {
    include_str!("../src/commands/test_model/dispatch.rs")
}

fn mapper_source() -> &'static str {
    include_str!("../src/commands/test_model/mapper.rs")
}

fn validator_source() -> &'static str {
    include_str!("../src/commands/test_model/validator.rs")
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
        orchestration_source(),
        "async fn test_model(",
        "pub(crate) fn test_model_with_db_path",
    )
}

fn combined_test_model_surface() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        test_model_body(),
        orchestration_source(),
        validator_source(),
        dispatch_source(),
        mapper_source()
    )
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
    // assumption-register (AGE-156 consolidation):
    //
    // - `TerminalSignalKind::QuotaExhaustedInband` is the authoritative driver
    //   for the durable `provider_quotas.exhausted_at` write here. The
    //   per-provider partitioned recognizers (AGE-162 WU-B in claude.rs /
    //   codex.rs / openai_compat.rs) emit `QuotaExhaustedInband` ONLY for
    //   persistent quota signatures; transient rate-limit signatures emit
    //   `TerminalSignalKind::RateLimited` and must not write `exhausted_at`.
    // - `DiagnosticsServiceRequest::ClassifyExhaustion` remains the
    //   degraded-mode fallback that runs only when the typed terminal-signal
    //   is absent (e.g., pre-typed-signal provider in a degraded execution
    //   path).
    let body = combined_test_model_surface();
    for required in [
        "select_route(RoutingServiceRequest",
        "ctx: None",
        "ExecutorServiceRequest::Effective",
        "working_dir: None",
        "parent_invocation_env: None",
        "should_run_diagnostics_fallback(result.exit_code)",
        "result.terminal_signal",
        "TerminalSignalKind::QuotaExhaustedInband",
        "DiagnosticsServiceRequest::ClassifyExhaustion",
        "if validator::should_mark_quota_exhausted",
        "<StateDb as ProviderQuotaRepository>::mark_exhausted",
    ] {
        assert_contains(
            &body,
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
            &body,
            forbidden,
            "test_model_with_db_path must stay outside lifecycle mutation paths",
        );
    }
}

#[test]
fn age156_test_model_with_db_path_gates_legacy_classifier_behind_typed_signal_absence() {
    // AGE-156 acceptance: typed-signal precedence — the legacy broad-string
    // classifier (`DiagnosticsServiceRequest::ClassifyExhaustion`) may only
    // run on the degraded-mode `None` branch of `result.terminal_signal`.
    // The persistent-quota typed kind `QuotaExhaustedInband` must be the
    // authority on the `Some(signal)` branch.
    let body = combined_test_model_surface();

    let signal_idx = body
        .find("result.terminal_signal")
        .expect("typed-signal precedence: `result.terminal_signal` access must appear");
    let typed_idx = body
        .find("TerminalSignalKind::QuotaExhaustedInband")
        .expect("typed-signal precedence: `QuotaExhaustedInband` match must appear");
    let legacy_idx = body
        .find("DiagnosticsServiceRequest::ClassifyExhaustion")
        .expect("degraded-mode fallback: legacy classifier must remain reachable");

    assert!(
        signal_idx < typed_idx,
        "typed-signal precedence: signal access must precede the kind match"
    );
    assert!(
        typed_idx < legacy_idx,
        "typed-signal precedence: the `QuotaExhaustedInband` typed kind must \
         be checked before the legacy classifier is consulted (legacy is the \
         degraded-mode fallback only)"
    );
}
