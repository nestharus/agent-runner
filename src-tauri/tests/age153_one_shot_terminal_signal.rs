#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, assert_no_terminal_marker_on_stdout, assert_single_terminal_signal,
    legacy_quota_like_non_signal_body, line_count, nonzero_exit_with_non_quota_error_body,
    prolonged_silence_body, quota_body, signal_exit_with_non_quota_error_body, success_body,
    terminal_signal_lines, unknown_with_non_quota_error_body,
};

#[test]
fn one_shot_quota_signal_marks_exhausted_retries_sibling_and_emits_marker() {
    let fixture = Age153Fixture::new();
    let first_marker = fixture.dir.path().join("one-shot-quota-a.txt");
    let sibling_marker = fixture.dir.path().join("one-shot-quota-b.txt");
    fixture.write_model("age153-one-shot", &["claude-age153-a", "claude-age153-b"]);
    fixture.write_providers_with_bodies(&[
        ("claude-age153-a", &quota_body(&first_marker, 42)),
        (
            "claude-age153-b",
            &success_body(&sibling_marker, "typed sibling success"),
        ),
    ]);

    let output = fixture.run_one_shot("age153-one-shot");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "QuotaExhaustedInband", false);
    assert_eq!(fixture.exhausted_row_count("claude-age153-a"), 1);
    assert_eq!(fixture.exhausted_row_count("claude-age153-b"), 0);
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-a", "quota_exhausted_inband"),
        1
    );
    assert_eq!(line_count(&first_marker), 1);
    assert_eq!(line_count(&sibling_marker), 1);
}

#[test]
fn one_shot_prolonged_silence_signal_fails_without_exhausted_write() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("one-shot-prolonged-silence.txt");
    fixture.write_model("age153-one-shot-silence", &["claude-age153-silence"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-silence",
        &prolonged_silence_body(&marker),
    )]);

    let output = fixture.run_one_shot_with_env(
        "age153-one-shot-silence",
        &[("OULIPOLY_TEST_BOUNDED_SILENCE_MS", "120")],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "ProlongedSilence", false);
    assert_eq!(fixture.exhausted_row_count("claude-age153-silence"), 0);
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-silence", "bounded_silence"),
        1
    );
    assert_eq!(line_count(&marker), 1);
}

/// Typed `SignalExit` emits a marker, records the SIGTERM terminal reason, and leaves quota state untouched.
#[test]
fn one_shot_signal_exit_terminal_signal_does_not_write_exhausted_at() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("one-shot-signal-exit.txt");
    fixture.write_model("age153-signal-exit", &["claude-age153-signal"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-signal",
        &signal_exit_with_non_quota_error_body(&marker),
    )]);

    let output = fixture.run_one_shot("age153-signal-exit");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "SignalExit", false);
    assert_eq!(fixture.exhausted_row_count("claude-age153-signal"), 0);
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-signal", "signal:SIGTERM"),
        1
    );
    assert_eq!(line_count(&marker), 1);
}

/// Typed `NonzeroExit` emits a marker, records `exit_nonzero`, and leaves quota state untouched.
#[test]
fn one_shot_nonzero_exit_terminal_signal_does_not_write_exhausted_at() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("one-shot-nonzero-exit.txt");
    fixture.write_model("age153-nonzero-exit", &["claude-age153-nonzero"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-nonzero",
        &nonzero_exit_with_non_quota_error_body(&marker),
    )]);

    let output = fixture.run_one_shot("age153-nonzero-exit");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "NonzeroExit", false);
    assert_eq!(fixture.exhausted_row_count("claude-age153-nonzero"), 0);
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-nonzero", "exit_nonzero"),
        1
    );
    assert_eq!(line_count(&marker), 1);
}

/// Typed `SpawnError` emits a marker, records `spawn_error`, and leaves quota state untouched.
#[test]
fn one_shot_spawn_error_terminal_signal_does_not_write_exhausted_at() {
    let fixture = Age153Fixture::new();
    let missing_command = fixture.dir.path().join("missing-provider-command");
    fixture.write_model("age153-spawn-error", &["claude-age153-spawn"]);
    fixture.write_providers_with_command_paths(&[("claude-age153-spawn", &missing_command)]);

    let output = fixture.run_one_shot("age153-spawn-error");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "SpawnError", false);
    assert_eq!(fixture.exhausted_row_count("claude-age153-spawn"), 0);
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-spawn", "spawn_error"),
        1
    );
}

/// Typed `Unknown` emits a marker, records `unknown_exit`, and leaves quota state untouched.
#[test]
fn one_shot_unknown_terminal_signal_does_not_write_exhausted_at() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("one-shot-unknown.txt");
    fixture.write_model("age153-unknown", &["claude-age153-unknown"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-unknown",
        &unknown_with_non_quota_error_body(&marker),
    )]);

    let output = fixture.run_one_shot_with_env(
        "age153-unknown",
        &[("OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND", "Unknown")],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "Unknown", false);
    assert_eq!(fixture.exhausted_row_count("claude-age153-unknown"), 0);
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-unknown", "unknown_exit"),
        1
    );
    assert_eq!(line_count(&marker), 1);
}

#[test]
fn one_shot_all_providers_exhausted_by_typed_quota_returns_nonzero_with_one_marker_per_attempt() {
    let fixture = Age153Fixture::new();
    let first_marker = fixture.dir.path().join("one-shot-all-a.txt");
    let second_marker = fixture.dir.path().join("one-shot-all-b.txt");
    fixture.write_model(
        "age153-all-exhausted",
        &["claude-age153-a", "claude-age153-b"],
    );
    fixture.write_providers_with_bodies(&[
        ("claude-age153-a", &quota_body(&first_marker, 42)),
        ("claude-age153-b", &quota_body(&second_marker, 43)),
    ]);

    let output = fixture.run_one_shot("age153-all-exhausted");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(terminal_signal_lines(&stderr).len(), 2, "{stderr}");
    assert!(
        stderr.contains("BLOCKED:all-providers-exhausted"),
        "{stderr}"
    );
    assert_eq!(fixture.exhausted_row_count("claude-age153-a"), 1);
    assert_eq!(fixture.exhausted_row_count("claude-age153-b"), 1);
    assert_eq!(line_count(&first_marker), 1);
    assert_eq!(line_count(&second_marker), 1);
}

#[test]
fn one_shot_absent_signal_quota_like_legacy_text_does_not_emit_typed_marker() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("one-shot-absent-signal.txt");
    fixture.write_model("age153-absent-signal", &["claude-age153-absent"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-absent",
        &legacy_quota_like_non_signal_body(&marker),
    )]);

    let output = fixture.run_one_shot_with_env(
        "age153-absent-signal",
        &[("OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_NONE", "1")],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(terminal_signal_lines(&stderr).len(), 0, "{stderr}");
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-absent", "quota_exhausted_inband"),
        0,
        "absent terminal_signal may fall through to legacy diagnostics, but must not take the typed quota signal finalization path"
    );
    assert_eq!(
        fixture.exhausted_row_count("claude-age153-absent"),
        0,
        "provider_quotas.exhausted_at must remain unset unless the typed QuotaExhaustedInband terminal_signal path runs; stderr={stderr}"
    );
    assert_eq!(line_count(&marker), 1);
}
