fn disposition_source() -> &'static str {
    concat!(
        include_str!("../src/run/balancing/disposition.rs"),
        "\n",
        include_str!("../src/run/balancing/disposition/control.rs"),
        "\n",
        include_str!("../src/run/balancing/disposition/failure.rs"),
        "\n",
        include_str!("../src/run/balancing/disposition/input.rs"),
        "\n",
        include_str!("../src/run/balancing/disposition/maybe_quota.rs"),
        "\n",
        include_str!("../src/run/balancing/disposition/quota.rs"),
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
    production_block_from_parse_result(source, start, production_block_bounds(source, start))
}

fn production_block_bounds(source: &str, start: &str) -> Result<(usize, usize), BlockError> {
    let start_idx = source.find(start).ok_or(BlockError::Start)?;
    let open_idx = source[start_idx..]
        .find('{')
        .map(|idx| start_idx + idx)
        .ok_or(BlockError::OpeningBrace)?;
    let mut depth = 1usize;
    let mut idx = open_idx + 1;
    let bytes = source.as_bytes();

    while idx < bytes.len() {
        match bytes[idx] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((open_idx + 1, idx));
                }
            }
            _ => {}
        }
        idx += 1;
    }

    Err(BlockError::ClosingBrace)
}

fn production_block_from_parse_result<'a>(
    source: &'a str,
    start: &str,
    result: Result<(usize, usize), BlockError>,
) -> &'a str {
    production_block_slice(source, validated_block_bounds(start, result))
}

fn validated_block_bounds(
    start: &str,
    result: Result<(usize, usize), BlockError>,
) -> (usize, usize) {
    result.unwrap_or_else(|error| panic!("{}", block_error_message(error, start)))
}

fn production_block_slice(source: &str, bounds: (usize, usize)) -> &str {
    &source[bounds.0..bounds.1]
}

enum BlockError {
    Start,
    OpeningBrace,
    ClosingBrace,
}

fn block_error_message(error: BlockError, start: &str) -> String {
    match error {
        BlockError::Start => format!("missing {start}"),
        BlockError::OpeningBrace => format!("missing opening brace after {start}"),
        BlockError::ClosingBrace => format!("missing closing brace after {start}"),
    }
}

#[test]
fn age153_typed_signal_precedence_runs_before_legacy_diagnostics_in_balancing_path() {
    let balanced = production_block_after(finalization_source(), "fn finalize_completed_attempt(");
    assert_typed_signal_precedes_diagnostics(typed_signal_precedence_positions(balanced));
}

fn typed_signal_precedence_positions(body: &str) -> (Option<usize>, Option<usize>) {
    (
        body.find("apply_terminal_signal_outcome"),
        body.find("balanced_result_error_category"),
    )
}

fn assert_typed_signal_precedes_diagnostics(positions: (Option<usize>, Option<usize>)) {
    let signal_idx = positions
        .0
        .expect("finalize_completed_attempt must consume typed signal");
    let diagnostics_idx = positions
        .1
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
    guard_evidence_from_facts(disposition_guard_facts(function_name, disposition))
}

fn disposition_guard_facts<'a>(function_name: &'a str, disposition: &str) -> GuardFacts<'a> {
    let body = production_block_after(disposition_source(), function_name);
    let disposition_token = disposition_token(disposition);
    let has_disposition = has_disposition_token(body, &disposition_token);
    let finalize = finalize_position(body);
    let mark = guard_mark_position(body);
    let has_guard_fallback = has_guard_fallback(body);
    guard_facts(
        function_name,
        disposition_token,
        has_disposition,
        finalize,
        mark,
        has_guard_fallback,
    )
}

fn has_disposition_token(body: &str, disposition_token: &str) -> bool {
    body.contains(disposition_token)
}

fn finalize_position(body: &str) -> Option<usize> {
    body.find("finalize_invocation")
}

fn guard_mark_position(body: &str) -> Option<usize> {
    body.find("guard.mark_finalized()")
}

fn has_guard_fallback(body: &str) -> bool {
    body.contains("finalize_invocation_from_guard")
}

fn disposition_token(disposition: &str) -> String {
    format!("TerminalSignalDisposition::{disposition}")
}

fn guard_evidence_from_facts(facts: GuardFacts<'_>) -> GuardEvidence<'_> {
    guard_evidence(
        facts.function_name,
        facts.disposition_token,
        facts.has_disposition,
        facts.finalize,
        facts.mark,
        facts.has_guard_fallback,
    )
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

struct GuardFacts<'a> {
    function_name: &'a str,
    disposition_token: String,
    has_disposition: bool,
    finalize: Option<usize>,
    mark: Option<usize>,
    has_guard_fallback: bool,
}

fn guard_facts<'a>(
    function_name: &'a str,
    disposition_token: String,
    has_disposition: bool,
    finalize: Option<usize>,
    mark: Option<usize>,
    has_guard_fallback: bool,
) -> GuardFacts<'a> {
    GuardFacts {
        function_name,
        disposition_token,
        has_disposition,
        finalize,
        mark,
        has_guard_fallback,
    }
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
