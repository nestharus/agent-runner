#[cfg(feature = "age319-closed-fresh")]
#[test]
fn closed_fresh_runner_refuses_cli_and_internal_entry_before_state() {
    use std::process::Command;

    let root = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_oulipoly-agent-runner");
    for arguments in [vec!["--help"], vec!["notify", "agent-bash-complete"]] {
        let output = Command::new(binary)
            .args(&arguments)
            .env("OULIPOLY_DATA_DIR", root.path())
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .output()
            .unwrap();
        assert!(!output.status.success(), "{arguments:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("OULIPOLY_AGE319_FRESH_CLOSED"),
            "{arguments:?}: {output:?}"
        );
        assert!(output.stdout.is_empty());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
