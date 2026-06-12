//! ## Declared roles
//!
//! `accessor`, `parser`, `validator`, `formatter`, `mapper`, `orchestration`

#[cfg(test)]
mod tests {
    fn disposition_source() -> &'static str {
        concat!(
            include_str!("disposition.rs"),
            "\n",
            include_str!("formatter.rs"),
            "\n",
            include_str!("mapper.rs"),
            "\n",
            include_str!("mapper/attempt.rs"),
            "\n",
            include_str!("mapper/attempt/completed.rs"),
            "\n",
            include_str!("mapper/attempt/disposition.rs"),
            "\n",
            include_str!("mapper/attempt/quota.rs"),
            "\n",
            include_str!("mapper/attempt/shared.rs"),
            "\n",
            include_str!("mapper/attempt/spawn.rs"),
            "\n",
            include_str!("mapper/context.rs"),
            "\n",
            include_str!("mapper/context/config.rs"),
            "\n",
            include_str!("mapper/context/invocation.rs"),
            "\n",
            include_str!("mapper/context/quota.rs"),
            "\n",
            include_str!("mapper/context/routing.rs"),
            "\n",
            include_str!("mapper/context/session.rs"),
            "\n",
            include_str!("mapper/executor_request.rs"),
            "\n",
            include_str!("mapper/failure.rs"),
            "\n",
            include_str!("mapper/finalizer_request.rs"),
            "\n",
            include_str!("mapper/session_ingest.rs"),
            "\n",
            include_str!("mapper/terminal.rs"),
            "\n",
            include_str!("predicate.rs"),
            "\n",
            include_str!("state_update.rs"),
            "\n",
            include_str!("validator.rs"),
        )
    }

    fn finalization_source() -> &'static str {
        concat!(
            include_str!("finalization.rs"),
            "\n",
            include_str!("formatter.rs"),
            "\n",
            include_str!("mapper.rs"),
            "\n",
            include_str!("mapper/attempt.rs"),
            "\n",
            include_str!("mapper/attempt/completed.rs"),
            "\n",
            include_str!("mapper/attempt/disposition.rs"),
            "\n",
            include_str!("mapper/attempt/quota.rs"),
            "\n",
            include_str!("mapper/attempt/shared.rs"),
            "\n",
            include_str!("mapper/attempt/spawn.rs"),
            "\n",
            include_str!("mapper/context.rs"),
            "\n",
            include_str!("mapper/context/config.rs"),
            "\n",
            include_str!("mapper/context/invocation.rs"),
            "\n",
            include_str!("mapper/context/quota.rs"),
            "\n",
            include_str!("mapper/context/routing.rs"),
            "\n",
            include_str!("mapper/context/session.rs"),
            "\n",
            include_str!("mapper/executor_request.rs"),
            "\n",
            include_str!("mapper/failure.rs"),
            "\n",
            include_str!("mapper/finalizer_request.rs"),
            "\n",
            include_str!("mapper/session_ingest.rs"),
            "\n",
            include_str!("mapper/terminal.rs"),
            "\n",
            include_str!("predicate.rs"),
            "\n",
            include_str!("state_update.rs"),
            "\n",
            include_str!("validator.rs"),
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
        let mut in_string = false;
        let mut escaped = false;

        while idx < bytes.len() {
            let byte = bytes[idx];
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
                idx += 1;
                continue;
            }

            match byte {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                return Ok((open_idx + 1, idx));
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
        match result {
            Ok((body_start, body_end)) => &source[body_start..body_end],
            Err(error) => panic!("{}", block_error_message(error, start)),
        }
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
        assert_ordered_positions(
            balanced_signal_precedence_positions(),
            "finalize_completed_attempt must consume typed signal",
            "finalize_completed_attempt diagnostics fallback",
            "typed signal must precede balanced legacy diagnostics",
        );
    }

    fn balanced_signal_precedence_positions() -> OrderedPositions {
        let balanced =
            production_block_after(finalization_source(), "fn finalize_completed_attempt(");
        ordered_positions(
            balanced.find("apply_terminal_signal_outcome"),
            balanced.find("balanced_result_error_category"),
        )
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

    fn disposition_guard_evidence(function_name: &str, disposition: &str) -> GuardEvidence {
        let body = production_block_after(disposition_source(), function_name);
        let disposition_token = disposition_token(disposition);
        guard_evidence(
            body.contains(&disposition_token),
            body.find("finalize_invocation"),
            body.find("guard.mark_finalized()"),
            body.contains("finalize_invocation_from_guard"),
        )
    }

    fn disposition_token(disposition: &str) -> String {
        format!("TerminalSignalDisposition::{disposition}")
    }

    fn assert_disposition_guard_evidence(evidence: GuardEvidence) {
        assert!(evidence.has_disposition, "typed signal disposition missing");
        assert!(evidence.finalize.is_some(), "finalize invocation missing");
        assert!(evidence.mark.is_some(), "guard mark missing");
        assert!(
            evidence.finalize < evidence.mark,
            "guard must be marked only after explicit finalization"
        );
        assert!(
            !evidence.has_guard_fallback,
            "typed signal must not route through FinalizerGuard::drop"
        );
    }

    struct OrderedPositions {
        first: Option<usize>,
        second: Option<usize>,
    }

    fn ordered_positions(first: Option<usize>, second: Option<usize>) -> OrderedPositions {
        OrderedPositions { first, second }
    }

    fn assert_ordered_positions(
        positions: OrderedPositions,
        missing_first_message: &str,
        missing_second_message: &str,
        order_message: &str,
    ) {
        assert!(positions.first.is_some(), "{missing_first_message}");
        assert!(positions.second.is_some(), "{missing_second_message}");
        assert!(positions.first < positions.second, "{order_message}");
    }

    struct GuardEvidence {
        has_disposition: bool,
        finalize: Option<usize>,
        mark: Option<usize>,
        has_guard_fallback: bool,
    }

    fn guard_evidence(
        has_disposition: bool,
        finalize: Option<usize>,
        mark: Option<usize>,
        has_guard_fallback: bool,
    ) -> GuardEvidence {
        GuardEvidence {
            has_disposition,
            finalize,
            mark,
            has_guard_fallback,
        }
    }
}
