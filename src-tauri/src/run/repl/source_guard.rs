//! ## Declared roles
//!
//! `accessor`, `parser`, `validator`, `formatter`, `orchestration`

#[cfg(test)]
mod tests {
    fn orchestration_source() -> &'static str {
        concat!(
            include_str!("orchestration.rs"),
            "\n",
            include_str!("resolution.rs"),
            "\n",
            include_str!("disposition.rs"),
            "\n",
            include_str!("finalization.rs"),
            "\n",
            include_str!("formatter.rs"),
            "\n",
            include_str!("mapper.rs"),
            "\n",
            include_str!("validator.rs"),
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
    fn age153_interactive_signal_precedence_and_clean_no_marker_are_declared() {
        let repl =
            production_block_after(orchestration_source(), "fn finalize_repl_execution_result(");
        assert!(
            repl.contains("InteractiveExecutionResult") || repl.contains("terminal_signal"),
            "interactive finalization must inspect the execution result signal"
        );
        assert!(
            repl.contains("handle_terminal_signal_disposition")
                && repl.contains("finalize_completed_repl_execution"),
            "interactive typed signal handling must precede regular finalization"
        );
        assert!(
            !repl.contains("execute_with_bounded_silence"),
            "run_repl inherited-stdio path must not add bounded-silence supervision"
        );
    }

    #[test]
    fn age153_marker_emitting_typed_dispositions_mark_guard_after_explicit_finalize() {
        assert_typed_signal_disposition_marks_guard_after_finalize(
            "fn handle_terminal_signal_disposition(",
            "InteractiveFail",
        );
    }

    #[test]
    fn run_repl_services_route_through_agent_runtime_services() {
        let source = orchestration_source();
        for required in [
            "agent_runtime_services",
            ".resume_service",
            ".migration_service",
            ".routing_service",
            ".invocation_lifecycle_service",
        ] {
            assert!(
                source.contains(required),
                "run_repl must keep service routing through AgentRuntimeServices: {required}"
            );
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
