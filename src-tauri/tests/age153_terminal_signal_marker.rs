#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, assert_no_terminal_marker_on_stdout, assert_ordered,
    assert_single_terminal_signal, line_count, main_rs_source, prolonged_silence_body, quota_body,
    source_block_after, success_body, terminal_outcome_adapter_source, terminal_signal_lines,
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
        &[(
            "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND",
            "QuotaExhaustedInband,None",
        )],
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

#[test]
#[ignore = "AGE-163 removed the bounded_silence supervisor; OULIPOLY_TEST_BOUNDED_SILENCE_MS is no longer honored and prolonged_silence_body hangs without a kill path."]
fn typed_signal_handled_path_finalizes_explicitly_not_guard_drop() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("guard-path-prolonged-silence.txt");
    fixture.write_model("age153-guard-path", &["claude-age153-guard"]);
    fixture
        .write_providers_with_bodies(&[("claude-age153-guard", &prolonged_silence_body(&marker))]);

    let output = fixture.run_one_shot_with_env(
        "age153-guard-path",
        &[("OULIPOLY_BOUNDED_SILENCE_MS", "120")],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "ProlongedSilence", false);
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-guard", "bounded_silence"),
        1,
        "typed-signal-handled invocation must keep the executor-produced terminal_reason"
    );
    assert_eq!(
        fixture.invocation_count_with_terminal_reason("guard_drop"),
        0,
        "FinalizerGuard::drop must not be the finalization path after a typed signal is handled"
    );
    assert_eq!(line_count(&marker), 1);
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

    let source = main_rs_source();
    let balanced = source_block_after(&source, "fn run_with_balancing(");
    let signal_idx = balanced
        .find("apply_terminal_signal_outcome")
        .expect("run_with_balancing must consume typed terminal signals");
    let after_signal = &balanced[signal_idx..];
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
}
