#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, FORCE_TERMINAL_SIGNAL_KIND, assert_no_terminal_marker_on_stdout,
    assert_single_terminal_signal, quota_body, success_body,
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

    let output = fixture.run_resume_with_env(
        "age153-resume",
        &[(FORCE_TERMINAL_SIGNAL_KIND, "QuotaExhaustedInband,None")],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "QuotaExhaustedInband", true);
    assert_eq!(fixture.exhausted_row_count("claude-age153-a"), 1);
    assert_eq!(fixture.exhausted_row_count("claude-age153-b"), 0);
    assert_eq!(fixture.active_segment_provider(), "claude-age153-b");
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-a", "quota_exhausted_inband"),
        1
    );
}
