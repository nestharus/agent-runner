//! A private user/PID/mount namespace demonstrates that broker identity reads
//! stay on the detached procfs mount after the visible /proc is bind replaced.
#![cfg(target_os = "linux")]
use oulipoly_kernel_broker::identity::{PinnedProcess, host_proc_file, install_detached_host_proc};
use std::ffi::CString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::process::Command;

#[test]
fn detached_observer_survives_visible_proc_replacement() {
    if std::env::var_os("AGE319_DETACHED_OBSERVER_INNER").is_none() {
        let output = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "detached_observer_survives_visible_proc_replacement",
                "--nocapture",
            ])
            .env("AGE319_DETACHED_OBSERVER_INNER", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    install_detached_host_proc().unwrap();
    let observer_ns = host_proc_file("self/ns/pid").unwrap();
    let self_pid = std::process::id() as i32;
    let process = PinnedProcess::open(self_pid).unwrap();
    assert!(process.in_namespace(&observer_ns).unwrap());
    let blank = tempfile::tempdir().unwrap();
    let source = CString::new(blank.path().as_os_str().as_encoded_bytes()).unwrap();
    let target = CString::new("/proc").unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        },
        0
    );
    assert!(fs::read_to_string("/proc/self/stat").is_err());
    assert_eq!(
        host_proc_file("self/ns/pid")
            .unwrap()
            .metadata()
            .unwrap()
            .ino(),
        observer_ns.metadata().unwrap().ino()
    );
    process.verify().unwrap();
    PinnedProcess::open(self_pid).unwrap().verify().unwrap();
    assert_eq!(
        unsafe { libc::umount2(target.as_ptr(), libc::MNT_DETACH) },
        0
    );
}
