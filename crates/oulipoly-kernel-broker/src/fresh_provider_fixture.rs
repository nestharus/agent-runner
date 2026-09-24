//! Private deterministic provider executable. This is an ELF binary launched
//! directly by broker K; it never calls the Runner's local provider spawn.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Command, Stdio};

fn causal_bash(args: &[String]) -> std::io::Result<()> {
    let marker = std::path::Path::new(&args[0]);
    let gate = marker
        .parent()
        .ok_or_else(|| std::io::Error::other("gate absent"))?;
    let first = unsafe { libc::fork() };
    if first < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if first != 0 {
        return Ok(());
    }
    unsafe {
        if libc::setsid() < 0 {
            libc::_exit(71);
        }
        if libc::clearenv() != 0 {
            libc::_exit(71);
        }
        let keyring = libc::syscall(libc::SYS_keyctl, 1, 0);
        let keyring_result = if keyring >= 0 {
            "joined-empty-session-keyring".to_owned()
        } else {
            format!("unavailable: {}", std::io::Error::last_os_error())
        };
        if std::fs::write(gate.join("causal-keyring-result"), keyring_result).is_err()
            || std::fs::write(
                gate.join("causal-env-count"),
                std::env::vars_os().count().to_string(),
            )
            .is_err()
        {
            libc::_exit(71);
        }
        if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) != 0 {
            libc::_exit(72);
        }
    }
    std::fs::write(gate.join("causal-fd-clear"), b"closed")?;
    let second = unsafe { libc::fork() };
    if second < 0 {
        unsafe {
            libc::_exit(73);
        }
    }
    if second != 0 {
        unsafe {
            libc::_exit(0);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    let outcome = (|| -> std::io::Result<()> {
        std::fs::write(gate.join("causal-intermediary-start"), b"started")?;
        let status = std::fs::read_to_string("/proc/self/status")?;
        let proc_pid = status
            .lines()
            .find_map(|line| line.strip_prefix("Pid:"))
            .ok_or_else(|| std::io::Error::other("intermediary proc PID absent"))?
            .trim();
        std::fs::write(gate.join("causal-intermediary-proc-pid"), proc_pid)?;
        let survivor = unsafe { libc::fork() };
        if survivor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if survivor == 0 {
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
                loop {
                    libc::pause();
                }
            }
        }
        if unsafe { libc::unshare(libc::CLONE_NEWPID) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let output = std::fs::File::create(gate.join("bash-causal-output"))?;
        let error = std::fs::File::create(gate.join("bash-causal-error"))?;
        let child = Command::new(&args[1])
            .arg("__age319-private-admit-child-v1")
            .args([&args[2], &args[3], &args[4]])
            .args(args.get(5))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(error))
            .spawn()?;
        std::fs::write(gate.join("causal-bash-pid"), child.id().to_string())?;
        Ok(())
    })();
    let code = match outcome {
        Ok(()) => 0,
        Err(error) => {
            let _ = std::fs::write(gate.join("causal-helper-error"), error.to_string());
            74
        }
    };
    unsafe {
        libc::_exit(code);
    }
}

fn main() -> std::io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let marker = args
        .first()
        .ok_or_else(|| std::io::Error::other("marker absent"))?;
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(marker)?;
    file.write_all(b"one-provider-effect\n")?;
    file.sync_all()?;
    if args.len() == 5 || args.len() == 6 {
        return causal_bash(&args);
    }
    if args.len() != 1 {
        return Err(std::io::Error::other("provider fixture arguments changed"));
    }
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            libc::setsid();
            libc::clearenv();
            if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) != 0 {
                libc::_exit(72);
            }
            loop {
                libc::pause();
            }
        }
    }
    std::io::stdout().write_all(b"provider-stdout:")?;
    std::io::stdout().write_all(&input)?;
    std::io::stderr().write_all(b"provider-stderr\n")?;
    Ok(())
}
