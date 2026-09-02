#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, FORCE_TERMINAL_SIGNAL_KIND, assert_no_terminal_marker_on_stdout,
    assert_signal_consumer_source_wired, quota_body, success_body,
};

#[test]
fn authoritative_spooled_success_omits_result_envelope() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-success.txt");
    fixture.write_model("age153-result", &["claude-age153-result"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-result",
        &success_body(&marker, "result-compatible stdout"),
    )]);

    let output = fixture.run_one_shot("age153-result");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "result-compatible stdout\n");
    assert_signal_consumer_source_wired(
        "fn emit_result_envelope_line(",
        &["emit_stdout_marker_line(\"OULIPOLY_RESULT\""],
    );
    assert_signal_consumer_source_wired(
        "fn emit_stdout_marker_line(",
        &[
            "stdout.write_all(marker.as_bytes())",
            "stdout.write_all(b\"=\")",
            "stdout.write_all(json.as_bytes())",
        ],
    );
}

#[test]
fn terminal_signal_marker_stays_on_stderr_when_spooled_success_omits_result() {
    let fixture = Age153Fixture::new();
    let first_marker = fixture.dir.path().join("result-quota-a.txt");
    let sibling_marker = fixture.dir.path().join("result-quota-b.txt");
    fixture.write_model(
        "age153-result-quota",
        &["claude-age153-a", "claude-age153-b"],
    );
    fixture.write_providers_with_bodies(&[
        ("claude-age153-a", &quota_body(&first_marker, 42)),
        (
            "claude-age153-b",
            &success_body(&sibling_marker, "result sibling stdout"),
        ),
    ]);

    let output = fixture.run_one_shot_with_env(
        "age153-result-quota",
        &[(FORCE_TERMINAL_SIGNAL_KIND, "QuotaExhaustedInband,None")],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "result sibling stdout\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OULIPOLY_TERMINAL_SIGNAL="), "{stderr}");
}
