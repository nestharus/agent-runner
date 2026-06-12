fn disposition_source() -> &'static str {
    concat!(
        include_str!("../src/run/balancing/disposition.rs"),
        "\n",
        include_str!("../src/run/balancing/formatter.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/completed.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/disposition.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/quota.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/shared.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/spawn.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/config.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/invocation.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/quota.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/routing.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/session.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/executor_request.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/failure.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/finalizer_request.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/session_ingest.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/terminal.rs"),
        "\n",
        include_str!("../src/run/balancing/predicate.rs"),
        "\n",
        include_str!("../src/run/balancing/state_update.rs"),
        "\n",
        include_str!("../src/run/balancing/validator.rs"),
    )
}

fn finalization_source() -> &'static str {
    concat!(
        include_str!("../src/run/balancing/finalization.rs"),
        "\n",
        include_str!("../src/run/balancing/formatter.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/completed.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/disposition.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/quota.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/shared.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/spawn.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/config.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/invocation.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/quota.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/routing.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/session.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/executor_request.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/failure.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/finalizer_request.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/session_ingest.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/terminal.rs"),
        "\n",
        include_str!("../src/run/balancing/predicate.rs"),
        "\n",
        include_str!("../src/run/balancing/state_update.rs"),
        "\n",
        include_str!("../src/run/balancing/validator.rs"),
    )
}

fn production_block_after<'a>(source: &'a str, start: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    let open_idx = source[start_idx..]
        .find('{')
        .map(|idx| start_idx + idx)
        .unwrap_or_else(|| panic!("missing opening brace after {start}"));
    let mut depth = 1usize;
    let mut idx = open_idx + 1;
    let bytes = source.as_bytes();

    while idx < bytes.len() {
        match bytes[idx] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open_idx + 1..idx];
                }
            }
            _ => {}
        }
        idx += 1;
    }

    panic!("missing closing brace after {start}");
}

#[test]
fn age153_typed_signal_precedence_runs_before_legacy_diagnostics_in_balancing_path() {
    let balanced = production_block_after(finalization_source(), "fn finalize_completed_attempt(");
    let signal_idx = balanced
        .find("apply_terminal_signal_outcome")
        .expect("finalize_completed_attempt must consume typed signal");
    let diagnostics_idx = balanced
        .find("balanced_result_error_category")
        .expect("finalize_completed_attempt diagnostics fallback");
    assert!(
        signal_idx < diagnostics_idx,
        "typed signal must precede balanced legacy diagnostics"
    );
}

#[test]
fn age153_marker_emitting_typed_dispositions_mark_guard_after_explicit_finalize() {
    for (function_name, disposition) in [
        ("fn handle_quota_exhausted_retry(", "QuotaExhaustedRetry"),
        ("fn handle_prolonged_silence_fail(", "ProlongedSilenceFail"),
    ] {
        assert_typed_signal_disposition_marks_guard_after_finalize(function_name, disposition);
    }
}

fn assert_typed_signal_disposition_marks_guard_after_finalize(
    function_name: &str,
    disposition: &str,
) {
    assert_disposition_guard_evidence(disposition_guard_evidence(function_name, disposition));
}

fn disposition_guard_evidence<'a>(function_name: &'a str, disposition: &str) -> GuardEvidence<'a> {
    let body = production_block_after(disposition_source(), function_name);
    let disposition_token = disposition_token(disposition);
    let has_disposition = body.contains(&disposition_token);
    let finalize = body.find("finalize_invocation");
    let mark = body.find("guard.mark_finalized()");
    let has_guard_fallback = body.contains("finalize_invocation_from_guard");
    guard_evidence(
        function_name,
        disposition_token,
        has_disposition,
        finalize,
        mark,
        has_guard_fallback,
    )
}

fn disposition_token(disposition: &str) -> String {
    format!("TerminalSignalDisposition::{disposition}")
}

fn assert_disposition_guard_evidence(evidence: GuardEvidence<'_>) {
    assert!(
        evidence.has_disposition,
        "{} must handle {}",
        evidence.function_name, evidence.disposition_token
    );
    let finalize_idx = evidence.finalize.unwrap_or_else(|| {
        panic!(
            "{} must explicitly finalize invocation",
            evidence.disposition_token
        )
    });
    let mark_idx = evidence.mark.unwrap_or_else(|| {
        panic!(
            "{} must mark FinalizerGuard finalized",
            evidence.disposition_token
        )
    });
    assert!(
        finalize_idx < mark_idx,
        "{} must mark the guard only after explicit finalization",
        evidence.disposition_token
    );
    assert!(
        !evidence.has_guard_fallback,
        "{} must not route typed-signal handling through FinalizerGuard::drop",
        evidence.disposition_token
    );
}

struct GuardEvidence<'a> {
    function_name: &'a str,
    disposition_token: String,
    has_disposition: bool,
    finalize: Option<usize>,
    mark: Option<usize>,
    has_guard_fallback: bool,
}

fn guard_evidence<'a>(
    function_name: &'a str,
    disposition_token: String,
    has_disposition: bool,
    finalize: Option<usize>,
    mark: Option<usize>,
    has_guard_fallback: bool,
) -> GuardEvidence<'a> {
    GuardEvidence {
        function_name,
        disposition_token,
        has_disposition,
        finalize,
        mark,
        has_guard_fallback,
    }
}
