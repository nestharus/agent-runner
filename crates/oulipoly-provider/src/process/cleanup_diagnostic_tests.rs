//! Actual syscall errors must survive failed settlement, without promoting it.
use super::*;

#[test]
fn cleanup_second_waitid_retains_actual_echild() {
    let raw = Command::new("/bin/true").spawn().unwrap();
    let mut child = Child::new(raw, None);
    child.wait().unwrap();
    let (operation, error) = child.check_signal_group().unwrap_err();
    assert_eq!(operation, "cleanup_waitid_wnowait");
    assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
}

#[test]
fn completed_child_missing_group_preserves_actual_signal_error_and_no_status() {
    // Deliberately no new process group: only this retained Child is owned.
    // Its PID was never a PGID. Never signal the inherited parent group.
    let raw = Command::new("/bin/true").spawn().unwrap();
    let mut child = Child::new(raw, None);
    let command = ProcessCommand::new("/not-logged")
        .arg("describe")
        .arg("secret-argv");
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        let result = poll_child_status(&mut child, &command);
        if !matches!(result, Ok(None)) || Instant::now() >= deadline {
            break result;
        }
        thread::sleep(Duration::from_millis(2));
    };
    // Explicit direct-child reaping after the diagnostic observation, not an
    // assertion that the failed production settlement drained its group.
    child.wait().unwrap();
    let error = result.unwrap_err();
    assert_eq!(error.transport_kind(), "wait_failed");
    assert_eq!(error.process_status(), None);
    assert!(!error.diagnostics().process_was_reaped);
    assert_eq!(
        error.diagnostics().description.as_deref(),
        Some(
            format!(
                "cleanup_group_kill: actor_custody=absent; errno={}",
                libc::ESRCH
            )
            .as_str()
        )
    );
}
