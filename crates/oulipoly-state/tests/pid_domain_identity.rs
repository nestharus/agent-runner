#![cfg(target_os = "linux")]

use oulipoly_state::pid_identity::{
    PROCFS_OBSERVER_DOMAIN_ENV, procfs_observer_domain, read_current_process_identity,
    read_direct_child_process_identity, read_live_process_identity, read_parent_process_identity,
    read_retained_direct_child_process_identity, require_declared_procfs_observer_domain,
};
use std::process::{Command, Stdio};

#[test]
fn ordinary_observer_resolves_self_and_child() {
    let self_identity = read_current_process_identity().unwrap();
    assert_eq!(self_identity.os_pid, i64::from(std::process::id()));
    assert_eq!(
        read_live_process_identity(self_identity.os_pid).unwrap(),
        Some(self_identity)
    );
    let mut child = Command::new("sleep")
        .arg("10")
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let identity = read_direct_child_process_identity(child.id()).unwrap();
    assert_eq!(identity.os_pid, i64::from(child.id()));
    assert_eq!(
        read_live_process_identity(identity.os_pid).unwrap(),
        Some(identity.clone())
    );
    child.kill().unwrap();
    let mut terminal = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    assert_eq!(
        unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as libc::id_t,
                terminal.as_mut_ptr(),
                libc::WEXITED | libc::WNOWAIT,
            )
        },
        0
    );
    // The unreaped zombie still has a stat/starttime, but its pinned pidfd
    // reports exit and cannot authorize a new live identity binding.
    assert!(read_direct_child_process_identity(child.id()).is_err());
    assert_eq!(
        read_retained_direct_child_process_identity(child.id()).unwrap(),
        identity
    );
    child.wait().unwrap();
    assert!(read_direct_child_process_identity(child.id()).is_err());
    assert!(read_retained_direct_child_process_identity(child.id()).is_err());
}

#[test]
fn private_host_proc_observer_translates_local_pids() {
    if std::env::var_os("OULIPOLY_PID_DOMAIN_LOCAL_OBSERVER").is_some() {
        assert_eq!(std::process::id(), 1);
        assert_ne!(
            procfs_observer_domain().unwrap(),
            std::env::var("OULIPOLY_PID_DOMAIN_OBSERVER").unwrap()
        );
        return;
    }
    if std::env::var_os("OULIPOLY_PID_DOMAIN_PRIVATE_GRANDCHILD").is_some() {
        let parent = read_parent_process_identity().unwrap().unwrap();
        assert_eq!(
            parent.os_pid.to_string(),
            std::env::var("OULIPOLY_PID_DOMAIN_PARENT").unwrap()
        );
        if std::env::var("OULIPOLY_PID_DOMAIN_EXPECT_MATCH").as_deref() == Ok("1") {
            assert!(require_declared_procfs_observer_domain().is_ok());
        } else {
            assert!(require_declared_procfs_observer_domain().is_err());
        }
        return;
    }
    if std::env::var_os("OULIPOLY_PID_DOMAIN_PRIVATE_CHILD").is_some() {
        assert_eq!(std::process::id(), 1);
        let observer = procfs_observer_domain().unwrap();
        assert_eq!(
            observer,
            std::env::var("OULIPOLY_PID_DOMAIN_OBSERVER").unwrap()
        );
        let self_identity = read_current_process_identity().unwrap();
        assert_ne!(self_identity.os_pid, 1);
        let local_observer = Command::new("unshare")
            .args(["--pid", "--fork", "--mount", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "private_host_proc_observer_translates_local_pids",
                "--nocapture",
            ])
            .env("OULIPOLY_PID_DOMAIN_LOCAL_OBSERVER", "1")
            .env("OULIPOLY_PID_DOMAIN_OBSERVER", &observer)
            .output()
            .unwrap();
        assert!(
            local_observer.status.success(),
            "nested local procfs probe failed: {}",
            String::from_utf8_lossy(&local_observer.stderr)
        );
        for (domain, matches) in [
            (observer.clone(), true),
            ("foreign-procfs-observer".into(), false),
        ] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "private_host_proc_observer_translates_local_pids",
                    "--nocapture",
                ])
                .env("OULIPOLY_PID_DOMAIN_PRIVATE_GRANDCHILD", "1")
                .env(
                    "OULIPOLY_PID_DOMAIN_PARENT",
                    self_identity.os_pid.to_string(),
                )
                .env(PROCFS_OBSERVER_DOMAIN_ENV, domain)
                .env(
                    "OULIPOLY_PID_DOMAIN_EXPECT_MATCH",
                    if matches { "1" } else { "0" },
                )
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "private parent/observer probe failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let mut child = Command::new("sleep")
            .arg("10")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let local_child_pid = child.id();
        let identity = read_direct_child_process_identity(local_child_pid).unwrap();
        assert_ne!(identity.os_pid, i64::from(local_child_pid));
        assert_ne!(
            read_live_process_identity(i64::from(local_child_pid)).unwrap(),
            Some(identity.clone())
        );
        assert_eq!(
            read_live_process_identity(identity.os_pid).unwrap(),
            Some(identity.clone())
        );
        let mut reused = read_live_process_identity(identity.os_pid)
            .unwrap()
            .unwrap();
        reused.os_pid_starttime_ticks += 1;
        assert_ne!(
            read_live_process_identity(reused.os_pid).unwrap(),
            Some(reused)
        );
        let marker_dir = tempfile::tempdir().unwrap();
        let marker = marker_dir.path().join("nested-ready");
        let mut foreign = Command::new("unshare")
            .args(["--pid", "--fork", "--mount", "--mount-proc", "sh", "-c"])
            .arg("touch \"$1\"; sleep 10")
            .arg("nested-probe")
            .arg(&marker)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(marker.exists(), "nested private namespace did not start");
        // The nested process has local PID 1, but it is in a foreign PID
        // namespace and is not the direct child represented by that number.
        assert!(read_direct_child_process_identity(1).is_err());
        foreign.kill().unwrap();
        foreign.wait().unwrap();
        child.kill().unwrap();
        let mut terminal = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        assert_eq!(
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    local_child_pid as libc::id_t,
                    terminal.as_mut_ptr(),
                    libc::WEXITED | libc::WNOWAIT,
                )
            },
            0
        );
        assert!(read_direct_child_process_identity(local_child_pid).is_err());
        assert_eq!(
            read_retained_direct_child_process_identity(local_child_pid).unwrap(),
            identity
        );
        child.wait().unwrap();
        assert!(read_direct_child_process_identity(local_child_pid).is_err());
        return;
    }
    let binary = std::env::current_exe().unwrap();
    let output = Command::new("unshare")
        .args(["--user", "--map-root-user", "--pid", "--fork", "--mount"])
        .arg(binary)
        .args([
            "--exact",
            "private_host_proc_observer_translates_local_pids",
            "--nocapture",
        ])
        .env("OULIPOLY_PID_DOMAIN_PRIVATE_CHILD", "1")
        .env(
            "OULIPOLY_PID_DOMAIN_OBSERVER",
            procfs_observer_domain().unwrap(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "private namespace probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
