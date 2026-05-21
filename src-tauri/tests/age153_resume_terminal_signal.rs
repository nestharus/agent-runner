#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, assert_no_terminal_marker_on_stdout, assert_single_terminal_signal,
    prolonged_silence_body, quota_body, success_body,
};

#[test]
fn resume_quota_signal_marks_active_provider_exhausted_migrates_and_emits_marker() {
    let fixture = Age153Fixture::new();
    let first_marker = fixture.dir.path().join("resume-quota-a.txt");
    let sibling_marker = fixture.dir.path().join("resume-quota-b.txt");
    let first_body = quota_body(&first_marker, 42);
    let sibling_body = success_body(&sibling_marker, "resume sibling success");
    fixture.write_resume_pool(
        "age153-resume",
        &[
            ("claude-age153-a", first_body),
            ("claude-age153-b", sibling_body),
        ],
    );
    fixture.stage_active_claude_jsonl("claude-age153-a");
    fixture.seed_active_chain("claude-age153-a", "age153-resume");

    let output = fixture.run_resume("age153-resume");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "QuotaExhaustedInband", true);
    // AGE-163 WU-A.4: durable working-set write moved from `exhausted_at`
    // to `next_available_at` via the typed forensics path.
    assert_eq!(fixture.next_available_at_row_count("claude-age153-a"), 1);
    assert_eq!(fixture.next_available_at_row_count("claude-age153-b"), 0);
    assert_eq!(fixture.active_segment_provider(), "claude-age153-b");
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-a", "quota_exhausted_inband"),
        1
    );
}

#[test]
fn resume_prolonged_silence_signal_fails_without_exhausted_write_or_resume_semantic_change() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("resume-prolonged-silence.txt");
    fixture.write_resume_pool(
        "age153-resume-silence",
        &[("claude-age153-silence", prolonged_silence_body(&marker))],
    );
    fixture.stage_active_claude_jsonl("claude-age153-silence");
    fixture.seed_active_chain("claude-age153-silence", "age153-resume-silence");

    let output = fixture.run_resume_with_env(
        "age153-resume-silence",
        &[("OULIPOLY_BOUNDED_SILENCE_MS", "120")],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "ProlongedSilence", true);
    assert_eq!(fixture.exhausted_row_count("claude-age153-silence"), 0);
    assert_eq!(fixture.active_segment_provider(), "claude-age153-silence");
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-silence", "bounded_silence"),
        1
    );
}
