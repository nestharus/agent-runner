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
    drop(guard);
    assert_child_reaped(root.path());
    // Grandchildren are reparented to the OS reaper; a zombie is not a live IO
    // worker. The owned direct helper must already be reaped above.
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        let state = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .next()
            .unwrap();
        assert!(
            state == "Z" || state == "X",
            "descendant remains live: {stat}"
        );
    }
    assert!(!root.path().join("descendant-finished").exists());
}
