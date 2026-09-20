//! Real procfs/direct-child signaling regression, in private namespaces.
//! Default: small list. Explicit resource-intensive control:
//! cargo test -p oulipoly-core --test child_enumeration -- --large
//! A single-threaded harness retains exclusive original wait ownership. It never
//! manufactures journals/drain proofs; native AGE360 fixtures cover that layer.
#[cfg(target_os = "linux")]
mod linux {
    use std::{
        fs,
        process::Command,
        time::{Duration, Instant},
    };

    fn observe(pid: i32) -> Option<i32> {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as u32,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        assert_eq!(
            rc,
            0,
            "original wait unavailable for {pid}: {}",
            std::io::Error::last_os_error()
        );
        (unsafe { info.si_pid() } == pid).then(|| unsafe { info.si_status() })
    }

    fn until(mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    fn fork_child(zombie: bool) -> i32 {
        let pid = unsafe { libc::fork() };
        assert!(
            pid >= 0,
            "real child population unavailable: {}",
            std::io::Error::last_os_error()
        );
        if pid == 0 {
            unsafe {
                if zombie {
                    libc::_exit(17);
                }
                // Inherited from the owner before fork; no setup race with TERM.
                loop {
                    libc::pause();
                }
            }
        }
        pid
    }

    fn list() -> Vec<u8> {
        fs::read("/proc/thread-self/children").unwrap()
    }

    fn owner(large: bool, unrelated: i32) {
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
        let mut children = Vec::new();
        loop {
            let child = fork_child(true);
            children.push(child);
            assert!(until(|| observe(child) == Some(17)));
            let bytes = list();
            if (large && bytes.len() > 4096) || (!large && children.len() == 3) {
                break;
            }
        }
        let prefix = list();
        let tail = [fork_child(false), fork_child(false)];
        let full = list();
        println!(
            "real list bytes={} retained_zombies={} tail={tail:?} prefix_last={:?} boundary={:?}",
            full.len(),
            children.len(),
            children.last(),
            full.get(4090..4105)
        );
        assert!(full.starts_with(&prefix));
        for pid in tail {
            assert_eq!(observe(pid), None);
        }
        assert!(oulipoly_core::launch_custody::signal_owned_children(
            libc::SIGTERM
        ));
        for pid in tail {
            assert_eq!(observe(pid), None, "TERM-resistant control");
        }
        // Multiple fresh traversals distinguish a persistent retained prefix
        // from progress caused by prefix reaping. No wait has been consumed.
        for _ in 0..3 {
            assert!(oulipoly_core::launch_custody::signal_owned_children(
                libc::SIGKILL
            ));
        }
        let reached = until(|| tail.iter().all(|pid| observe(*pid) == Some(libc::SIGKILL)));
        assert_eq!(list(), full, "retained membership must remain unchanged");
        for pid in &children {
            assert_eq!(observe(*pid), Some(17));
        }
        assert_eq!(
            unsafe { libc::kill(unrelated, 0) },
            0,
            "unrelated sibling must survive"
        );
        println!(
            "tail_cancelled={reached} unchanged_prefix=true original_WNOWAIT=true unrelated_alive=true"
        );
        assert!(
            reached,
            "live tail beyond retained zombie prefix was not cancelled"
        );
        children.extend(tail);
        for pid in children {
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
            if tail.contains(&pid) {
                assert!(libc::WIFSIGNALED(status));
                assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
            } else {
                assert!(libc::WIFEXITED(status));
                assert_eq!(libc::WEXITSTATUS(status), 17);
            }
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        println!("original_owner_consumed_all_exact_waits_then_ECHILD=true");
    }

    // TERM of an intervening parent adopts a previously non-direct child.
    // The next traversal must find it while retaining the parent's zombie.
    fn adoption() {
        assert_eq!(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) }, 0);
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        let bridge = unsafe { libc::fork() };
        assert!(bridge >= 0);
        if bridge == 0 {
            unsafe {
                libc::close(pipe[0]);
            }
            let descendant = fork_child(false);
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                assert_eq!(
                    libc::write(pipe[1], (&descendant as *const i32).cast(), 4),
                    4
                );
                libc::close(pipe[1]);
                loop {
                    libc::pause();
                }
            }
        }
        unsafe {
            libc::close(pipe[1]);
        }
        let mut descendant: i32 = 0;
        assert_eq!(
            unsafe { libc::read(pipe[0], (&mut descendant as *mut i32).cast(), 4) },
            4
        );
        unsafe {
            libc::close(pipe[0]);
        }
        assert_eq!(String::from_utf8(list()).unwrap(), format!("{bridge} "));
        assert!(oulipoly_core::launch_custody::signal_owned_children(
            libc::SIGTERM
        ));
        assert!(until(|| observe(bridge) == Some(libc::SIGTERM)));
        assert!(until(|| String::from_utf8(list())
            .unwrap()
            .split_whitespace()
            .any(|pid| pid == descendant.to_string())));
        assert_eq!(observe(descendant), None);
        assert!(oulipoly_core::launch_custody::signal_owned_children(
            libc::SIGKILL
        ));
        assert!(until(|| observe(descendant) == Some(libc::SIGKILL)));
        assert_eq!(observe(bridge), Some(libc::SIGTERM));
        for (pid, signal) in [(bridge, libc::SIGTERM), (descendant, libc::SIGKILL)] {
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
            assert!(libc::WIFSIGNALED(status));
            assert_eq!(libc::WTERMSIG(status), signal);
        }
        println!(
            "concurrent_parent_exit_adoption_then_fresh_traversal=true original_waits_retained=true"
        );
    }

    pub fn main() {
        let large = std::env::args().any(|arg| arg == "--large");
        if std::env::var_os("AGE360_ENUM_PRIVATE").is_none() {
            let root =
                std::env::temp_dir().join(format!("age360-enumeration-{}", std::process::id()));
            fs::create_dir(&root).unwrap();
            let result = Command::new("timeout")
                .args([
                    "--kill-after=5s",
                    "120s",
                    "unshare",
                    "--user",
                    "--map-current-user",
                    "--net",
                    "--pid",
                    "--fork",
                    "--mount-proc",
                    "--kill-child=KILL",
                    "--",
                ])
                .arg(std::env::current_exe().unwrap())
                .arg(if large { "--large" } else { "--small" })
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &root)
                .env("TMPDIR", &root)
                .env("AGE360_ENUM_PRIVATE", "1")
                .env(
                    "AGE360_ENUM_PARENT_NET",
                    fs::read_link("/proc/self/ns/net").unwrap(),
                )
                .status()
                .unwrap();
            fs::remove_dir_all(root).unwrap();
            assert!(
                result.success(),
                "private real-child control failed: {result}"
            );
            return;
        }
        assert_eq!(std::process::id(), 1);
        assert_ne!(
            fs::read_link("/proc/self/ns/net").unwrap().as_os_str(),
            std::env::var_os("AGE360_ENUM_PARENT_NET").unwrap()
        );
        let unrelated = fork_child(false);
        let worker = unsafe { libc::fork() };
        assert!(worker >= 0);
        if worker == 0 {
            owner(large, unrelated);
            adoption();
            unsafe {
                libc::_exit(0);
            }
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(worker, &mut status, 0) }, worker);
        assert_eq!(
            observe(unrelated),
            None,
            "unrelated original wait still live"
        );
        unsafe {
            libc::kill(unrelated, libc::SIGKILL);
            libc::waitpid(unrelated, std::ptr::null_mut(), 0);
        }
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "owner control failed: {status}"
        );
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
}
