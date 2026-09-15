//! Correction regressions. All filesystem/process resources are private and bounded.
use super::*;
use std::sync::mpsc;

// The old closure ran on the observer thread and could not be cancelled. Keep
// the same uncooperative channel wait, now in the actual disposable process;
// do not substitute a stop-aware sleep or release it before checking Drop.
#[test]
fn age355_correction_drop_does_not_wait_for_unrelated_inspection() {
    let root = tempfile::tempdir().unwrap();
    let command = private_child(root.path(), "blocked");
    let guard = helper::start_command(command).unwrap();
    wait_for_file(&root.path().join("inspection-started"));
    let (finished, done) = mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(guard);
        finished.send(()).unwrap();
    });
    let returned_without_io = done.recv_timeout(Duration::from_millis(250)).is_ok();
    dropper.join().unwrap();
    assert!(
        returned_without_io,
        "scope exit waited for unrelated inspection IO"
    );
    assert!(
        !root.path().join("inspection-finished").exists(),
        "IO returned instead of being terminated"
    );
    assert_child_reaped(root.path());
}

fn private_child(root: &std::path::Path, mode: &str) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "native_receipt::correction_tests::age355_private_inspection_child",
            "--nocapture",
        ])
        .env("AGE355_PRIVATE_CHILD_ROOT", root)
        .env("AGE355_PRIVATE_CHILD_MODE", mode);
    command
}

fn wait_for_file(path: &std::path::Path) {
    let start = Instant::now();
    while !path.exists() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(path.exists(), "child did not enter inspection");
}

fn assert_child_reaped(root: &std::path::Path) {
    let pid: u32 = std::fs::read_to_string(root.join("inspection-started"))
        .unwrap()
        .parse()
        .unwrap();
    #[cfg(unix)]
    {
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        assert!(
            handle.is_null(),
            "child process still present after scope exit"
        );
    }
}

#[test]
#[ignore = "subprocess fixture only; requires explicit private physical root"]
fn age355_private_inspection_child() {
    use std::io::{Read, Write};
    let root = std::path::PathBuf::from(
        std::env::var_os("AGE355_PRIVATE_CHILD_ROOT").expect("private child root"),
    );
    let mode = std::env::var("AGE355_PRIVATE_CHILD_MODE").unwrap();
    if mode == "descendant" {
        std::fs::write(
            root.join("descendant-started"),
            std::process::id().to_string(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_secs(30));
        std::fs::write(root.join("descendant-finished"), b"escaped").unwrap();
        return;
    }
    if mode == "entry" {
        helper::entry(false).unwrap();
        return;
    }
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte).unwrap();
    assert_eq!(byte, [1]);
    let mut descendant = if mode == "blocked-descendant" {
        let mut command = private_child(&root, "descendant");
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = command.spawn().unwrap();
        wait_for_file(&root.join("descendant-started"));
        Some(child)
    } else {
        None
    };
    std::fs::write(
        root.join("inspection-started"),
        std::process::id().to_string(),
    )
    .unwrap();
    std::io::stdout().flush().unwrap();
    // Identical failure stimulus to predecessor: unrelated synchronous IO
    // cannot observe the parent's stop. Only process termination releases it.
    let (_release, released) = mpsc::channel::<()>();
    let _ = released.recv_timeout(Duration::from_secs(5));
    if let Some(child) = descendant.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    std::fs::write(root.join("inspection-finished"), b"returned").unwrap();
}

#[test]
fn age355_stalled_inspection_is_terminated_and_reaped() {
    let root = tempfile::tempdir().unwrap();
    let mut command = private_child(root.path(), "blocked");
    let start = Instant::now();
    let result = helper::supervise(
        &mut command,
        &CancellationToken::new(),
        Duration::from_millis(150),
    );
    assert!(result.unwrap_err().contains("stalled"));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!root.path().join("inspection-finished").exists());
    assert_child_reaped(root.path());
}

#[test]
fn age355_stop_observed_after_io_skips_the_next_interval() {
    let (entered, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let mut guard = start_receipt_polling_with(
        move || {
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        },
        Duration::from_secs(2),
    )
    .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    guard.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    // No unpark token: model IO consuming the token sent by Drop. The stop
    // flag itself must prevent the post-IO park, not rely on a retained token.
    let worker = guard.worker.take().unwrap();
    let joiner = std::thread::spawn(move || {
        worker.join().unwrap();
        finished.send(()).unwrap();
    });
    release.send(()).unwrap();
    let skipped_interval = done.recv_timeout(Duration::from_secs(1)).is_ok();
    joiner.join().unwrap();
    assert!(skipped_interval, "worker parked again after observing stop");
}

#[cfg(target_os = "linux")]
#[test]
fn age355_scope_cancellation_terminates_contained_descendants() {
    let root = tempfile::tempdir().unwrap();
    let guard = helper::start_command(private_child(root.path(), "blocked-descendant")).unwrap();
    wait_for_file(&root.path().join("inspection-started"));
    let pid = std::fs::read_to_string(root.path().join("descendant-started")).unwrap();
    let before = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let helper = std::fs::read_to_string(root.path().join("inspection-started")).unwrap();
    let fields: Vec<_> = before
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    assert_eq!(
        fields[2], helper,
        "descendant not in the owned helper group"
    );
    eprintln!("receipt cancellation before: helper={helper} descendant={before}");
    drop(guard);
    assert_child_reaped(root.path());
    // Grandchildren are reparented to the OS reaper; a zombie is not a live IO
    // worker. The owned direct helper must already be reaped above.
    assert_descendant_terminal(
        &std::path::PathBuf::from(format!("/proc/{pid}/stat")),
        fields[19],
    );
    assert!(!root.path().join("descendant-finished").exists());
}

// Oracle-only: a read failure is not a terminal-state observation. The caller
// established the original descendant identity before cancellation.
#[cfg(target_os = "linux")]
fn assert_descendant_terminal(path: &std::path::Path, starttime: &str) {
    match std::fs::read_to_string(path) {
        Ok(stat) => {
            let state = stat
                .rsplit_once(')')
                .unwrap()
                .1
                .split_whitespace()
                .next()
                .unwrap();
            assert_eq!(
                stat.rsplit_once(')').unwrap().1.split_whitespace().nth(19),
                Some(starttime),
                "observation no longer describes the original descendant"
            );
            eprintln!("receipt cancellation after guard joined: {stat}");
            assert!(
                state == "Z" || state == "X",
                "descendant remains live: {stat}"
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "receipt descendant independently absent: {}: {error}",
                path.display()
            );
        }
        Err(error) => panic!(
            "descendant observation unavailable: {}: {error}",
            path.display()
        ),
    }
}

#[cfg(target_os = "linux")]
#[test]
fn receipt_descendant_oracle_rejects_unreadable_live_and_wrong_identity() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("stat");
    let record = |state| format!("1 (oracle fixture) {state} 0 7 {} 123", ["0"; 16].join(" "));
    for state in ["R", "S", "D"] {
        std::fs::write(&path, record(state)).unwrap();
        assert!(std::panic::catch_unwind(|| assert_descendant_terminal(&path, "123")).is_err());
    }
    std::fs::write(&path, record("Z")).unwrap();
    assert!(std::panic::catch_unwind(|| assert_descendant_terminal(&path, "456")).is_err());
    assert_descendant_terminal(&path, "123");
    std::fs::write(&path, b"invalid encoding \xff").unwrap();
    assert!(std::panic::catch_unwind(|| assert_descendant_terminal(&path, "123")).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0)).unwrap();
    let read = std::fs::read_to_string(&path).unwrap_err();
    assert_eq!(
        read.kind(),
        std::io::ErrorKind::PermissionDenied,
        "negative requires real DAC denial"
    );
    assert!(std::panic::catch_unwind(|| assert_descendant_terminal(&path, "123")).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_descendant_terminal(&path, "123");
    // Other I/O errors (a directory rather than a record) are not absence.
    assert!(std::panic::catch_unwind(|| assert_descendant_terminal(root.path(), "123")).is_err());
    eprintln!(
        "strict oracle: live/wrong identity/InvalidData/PermissionDenied/other I/O rejected; terminal and NotFound accepted; synthetic records are oracle-only, not custody evidence"
    );
}

#[cfg(unix)]
#[test]
fn age355_idle_helper_releases_rebuild_custody_without_splitting_owner() {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let state_path = root.path().join("state.db");
    drop(oulipoly_state::StateDb::open(&state_path).unwrap());
    let path = MailboxDb::path_for_state_db(&state_path);
    let db = MailboxDb::open(&path).unwrap();
    let generation = db.sidecar_generation().unwrap();
    drop(db);
    let mut command = private_child(root.path(), "entry");
    command.env("OULIPOLY_DATA_DIR", root.path());
    let guard = helper::start_command(command).unwrap();
    let owner_path = path.with_extension("receipt-owner");
    wait_for_file(&owner_path);
    let started = Instant::now();
    while std::fs::read(&owner_path).unwrap().is_empty() {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    let state_authority = oulipoly_state::StateDb::acquire_rebuild_authority(&state_path).unwrap();
    let mut rebuild = MailboxDb::acquire_rebuild_authority(&state_authority).unwrap();
    assert!(helper::try_admit(&path, "receipt-owner").unwrap().is_none());
    rebuild.reset().unwrap();
    rebuild.initialize_after_rebuild().unwrap();
    assert!(helper::try_admit(&path, "receipt-owner").unwrap().is_none());
    drop(rebuild);
    drop(state_authority);
    let reopened = MailboxDb::open(&path).unwrap();
    assert_ne!(generation, reopened.sidecar_generation().unwrap());
    drop(reopened);
    let before = std::fs::read_to_string(&owner_path).unwrap();
    let start = Instant::now();
    loop {
        let mut stamp = String::new();
        std::fs::File::open(&owner_path)
            .unwrap()
            .read_to_string(&mut stamp)
            .unwrap();
        if !stamp.is_empty() && stamp != before {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(guard);
    assert!(helper::try_admit(&path, "receipt-owner").unwrap().is_some());
}

#[test]
fn age355_target_anchor_binding_is_framed_and_bounded_for_large_tokens() {
    let mut anchor = MailboxDeliveryObservationAnchor {
        provider_name: "account".into(),
        provider_instance_id: "instance".into(),
        settings_id: "settings".into(),
        provider_session_id: "session".into(),
        resume_token: Some("x".repeat(256 * 1024)),
        expected_sha256: "digest".into(),
    };
    let original = helper::anchor_identity(&anchor);
    let target = helper::Target {
        admission_purpose: helper::AdmissionPurpose::TerminalBounded,
        attempt_id: "nonce".into(),
        anchor_identity: original.clone(),
        model_name: "model".into(),
        cwd: "/private".into(),
        config_root: "/private/config".into(),
    };
    assert!(serde_json::to_vec(&target).unwrap().len() < 512);
    anchor.resume_token.as_mut().unwrap().push('y');
    assert_ne!(helper::anchor_identity(&anchor), original);
    anchor.resume_token = None;
    let absent = helper::anchor_identity(&anchor);
    anchor.resume_token = Some(String::new());
    assert_ne!(helper::anchor_identity(&anchor), absent);
    anchor.provider_name = "ab".into();
    anchor.provider_instance_id = "c".into();
    let framed = helper::anchor_identity(&anchor);
    anchor.provider_name = "a".into();
    anchor.provider_instance_id = "bc".into();
    assert_ne!(helper::anchor_identity(&anchor), framed);
}

#[test]
fn receipt_cwd_recovery_requires_exact_persisted_authority() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.db");
    let state = oulipoly_state::StateDb::open(&path).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
         VALUES ('chain', '2026-09-11', '2026-09-11', 'fixture');
         INSERT INTO session_chain_segments
             (chain_id, provider_name, session_id, started_at, transition_reason)
         VALUES ('chain', 'account', 'session', '2026-09-11', 'initial');
         INSERT INTO session_chain_segment_provider_authority
             (segment_id, provider_instance_id, settings_id)
         SELECT id, 'instance', 'settings' FROM session_chain_segments;",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO imported_session_display_metadata
             (provider_name, provider_session_id, cwd, first_seen_at, last_seen_at)
         VALUES ('account', 'session', ?1, '2026-09-11', '2026-09-11')",
            [root.path().to_str().unwrap()],
        )
        .unwrap();
    let mut anchor = MailboxDeliveryObservationAnchor {
        provider_name: "account".into(),
        provider_instance_id: "instance".into(),
        settings_id: "settings".into(),
        provider_session_id: "session".into(),
        resume_token: Some("anchor".into()),
        expected_sha256: "digest".into(),
    };
    assert_eq!(
        recover_observation_cwd(&state, &anchor).unwrap(),
        Some(root.path().into())
    );
    for field in 0..4 {
        let slot = match field {
            0 => &mut anchor.provider_name,
            1 => &mut anchor.provider_instance_id,
            2 => &mut anchor.settings_id,
            _ => &mut anchor.provider_session_id,
        };
        let original = std::mem::replace(slot, "mismatch".into());
        assert_eq!(recover_observation_cwd(&state, &anchor).unwrap(), None);
        match field {
            0 => anchor.provider_name = original,
            1 => anchor.provider_instance_id = original,
            2 => anchor.settings_id = original,
            _ => anchor.provider_session_id = original,
        }
    }
    connection
        .execute(
            "UPDATE imported_session_display_metadata SET cwd = 'relative'",
            [],
        )
        .unwrap();
    assert!(
        recover_observation_cwd(&state, &anchor)
            .unwrap_err()
            .contains("absolute")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn receipt_cancellation_does_not_terminate_another_owned_group() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let guard = helper::start_command(private_child(first.path(), "blocked-descendant")).unwrap();
    let other = helper::start_command(private_child(second.path(), "blocked-descendant")).unwrap();
    wait_for_file(&first.path().join("inspection-started"));
    wait_for_file(&second.path().join("inspection-started"));
    let before = std::fs::read_to_string(second.path().join("descendant-started")).unwrap();
    drop(guard);
    assert_child_reaped(first.path());
    let stat = std::fs::read_to_string(format!("/proc/{before}/stat")).unwrap();
    let state = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .next()
        .unwrap();
    assert!(
        !matches!(state, "Z" | "X"),
        "unrelated owned group was killed: {stat}"
    );
    eprintln!("other exact group still live after first scope joined: {stat}");
    drop(other);
    assert_child_reaped(second.path());
}

#[cfg(target_os = "linux")]
#[test]
fn receipt_group_cleanup_rejects_an_unowned_group() {
    let root = tempfile::tempdir().unwrap();
    let mut child = private_child(root.path(), "descendant")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_for_file(&root.path().join("descendant-started"));
    let result = oulipoly_provider::client::settle_receipt_inspection_group(&child);
    let still_live = child.try_wait().unwrap().is_none();
    // This test retains the direct child and cleans it even if the oracle fails.
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(result.unwrap_err().to_string().contains("owned group"));
    assert!(still_live, "observer signalled a group it did not own");
    let lost_wait = oulipoly_provider::client::settle_receipt_inspection_group(&child).unwrap_err();
    assert_eq!(lost_wait.raw_os_error(), Some(libc::ECHILD));
}
