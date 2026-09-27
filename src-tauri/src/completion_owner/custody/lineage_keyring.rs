//! Per-worker negative standalone-admission lineage for Linux descendants.
//! This is NOT an original-work grant. Only the worker changes its own keyring;
//! neither the guardian nor an unrelated launch gains the marker.
use std::ffi::{CStr, CString};
use std::io;

pub(super) const PREFIX: &str = "oulipoly-paired-original-work-v1:";
// linux/keyctl.h. libc exposes SYS_keyctl but not these UAPI command numbers.
const GET_KEYRING_ID: libc::c_long = 0;
const JOIN_SESSION_KEYRING: libc::c_long = 1;
const DESCRIBE: libc::c_long = 6;
const LINK: libc::c_long = 8;
#[cfg(test)]
const UNLINK: libc::c_long = 9;
#[cfg(test)]
const SEARCH: libc::c_long = 10;
const PROCESS_KEYRING: libc::c_long = -2;
const SESSION_KEYRING: libc::c_long = -3;

fn keyctl(
    command: libc::c_long,
    a: libc::c_long,
    b: libc::c_long,
    c: libc::c_long,
) -> Result<libc::c_long, String> {
    let result = unsafe { libc::syscall(libc::SYS_keyctl, command, a, b, c, 0 as libc::c_long) };
    if result < 0 {
        Err(format!(
            "keyctl({command}) failed: {}",
            io::Error::last_os_error()
        ))
    } else {
        Ok(result)
    }
}

// Command::pre_exec runs after fork in a multithreaded guardian. Keep this
// path to stack data and syscalls; all allocation (including the random name)
// happens in the parent before spawn.
fn keyctl_pre_exec(
    command: libc::c_long,
    a: libc::c_long,
    b: libc::c_long,
    c: libc::c_long,
) -> io::Result<libc::c_long> {
    let result = unsafe { libc::syscall(libc::SYS_keyctl, command, a, b, c, 0 as libc::c_long) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

fn pre_exec_ring_id(which: libc::c_long, create: bool) -> io::Result<libc::c_long> {
    let serial = keyctl_pre_exec(GET_KEYRING_ID, which, create as libc::c_long, 0)?;
    if serial == 0 || serial > i32::MAX as libc::c_long {
        return Err(io::Error::from_raw_os_error(libc::EPROTO));
    }
    Ok(serial)
}

fn decimal(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || (bytes.len() > 1 && bytes[0] == b'0') {
        return None;
    }
    bytes.iter().try_fold(0_u32, |number, byte| {
        if !byte.is_ascii_digit() {
            return None;
        }
        number.checked_mul(10)?.checked_add((byte - b'0') as u32)
    })
}

fn validate_pre_exec_ring(serial: libc::c_long, name: &CStr) -> io::Result<()> {
    if pre_exec_ring_id(SESSION_KEYRING, false)? != serial {
        return Err(io::Error::from_raw_os_error(libc::EPROTO));
    }
    let mut description = [0_u8; 256];
    let length = keyctl_pre_exec(
        DESCRIBE,
        serial,
        description.as_mut_ptr() as libc::c_long,
        description.len() as libc::c_long,
    )?;
    if length <= 0 || length as usize > description.len() || description[length as usize - 1] != 0 {
        return Err(io::Error::from_raw_os_error(libc::EPROTO));
    }
    let mut fields = description[..length as usize - 1].split(|byte| *byte == b';');
    let valid = fields.next() == Some(b"keyring".as_slice())
        && fields.next().and_then(decimal) == Some(unsafe { libc::geteuid() })
        && fields.next().and_then(decimal).is_some()
        && fields.next().is_some_and(|permissions| {
            permissions.len() == 8 && permissions.iter().all(u8::is_ascii_hexdigit)
        })
        && fields.next() == Some(name.to_bytes())
        && fields.next().is_none();
    if !valid {
        return Err(io::Error::from_raw_os_error(libc::EPROTO));
    }
    Ok(())
}

/// Called only from the accepted Bash worker's pre_exec closure. A failure
/// aborts exec, so no workload can inherit an unmarked session ring.
pub(super) fn establish_pre_exec(name: &CStr) -> io::Result<()> {
    let old = pre_exec_ring_id(SESSION_KEYRING, false)?;
    let process = pre_exec_ring_id(PROCESS_KEYRING, true)?;
    keyctl_pre_exec(LINK, old, process, 0)?;
    let new = keyctl_pre_exec(JOIN_SESSION_KEYRING, name.as_ptr() as libc::c_long, 0, 0)?;
    if new == old || new == 0 || new > i32::MAX as libc::c_long {
        return Err(io::Error::from_raw_os_error(libc::EPROTO));
    }
    validate_pre_exec_ring(new, name)?;
    keyctl_pre_exec(LINK, old, new, 0)?;
    validate_pre_exec_ring(new, name)
}

pub(super) fn random_name() -> Result<CString, String> {
    let mut random = [0_u8; 16];
    let mut filled = 0;
    while filled < random.len() {
        let received = unsafe {
            libc::getrandom(
                random[filled..].as_mut_ptr().cast(),
                random.len() - filled,
                0,
            )
        };
        if received < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("lineage UUID entropy unavailable: {error}"));
        }
        if received == 0 {
            return Err("lineage UUID entropy source returned EOF".into());
        }
        filled += received as usize;
    }
    CString::new(format!(
        "{PREFIX}{}",
        uuid::Builder::from_random_bytes(random).into_uuid()
    ))
    .map_err(|_| "lineage name contains NUL".into())
}

fn ring_id(which: libc::c_long, create: bool) -> Result<i32, String> {
    i32::try_from(keyctl(GET_KEYRING_ID, which, create as libc::c_long, 0)?)
        .map_err(|_| "keyring serial exceeds i32".into())
}

fn describe(serial: i32) -> Result<String, String> {
    let mut bytes = [0_u8; 256];
    let length = usize::try_from(keyctl(
        DESCRIBE,
        serial.into(),
        bytes.as_mut_ptr() as libc::c_long,
        bytes.len() as libc::c_long,
    )?)
    .map_err(|_| "keyring description length is invalid")?;
    if length == 0 || length > bytes.len() || bytes[length - 1] != 0 {
        return Err("keyring description is truncated or not terminated".into());
    }
    CStr::from_bytes_with_nul(&bytes[..length])
        .map_err(|_| "keyring description has embedded NUL")?
        .to_str()
        .map(str::to_owned)
        .map_err(|_| "keyring description is not UTF-8".into())
}

fn validate_new_ring(serial: i32, expected: &str) -> Result<(), String> {
    if ring_id(SESSION_KEYRING, false)? != serial {
        return Err("joined session keyring identity changed".into());
    }
    let description = describe(serial)?;
    let mut fields = description.split(';');
    let (Some("keyring"), Some(uid), Some(gid), Some(permissions), Some(name), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err("joined session keyring has invalid description".into());
    };
    if uid.parse::<u32>().ok() != Some(unsafe { libc::geteuid() })
        || gid.parse::<i64>().is_err()
        || permissions.len() != 8
        || !permissions.bytes().all(|digit| digit.is_ascii_hexdigit())
        || name != expected
    {
        return Err("joined session keyring has unexpected identity".into());
    }
    Ok(())
}

/// The original ring must be anchored BEFORE switching: once switched, linking
/// the now-unpossessed original directly into the new ring returns EACCES.
/// Keep the temporary process-ring link for the worker's lifetime (a forked
/// provider does not inherit its process keyring); the new session-ring link
/// carries the original search path into all descendants.
pub(super) fn establish() -> Result<String, String> {
    let old = ring_id(SESSION_KEYRING, false)?;
    let process = ring_id(PROCESS_KEYRING, true)?;
    keyctl(LINK, old.into(), process.into(), 0)?;
    // An unavailable entropy source is a retained launch failure, not a panic.
    let c_name = random_name()?;
    let name = c_name
        .to_str()
        .map_err(|_| "lineage name is not UTF-8")?
        .to_owned();
    let new = i32::try_from(keyctl(
        JOIN_SESSION_KEYRING,
        c_name.as_ptr() as libc::c_long,
        0,
        0,
    )?)
    .map_err(|_| "joined session keyring serial exceeds i32")?;
    if new == old {
        return Err("lineage session keyring did not change".into());
    }
    validate_new_ring(new, &name)?;
    keyctl(LINK, old.into(), new.into(), 0)?;
    // Check once more after linking; neither a partial switch nor an
    // unexpected session identity is sufficient to release the provider.
    validate_new_ring(new, &name)?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const TEST: &str =
        "completion_owner::custody::lineage_keyring::tests::lineage_isolated_process";
    const CHILD: &str = "AGE319_LINEAGE_CHILD";

    struct SyntheticKey {
        serial: libc::c_long,
        old: i32,
    }
    impl Drop for SyntheticKey {
        fn drop(&mut self) {
            if let Err(error) = keyctl(UNLINK, self.serial, self.old.into(), 0)
                && !std::thread::panicking()
            {
                panic!("synthetic key cleanup failed: {error}");
            }
        }
    }

    #[test]
    fn lineage_isolated_process() {
        match std::env::var(CHILD).as_deref() {
            Ok("setup") => check_setup(),
            Ok("exec") => check_exec(),
            Ok("orphan") => check_orphan(),
            Ok("sibling") => check_sibling(),
            _ => {
                let original = ring_id(SESSION_KEYRING, false).ok();
                let output = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", TEST, "--nocapture"])
                    .env(CHILD, "setup")
                    .output()
                    .unwrap();
                eprintln!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    output.status.success(),
                    "isolated kernel test failed: {output:?}"
                );
                assert_eq!(
                    ring_id(SESSION_KEYRING, false).ok(),
                    original,
                    "parent ring changed"
                );
            }
        }
    }

    fn check_setup() {
        let old = match ring_id(SESSION_KEYRING, false) {
            Ok(old) => old,
            Err(e) if unsupported(&e) => {
                eprintln!("SKIP: keyring unavailable: {e}");
                return;
            }
            Err(e) => panic!("baseline session keyring: {e}"),
        };
        // An unmarked sibling starts before the worker joins its own ring.
        let sibling = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, "sibling")
            .env("AGE319_EXPECT_OLD_RING", old.to_string())
            .spawn()
            .unwrap();
        let key_description = format!("age319-test-{}", uuid::Uuid::new_v4());
        let key_type = CString::new("user").unwrap();
        let key_description_c = CString::new(key_description.as_str()).unwrap();
        let payload = b"synthetic old session key (not a credential)";
        let key = unsafe {
            libc::syscall(
                libc::SYS_add_key,
                key_type.as_ptr(),
                key_description_c.as_ptr(),
                payload.as_ptr(),
                payload.len(),
                old,
            )
        };
        if key < 0 {
            let e = io::Error::last_os_error().to_string();
            assert!(sibling.wait_with_output().unwrap().status.success());
            if unsupported(&e) {
                eprintln!("SKIP: synthetic old-key creation unavailable: {e}");
                return;
            }
            panic!("synthetic old-key creation failed: {e}");
        }
        let _synthetic_key = SyntheticKey { serial: key, old };
        let name = match establish() {
            Ok(name) => name,
            Err(e) if unsupported(&e) => {
                let status = sibling.wait_with_output().unwrap().status;
                assert!(status.success());
                eprintln!("SKIP: keyring creation/anchor/join unavailable: {e}");
                return;
            }
            Err(e) => panic!("lineage establishment: {e}"),
        };
        let new = ring_id(SESSION_KEYRING, false).unwrap();
        assert_ne!(new, old);
        validate_new_ring(new, &name).unwrap();
        let run = |mode| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", TEST, "--nocapture"])
                .env(CHILD, mode)
                .env("AGE319_EXPECT_OLD_RING", old.to_string())
                .env("AGE319_EXPECT_TEST_KEY", key.to_string())
                .env("AGE319_EXPECT_KEY_DESCRIPTION", &key_description)
                .env("AGE319_EXPECT_NEW_RING", new.to_string())
                .env("AGE319_EXPECT_NAME", &name);
            command
        };
        let output = run("exec").output().unwrap();
        assert!(output.status.success(), "fork/exec: {output:?}");
        let sibling = sibling.wait_with_output().unwrap();
        assert!(sibling.status.success(), "unmarked sibling: {sibling:?}");
        assert_eq!(ring_id(SESSION_KEYRING, false).unwrap(), new);
        // Prove the old ring is reachable through the NEW session keyring,
        // not merely the worker's temporary process keyring. The child has no
        // process keyring inherited from this worker.
        let orphan = run("orphan").output().unwrap();
        assert!(orphan.status.success(), "setsid/reparent: {orphan:?}");
        eprintln!("lineage={name} old={old} new={new}; fork/exec, sibling and orphan checked");
    }

    fn check_exec() {
        let serial: i32 = std::env::var("AGE319_EXPECT_NEW_RING")
            .unwrap()
            .parse()
            .unwrap();
        let name = std::env::var("AGE319_EXPECT_NAME").unwrap();
        validate_new_ring(serial, &name).unwrap();
        assert!(
            ring_id(PROCESS_KEYRING, false).is_err(),
            "worker process ring leaked into exec child"
        );
        search_old_key(serial);
    }

    fn check_sibling() {
        let old: i32 = std::env::var("AGE319_EXPECT_OLD_RING")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(ring_id(SESSION_KEYRING, false).unwrap(), old);
        assert!(
            !describe(old)
                .unwrap()
                .rsplit(';')
                .next()
                .unwrap()
                .starts_with(PREFIX)
        );
    }

    fn check_orphan() {
        // A separate subreaper test process owns the grandchild's wait; no
        // orphan is abandoned to init. This process is itself marked.
        assert_eq!(
            unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
            0
        );
        let old: i32 = std::env::var("AGE319_EXPECT_OLD_RING")
            .unwrap()
            .parse()
            .unwrap();
        let original = unsafe { libc::getpid() };
        let new: i32 = std::env::var("AGE319_EXPECT_NEW_RING")
            .unwrap()
            .parse()
            .unwrap();
        let name = std::env::var("AGE319_EXPECT_NAME").unwrap();
        let key_type = CString::new("user").unwrap();
        let description =
            CString::new(std::env::var("AGE319_EXPECT_KEY_DESCRIPTION").unwrap()).unwrap();
        let expected: libc::c_long = std::env::var("AGE319_EXPECT_TEST_KEY")
            .unwrap()
            .parse()
            .unwrap();
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            let grandchild = unsafe { libc::fork() };
            if grandchild < 0 {
                unsafe { libc::_exit(70) }
            }
            if grandchild > 0 {
                unsafe { libc::_exit(0) }
            }
            // Wait until the intermediary dies and this test process adopts us.
            for _ in 0..1000 {
                if unsafe { libc::getppid() } == original {
                    break;
                }
                unsafe { libc::usleep(1000) };
            }
            if unsafe { libc::getppid() } != original || unsafe { libc::setsid() } < 0 {
                unsafe { libc::_exit(71) }
            }
            // Only stack data and raw syscalls in the post-fork grandchild.
            let current = unsafe {
                libc::syscall(libc::SYS_keyctl, GET_KEYRING_ID, SESSION_KEYRING, 0, 0, 0)
            };
            if current != libc::c_long::from(new) {
                unsafe { libc::_exit(72) }
            }
            let mut buffer = [0_u8; 256];
            let length = unsafe {
                libc::syscall(
                    libc::SYS_keyctl,
                    DESCRIBE,
                    new as libc::c_long,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    0,
                )
            };
            if length <= 0
                || length as usize > buffer.len()
                || buffer[length as usize - 1] != 0
                || !buffer[..length as usize - 1].ends_with(name.as_bytes())
            {
                unsafe { libc::_exit(72) }
            }
            if unsafe { libc::syscall(libc::SYS_keyctl, GET_KEYRING_ID, PROCESS_KEYRING, 0, 0, 0) }
                >= 0
            {
                unsafe { libc::_exit(73) }
            }
            if unsafe {
                libc::syscall(
                    libc::SYS_keyctl,
                    DESCRIBE,
                    old as libc::c_long,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    0,
                )
            } < 0
            {
                unsafe { libc::_exit(73) }
            }
            let found = unsafe {
                libc::syscall(
                    libc::SYS_keyctl,
                    SEARCH,
                    new as libc::c_long,
                    key_type.as_ptr(),
                    description.as_ptr(),
                    0 as libc::c_long,
                )
            };
            if found != expected {
                unsafe { libc::_exit(74) }
            }
            unsafe { libc::_exit(0) }
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        let adopted = unsafe { libc::waitpid(-1, &mut status, 0) };
        assert!(adopted > 0);
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "grandchild status: {status}"
        );
    }

    fn search_old_key(new: i32) {
        let key_type = CString::new("user").unwrap();
        let description =
            CString::new(std::env::var("AGE319_EXPECT_KEY_DESCRIPTION").unwrap()).unwrap();
        let expected: libc::c_long = std::env::var("AGE319_EXPECT_TEST_KEY")
            .unwrap()
            .parse()
            .unwrap();
        let found = unsafe {
            libc::syscall(
                libc::SYS_keyctl,
                SEARCH,
                new as libc::c_long,
                key_type.as_ptr(),
                description.as_ptr(),
                0 as libc::c_long,
            )
        };
        assert_eq!(
            found,
            expected,
            "old key search from new session ring: {}",
            io::Error::last_os_error()
        );
    }

    fn unsupported(error: &str) -> bool {
        error.contains("Operation not permitted")
            || error.contains("Function not implemented")
            || error.contains("Operation not supported")
    }
}
