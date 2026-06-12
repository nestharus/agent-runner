#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, FORCE_TERMINAL_SIGNAL_KIND, assert_no_terminal_marker_on_stdout, assert_ordered,
    assert_single_terminal_signal, line_count, quota_body, source_block_after, success_body,
    terminal_outcome_adapter_source, terminal_signal_lines,
};

#[test]
fn terminal_signal_marker_is_stderr_key_json_with_four_fields_and_once() {
    let fixture = Age153Fixture::new();
    let first_marker = fixture.dir.path().join("marker-a.txt");
    let sibling_marker = fixture.dir.path().join("marker-b.txt");
    fixture.write_model("age153-marker", &["claude-age153-a", "claude-age153-b"]);
    fixture.write_providers_with_bodies(&[
        ("claude-age153-a", &quota_body(&first_marker, 42)),
        (
            "claude-age153-b",
            &success_body(&sibling_marker, "sibling success"),
        ),
    ]);

    let output = fixture.run_one_shot_with_env(
        "age153-marker",
        &[(FORCE_TERMINAL_SIGNAL_KIND, "QuotaExhaustedInband,None")],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "QuotaExhaustedInband", false);
    assert_eq!(terminal_signal_lines(&stderr).len(), 1, "{stderr}");
    assert_marker_emission_is_adjacent_to_typed_signal_finalization();
    assert_ordered(
        &stderr,
        "OULIPOLY_TERMINAL_SIGNAL=",
        "[routing] provider claude-age153-a returned quota_exhausted; retrying another provider",
    );
    assert_eq!(line_count(&first_marker), 1);
    assert_eq!(line_count(&sibling_marker), 1);
}

fn assert_marker_emission_is_adjacent_to_typed_signal_finalization() {
    let terminal_outcome_adapter = terminal_outcome_adapter_source();
    let helper = source_block_after(
        &terminal_outcome_adapter,
        "fn apply_terminal_signal_outcome(",
    );
    assert!(
        helper.contains("emit_terminal_signal_marker"),
        "typed-signal outcome handling must be the marker emission authority"
    );

    let quota_retry = source_block_after(disposition_source(), "fn handle_quota_exhausted_retry(");
    let signal_idx = quota_retry
        .find("apply_terminal_signal_outcome")
        .expect("handle_quota_exhausted_retry must consume typed terminal signals");
    let after_signal = &quota_retry[signal_idx..];
    let finalize_idx = after_signal
        .find(".finalize_invocation(")
        .expect("typed terminal signal block must include lifecycle finalization");
    let retry_idx = after_signal
        .find("[routing] provider {provider_name} returned quota_exhausted; retrying another provider")
        .expect("quota retry diagnostic must remain in the typed quota retry path");
    assert!(
        finalize_idx < retry_idx,
        "typed marker emission must be tied to lifecycle finalization before retry diagnostics"
    );

    let adjacency_slice = &after_signal[..finalize_idx];
    for forbidden in [
        "balanced_result_error_category",
        "record_returned_artifacts",
        "increment_calls_since_refresh",
        "ingest_and_emit_session_id_resume_aware",
    ] {
        assert!(
            !adjacency_slice.contains(forbidden),
            "typed marker emission must be adjacent to finalization/envelope handling before {forbidden}"
        );
    }

    let completed = source_block_after(finalization_source(), "fn finalize_completed_attempt(");
    let completed_signal_idx = completed
        .find("apply_terminal_signal_outcome")
        .expect("finalize_completed_attempt must consume typed terminal signals");
    let legacy_diagnostics_idx = completed
        .find("balanced_result_error_category")
        .expect("finalize_completed_attempt must retain legacy diagnostics fallback");
    assert!(
        completed_signal_idx < legacy_diagnostics_idx,
        "typed marker emission must precede legacy diagnostics fallback"
    );
}

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
    )
}

fn finalization_source() -> &'static str {
    include_str!("../src/run/balancing/finalization.rs")
}
