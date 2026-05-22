#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, FORCE_TERMINAL_SIGNAL_KIND, assert_no_terminal_marker_on_stdout,
    parse_valid_invocations, terminal_signal_lines,
};
use oulipoly_state::{CompositeInvocationId, InvocationStatus};

#[test]
fn captured_child_supervision_uses_typed_quota_parent_reason() {
    let child_uuid = "66666666-1530-4530-8530-666666666666";
    let child_marker = CompositeInvocationId {
        source: "fixture-child".to_string(),
        id: child_uuid.to_string(),
    }
    .stderr_line();
    let fixture = Age153Fixture::new();
    fixture.write_model("age153-captured", &["claude-age153-parent"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-parent",
        &format!(
            r#"printf '%s\n' "{child_marker}" >&2
printf '%s\n' 'claude usage limit reached' >&2
exit 42"#
        ),
    )]);
    fixture.seed_running_child_for_first_parent(child_uuid);

    let output = fixture.run_one_shot_with_env(
        "age153-captured",
        &[(FORCE_TERMINAL_SIGNAL_KIND, "QuotaExhaustedInband")],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !terminal_signal_lines(&stderr).is_empty(),
        "typed parent signal must be externally observable:\n{stderr}"
    );
    assert!(
        parse_valid_invocations(&stderr)
            .iter()
            .any(|invocation| invocation.id == child_uuid),
        "child marker should be present in captured stderr: {stderr}"
    );
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(child_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(child_row.status, InvocationStatus::Failed);
    assert_eq!(child_row.success, Some(false));
    assert_eq!(
        child_row.terminal_reason.as_deref(),
        Some("supervisor_observed_quota_exhausted_inband")
    );
}

#[test]
fn captured_child_supervision_preserves_clean_signal_propagation() {
    let child_uuid = "88888888-1530-4530-8530-888888888888";
    let child_marker = CompositeInvocationId {
        source: "fixture-child".to_string(),
        id: child_uuid.to_string(),
    }
    .stderr_line();
    let fixture = Age153Fixture::new();
    fixture.write_model("age153-captured-clean", &["claude-age153-parent"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-parent",
        &format!(
            r#"printf '%s\n' "{child_marker}" >&2
printf '%s\n' 'clean parent success'"#
        ),
    )]);
    fixture.seed_running_child_for_first_parent(child_uuid);

    let output = fixture.run_one_shot("age153-captured-clean");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        terminal_signal_lines(&stderr).is_empty(),
        "clean parent must not synthesize a typed terminal-signal marker:\n{stderr}"
    );
    assert!(
        parse_valid_invocations(&stderr)
            .iter()
            .any(|invocation| invocation.id == child_uuid),
        "child marker should be present in captured stderr: {stderr}"
    );
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(child_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(child_row.status, InvocationStatus::Failed);
    assert_eq!(child_row.success, Some(false));
    assert_eq!(
        child_row.terminal_reason.as_deref(),
        Some("supervisor_observed_unknown_exit")
    );
    assert_ne!(
        child_row.terminal_reason.as_deref(),
        Some("supervisor_observed_quota_exhausted_inband")
    );
    assert_ne!(
        child_row.terminal_reason.as_deref(),
        Some("supervisor_observed_prolonged_silence")
    );
}
