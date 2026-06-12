use super::result_with_signal;
use crate::terminal_outcome_adapter::classify_error_category_with_fallback;
use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;

#[test]
fn age151_terminal_outcome_typed_quota_exhausted_inband_maps_to_quota_exhausted_before_diagnostics()
{
    let result = result_with_signal(Some(TerminalSignalKind::QuotaExhaustedInband));
    let mut fallback_called = false;

    let category = classify_error_category_with_fallback(&result, || {
        fallback_called = true;
        Some(ErrorCategory::Unknown.as_str().to_string())
    });

    assert_eq!(
        category.as_deref(),
        Some(ErrorCategory::QuotaExhausted.as_str())
    );
    assert!(!fallback_called);
}

#[test]
fn age151_terminal_outcome_typed_prolonged_silence_is_non_quota_terminal_outcome() {
    let result = result_with_signal(Some(TerminalSignalKind::ProlongedSilence));

    let category = classify_error_category_with_fallback(&result, || {
        Some(ErrorCategory::QuotaExhausted.as_str().to_string())
    });

    assert_eq!(
        category.as_deref(),
        Some(ErrorCategory::HungSubprocess.as_str())
    );
}

#[test]
fn age151_terminal_outcome_legacy_diagnostics_runs_only_when_typed_signal_absent() {
    let typed_result = result_with_signal(Some(TerminalSignalKind::QuotaExhaustedInband));
    let legacy_result = result_with_signal(None);
    let mut fallback_calls = 0;

    let typed_category = classify_error_category_with_fallback(&typed_result, || {
        fallback_calls += 1;
        Some(ErrorCategory::Unknown.as_str().to_string())
    });
    let legacy_category = classify_error_category_with_fallback(&legacy_result, || {
        fallback_calls += 1;
        Some(ErrorCategory::QuotaExhausted.as_str().to_string())
    });

    assert_eq!(
        typed_category.as_deref(),
        Some(ErrorCategory::QuotaExhausted.as_str())
    );
    assert_eq!(
        legacy_category.as_deref(),
        Some(ErrorCategory::QuotaExhausted.as_str())
    );
    assert_eq!(fallback_calls, 1);
}

#[test]
fn age151_terminal_outcome_terminal_reason_strings_do_not_create_typed_behavior() {
    let result = result_with_signal(None);

    let category = classify_error_category_with_fallback(&result, || None);

    assert_eq!(
        result.terminal_reason.as_deref(),
        Some("quota_exhausted_inband")
    );
    assert_eq!(category, None);
}
