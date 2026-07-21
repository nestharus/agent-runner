//! ## Declared roles
//!
//! `accessor`, `parser`, `validator`, `formatter`, `orchestration`

#[cfg(test)]
mod tests {
    fn orchestration_source() -> &'static str {
        concat!(
            include_str!("orchestration.rs"),
            "\n",
            include_str!("execution.rs"),
            "\n",
            include_str!("lifecycle.rs"),
            "\n",
            include_str!("migration.rs"),
            "\n",
            include_str!("terminal.rs"),
            "\n",
            include_str!("wake.rs"),
            "\n",
            include_str!("formatter.rs"),
            "\n",
            include_str!("mapper.rs"),
        )
    }

    fn disposition_source() -> &'static str {
        concat!(
            include_str!("disposition.rs"),
            "\n",
            include_str!("formatter.rs"),
            "\n",
            include_str!("mapper.rs"),
        )
    }

    fn finalization_source() -> &'static str {
        concat!(
            include_str!("finalization.rs"),
            "\n",
            include_str!("formatter.rs"),
            "\n",
            include_str!("mapper.rs"),
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
    fn age153_typed_signal_precedence_runs_before_legacy_diagnostics_in_headless_paths() {
        assert_ordered_positions(
            resume_signal_precedence_positions(),
            "run_resume must consume typed signal before finalization",
            "run_resume must keep legacy finalization fallback",
            "typed signal disposition must run before completed-attempt finalization",
        );
        assert_present_position(
            resume_diagnostics_fallback_position(),
            "run_resume diagnostics fallback",
        );
    }

    fn resume_signal_precedence_positions() -> OrderedPositions {
        let run_resume = production_block_after(
            orchestration_source(),
            "fn handle_resume_attempt_terminal_signal(",
        );
        ordered_positions(
            run_resume.find("handle_terminal_signal_disposition"),
            run_resume.find("finalize_completed_attempt"),
        )
    }

    fn resume_diagnostics_fallback_position() -> Option<usize> {
        production_block_after(
            finalization_source(),
            "fn completed_attempt_error_category(",
        )
        .find("resume_result_error_category")
    }

    #[test]
    fn age153_marker_emitting_typed_dispositions_mark_guard_after_explicit_finalize() {
        for (function_name, disposition) in [
            (
                "fn handle_terminal_signal_disposition(",
                "QuotaExhaustedRetry",
            ),
            (
                "fn handle_terminal_signal_disposition(",
                "ProlongedSilenceFail",
            ),
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
        let disposition_idx = body.find(&disposition_token);
        let branch = disposition_idx.map(|idx| disposition_branch_source(&body[idx..]));
        guard_evidence(
            disposition_idx.is_some(),
            branch.and_then(|source| source.find("finalize_invocation")),
            branch.and_then(|source| source.find("guard.mark_finalized()")),
            branch.is_some_and(|source| source.contains("finalize_invocation_from_guard")),
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

    fn disposition_branch_source(source: &str) -> &str {
        if let Some(arrow_idx) = source.find("=>") {
            let after_arrow = &source[arrow_idx + "=>".len()..];
            return match after_arrow.find("TerminalSignalDisposition::") {
                Some(next_idx) => &source[..arrow_idx + "=>".len() + next_idx],
                None => source,
            };
        }
        source
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

    fn assert_present_position(position: Option<usize>, missing_message: &str) {
        assert!(position.is_some(), "{missing_message}");
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
