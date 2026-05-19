fn main_source() -> &'static str {
    include_str!("../src/main.rs")
}

fn compact(source: &str) -> String {
    source.split_whitespace().collect::<String>()
}

fn assert_not_contains(source: &str, needle: &str, context: &str) {
    assert!(
        !source.contains(needle),
        "{context}: main.rs must not contain `{needle}`"
    );
}

#[test]
fn age151_main_does_not_branch_on_resume_acceptance_evidence_text() {
    assert_not_contains(
        main_source(),
        ".evidence.contains(",
        "resume acceptance evidence-text branching is quarantined to resume_acceptance_adapter",
    );
}

#[test]
fn age151_main_does_not_derive_typed_terminal_signals_from_terminal_reason_strings() {
    let main = compact(main_source());

    for forbidden in [
        "terminal_reason.contains(\"quota\")",
        "terminal_reason.contains(\"Quota\")",
        "terminal_reason.contains(\"prolonged\")",
        "terminal_reason.contains(\"silence\")",
        "terminal_reason.as_deref()==Some(\"quota_exhausted_inband\")",
        "terminal_reason.as_deref()==Some(\"prolonged_silence\")",
        "matchterminal_reason",
    ] {
        assert_not_contains(
            &main,
            forbidden,
            "terminal_reason must remain an operational/finalizer reason, not a typed signal source",
        );
    }

    let terminal_reason_idx = main.find("terminal_reason");
    if let Some(idx) = terminal_reason_idx {
        let rest = &main[idx..];
        assert!(
            !rest.contains("QuotaExhaustedInband") && !rest.contains("ProlongedSilence"),
            "main.rs must not map terminal_reason strings into TerminalSignalKind variants"
        );
    }
}

#[test]
fn age151_main_does_not_emit_terminal_signal_marker() {
    assert_not_contains(
        main_source(),
        "OULIPOLY_TERMINAL_SIGNAL",
        "terminal-signal marker emission is AGE-140C scope",
    );
}

#[test]
fn age151_main_does_not_call_balancer_mark_exhausted() {
    assert_not_contains(
        main_source(),
        "balancer::mark_exhausted",
        "balancer mark_exhausted integration is AGE-140C scope",
    );
    assert_not_contains(
        main_source(),
        ".mark_exhausted(",
        "direct quota mark_exhausted state mutation must not remain in main.rs",
    );
}
