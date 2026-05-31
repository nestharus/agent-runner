#[path = "age240_relocated_support.rs"]
mod support;

#[test]
fn age38_test_model_success_routes_effective_request_through_stub_ports() {
    support::age38_test_model_success_routes_effective_request_through_stub_ports();
}

#[test]
fn tauri_test_model_injects_policy() {
    support::tauri_test_model_injects_policy();
}

#[test]
fn age38_test_model_nonzero_not_exhausted_classifies_without_marking_quota() {
    support::age38_test_model_nonzero_not_exhausted_classifies_without_marking_quota();
}

#[test]
fn age38_test_model_nonzero_exhausted_classifies_and_marks_quota() {
    support::age38_test_model_nonzero_exhausted_classifies_and_marks_quota();
}

#[test]
fn age156_test_model_typed_rate_limited_signal_does_not_mark_exhausted_even_when_legacy_classifier_would()
 {
    support::age156_test_model_typed_rate_limited_signal_does_not_mark_exhausted_even_when_legacy_classifier_would();
}

#[test]
fn test_model_maybe_signal_is_non_durable() {
    support::test_model_maybe_signal_is_non_durable();
}

#[test]
fn age156_test_model_typed_quota_exhausted_inband_signal_marks_exhausted_even_when_legacy_classifier_would_not()
 {
    support::age156_test_model_typed_quota_exhausted_inband_signal_marks_exhausted_even_when_legacy_classifier_would_not();
}

#[test]
fn test_model_nonzero_stdout_exhausted_classifies_and_marks_quota() {
    support::test_model_nonzero_stdout_exhausted_classifies_and_marks_quota();
}

#[cfg(unix)]
#[test]
fn test_model_marks_provider_exhausted_on_quota_stderr() {
    support::test_model_marks_provider_exhausted_on_quota_stderr();
}

#[cfg(unix)]
#[test]
fn test_model_raw_sigterm_returns_unified_signal_exit_code() {
    support::test_model_raw_sigterm_returns_unified_signal_exit_code();
}
