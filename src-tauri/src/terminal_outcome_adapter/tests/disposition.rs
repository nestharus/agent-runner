use super::{
    assert_production_contains, production_block_after, production_source, result_with_signal,
    signal,
};
use crate::terminal_outcome_adapter::{
    TerminalSignalContext, TerminalSignalDisposition, apply_terminal_signal_outcome,
    classify_error_category_with_fallback, confirm_maybe_quota_exhausted,
    terminal_signal_error_category, terminal_signal_reason,
};
use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::TerminalSignal;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_state::StateDb;
use uuid::Uuid;

#[test]
fn age153_apply_terminal_signal_outcome_unit_contract_declares_five_dispositions() {
    assert_production_contains(&["fn ", "apply_terminal_signal_outcome", "("]);
    assert_production_contains(&["enum ", "TerminalSignalDisposition"]);
    for variant in [
        "QuotaExhaustedRetry",
        "ProlongedSilenceFail",
        "InteractiveFail",
        "InteractiveClean",
        "NotApplicable",
    ] {
        assert_production_contains(&["TerminalSignalDisposition::", variant]);
    }
}

#[test]
fn age153_apply_terminal_signal_outcome_maps_quota_to_typed_forensics_write() {
    let source = production_source();
    let outcome = production_block_after("fn apply_terminal_signal_outcome(");
    let disposition = production_block_after("fn terminal_signal_disposition(");
    assert!(
        outcome.contains("terminal_signal_disposition(signal)")
            && outcome.contains("TerminalSignalDisposition::QuotaExhaustedRetry")
            && outcome.contains("apply_typed_post_failure_forensics_quota_retry_side_effects")
            && disposition.contains("TerminalSignalKind::QuotaExhaustedInband")
            && disposition.contains("TerminalSignalDisposition::QuotaExhaustedRetry"),
        "AGE-163 WU-A.4: quota typed signal must route through apply_typed_post_failure_forensics"
    );
    let quota_retry_arm = outcome
        .find("TerminalSignalDisposition::QuotaExhaustedRetry")
        .expect("quota retry arm must exist in apply_terminal_signal_outcome");
    let after_quota_retry = &outcome[quota_retry_arm..];
    let helper =
        production_block_after("fn apply_typed_post_failure_forensics_quota_retry_side_effects<W");
    let forensics_call = helper
        .find("apply_typed_post_failure_forensics")
        .expect("quota retry path must call apply_typed_post_failure_forensics");
    let mark_exhausted = helper
        .find("mark_provider_exhausted")
        .expect("quota retry path must mark exhausted after forensics");
    let interactive_clean = outcome
        .find("TerminalSignalDisposition::InteractiveClean")
        .expect("InteractiveClean disposition must exist");
    assert!(
        forensics_call < mark_exhausted
            && after_quota_retry
                .find("apply_typed_post_failure_forensics_quota_retry_side_effects")
                < Some(interactive_clean),
        "forensics write must stay reachable from the typed-signal quota path"
    );
    assert!(
        source.contains("FailureClass::from_terminal_signal_kind")
            || source.contains("apply_typed_post_failure_forensics"),
        "typed forensics surface must be referenced in the adapter"
    );
    assert!(
        source.contains("terminal_signal") && source.contains("classify_error_category"),
        "typed-signal precedence must coexist with legacy fallback helpers"
    );
}

#[test]
fn age153_emit_terminal_signal_marker_unit_contract_is_key_json_stderr_line() {
    assert_production_contains(&["fn ", "emit_terminal_signal_marker", "("]);
    let payload_helper = production_block_after("fn terminal_signal_marker_payload(");
    let write_helper = production_block_after("fn write_terminal_signal_marker(");
    assert!(
        write_helper.contains("OULIPOLY_TERMINAL_SIGNAL="),
        "{write_helper}"
    );
    assert!(write_helper.contains("serde_json"), "{write_helper}");
    assert!(payload_helper.contains("kind"), "{payload_helper}");
    assert!(payload_helper.contains("evidence"), "{payload_helper}");
    assert!(payload_helper.contains("invocation_id"), "{payload_helper}");
    assert!(payload_helper.contains("session_id"), "{payload_helper}");
    assert!(
        write_helper.contains("writeln!") || write_helper.contains("write_all"),
        "marker helper must write exactly one newline-terminated stderr record"
    );
}

#[test]
fn terminal_outcome_adapter_maybe_quota_verify_disposition_non_durable() {
    let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
    db.upsert_quota_refresh("provider-a", &[]).unwrap();
    let invocation_id = Uuid::nil();
    let maybe = signal(TerminalSignalKind::MaybeQuotaExhausted);
    let mut stderr = Vec::new();
    let mut ctx = TerminalSignalContext {
        invocation_id: &invocation_id,
        session_id: None,
        provider: "provider-a",
        state_db: &db,
        stderr: &mut stderr,
    };

    let disposition = apply_terminal_signal_outcome(&Some(maybe), &mut ctx);
    let category = classify_error_category_with_fallback(
        &result_with_signal(Some(TerminalSignalKind::MaybeQuotaExhausted)),
        || None,
    );

    assert!(matches!(
        disposition,
        TerminalSignalDisposition::MaybeQuotaVerify
    ));
    assert_eq!(category, None);
    assert_eq!(
        db.get_quota("provider-a").unwrap().unwrap().exhausted_at,
        None
    );
    let marker = String::from_utf8(stderr).unwrap();
    assert!(marker.contains("OULIPOLY_TERMINAL_SIGNAL="), "{marker}");
    assert!(
        marker.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "{marker}"
    );
}

struct CancelledTerminalFixture {
    db: StateDb,
    invocation_id: Uuid,
    signal: TerminalSignal,
    stderr: Vec<u8>,
}

fn cancelled_terminal_fixture() -> CancelledTerminalFixture {
    let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
    db.upsert_quota_refresh("provider-a", &[]).unwrap();
    CancelledTerminalFixture {
        db,
        invocation_id: Uuid::nil(),
        signal: cancelled_terminal_signal(),
        stderr: Vec::new(),
    }
}

fn cancelled_terminal_signal() -> TerminalSignal {
    let mut cancelled = signal(TerminalSignalKind::Unknown);
    cancelled.evidence = "terminal-classify-cancelled".to_string();
    cancelled
}

fn apply_cancelled_terminal_outcome(
    fixture: &mut CancelledTerminalFixture,
) -> TerminalSignalDisposition {
    let mut ctx = TerminalSignalContext {
        invocation_id: &fixture.invocation_id,
        session_id: None,
        provider: "provider-a",
        state_db: &fixture.db,
        stderr: &mut fixture.stderr,
    };
    apply_terminal_signal_outcome(&Some(fixture.signal.clone()), &mut ctx)
}

fn cancelled_terminal_reason() -> &'static str {
    terminal_signal_reason(&Some(cancelled_terminal_signal()), Some("cancelled")).expect("reason")
}

fn cancelled_terminal_category(terminal_reason: &str) -> Option<&str> {
    terminal_signal_error_category(&Some(signal(TerminalSignalKind::Unknown)), terminal_reason)
}

#[test]
fn terminal_outcome_adapter_cancelled_reason_is_nondurable_interactive_failure() {
    let mut fixture = cancelled_terminal_fixture();
    let disposition = apply_cancelled_terminal_outcome(&mut fixture);
    let terminal_reason = cancelled_terminal_reason();
    let category = cancelled_terminal_category(terminal_reason);

    assert!(matches!(
        disposition,
        TerminalSignalDisposition::InteractiveFail
    ));
    assert_eq!(terminal_reason, "cancelled");
    assert_eq!(category, Some("cancelled"));
    assert_eq!(db_exhausted_at(&fixture), None);
    let marker = String::from_utf8(fixture.stderr.clone()).unwrap();
    assert!(marker.contains("OULIPOLY_TERMINAL_SIGNAL="), "{marker}");
    assert!(marker.contains("terminal-classify-cancelled"), "{marker}");
}

fn db_exhausted_at(fixture: &CancelledTerminalFixture) -> Option<chrono::DateTime<chrono::Utc>> {
    fixture
        .db
        .get_quota("provider-a")
        .unwrap()
        .unwrap()
        .exhausted_at
}

#[test]
fn terminal_outcome_adapter_confirmed_exhaustion_writes_durable() {
    let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
    db.upsert_quota_refresh("provider-a", &[]).unwrap();
    let invocation_id = Uuid::nil();
    let maybe = signal(TerminalSignalKind::MaybeQuotaExhausted);
    let mut stderr = Vec::new();
    let mut ctx = TerminalSignalContext {
        invocation_id: &invocation_id,
        session_id: None,
        provider: "provider-a",
        state_db: &db,
        stderr: &mut stderr,
    };

    let category = confirm_maybe_quota_exhausted(&maybe, &mut ctx);

    assert_eq!(category, ErrorCategory::QuotaExhausted.as_str());
    assert!(
        db.get_quota("provider-a")
            .unwrap()
            .unwrap()
            .exhausted_at
            .is_some()
    );
    let marker = String::from_utf8(stderr).unwrap();
    assert!(
        marker.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "{marker}"
    );
}

#[test]
fn quota_exhausted_inband_semantics_regression() {
    let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
    db.upsert_quota_refresh("provider-a", &[]).unwrap();
    let invocation_id = Uuid::nil();
    let quota = signal(TerminalSignalKind::QuotaExhaustedInband);
    let mut stderr = Vec::new();
    let mut ctx = TerminalSignalContext {
        invocation_id: &invocation_id,
        session_id: None,
        provider: "provider-a",
        state_db: &db,
        stderr: &mut stderr,
    };

    let disposition = apply_terminal_signal_outcome(&Some(quota.clone()), &mut ctx);
    let category = classify_error_category_with_fallback(
        &result_with_signal(Some(TerminalSignalKind::QuotaExhaustedInband)),
        || Some(ErrorCategory::Unknown.as_str().to_string()),
    );
    let terminal_reason = terminal_signal_reason(&Some(quota.clone()), None);
    let terminal_error_category =
        terminal_signal_error_category(&Some(quota), terminal_reason.unwrap());

    assert!(matches!(
        disposition,
        TerminalSignalDisposition::QuotaExhaustedRetry
    ));
    assert_eq!(
        category.as_deref(),
        Some(ErrorCategory::QuotaExhausted.as_str())
    );
    assert!(
        db.get_quota("provider-a")
            .unwrap()
            .unwrap()
            .exhausted_at
            .is_some()
    );
    assert_eq!(terminal_reason, Some("quota_exhausted_inband"));
    assert_eq!(terminal_error_category, Some("quota_exhausted_inband"));
    let marker = String::from_utf8(stderr).unwrap();
    assert!(
        marker.contains("\"kind\":\"QuotaExhaustedInband\""),
        "{marker}"
    );
}
