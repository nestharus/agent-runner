#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, assert_no_terminal_marker_on_stdout,
    assert_normalized_result_stdout_matches_golden, assert_result_envelope_shape,
    assert_signal_consumer_source_wired, quota_body, success_body,
};

const NORMALIZED_NON_SIGNAL_SUCCESS_GOLDEN: &str = "result-compatible stdout\nOULIPOLY_RESULT={\"error_category\":null,\"exit_code\":0,\"finished_at\":\"<ts>\",\"id\":\"<uuid>\",\"status\":\"succeeded\",\"success\":true,\"terminal_reason\":null}\n";

#[test]
fn result_envelope_stdout_byte_shape_is_preserved_for_non_signal_success() {
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
    assert!(
        stdout.starts_with("result-compatible stdout\nOULIPOLY_RESULT="),
        "{stdout}"
    );
    let envelope = assert_result_envelope_shape(&stdout);
    assert_eq!(envelope["status"], "succeeded");
    assert_eq!(envelope["success"], true);
    assert_eq!(envelope["exit_code"], 0);
    assert_normalized_result_stdout_matches_golden(&stdout, NORMALIZED_NON_SIGNAL_SUCCESS_GOLDEN);
    assert_signal_consumer_source_wired(
        "fn emit_result_envelope_line(",
        &["OULIPOLY_RESULT={json}"],
    );
}

#[test]
fn terminal_signal_marker_never_moves_oulipoly_result_off_stdout() {
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
        &[(
            "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND",
            "QuotaExhaustedInband,None",
        )],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OULIPOLY_RESULT="), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OULIPOLY_TERMINAL_SIGNAL="), "{stderr}");
}
