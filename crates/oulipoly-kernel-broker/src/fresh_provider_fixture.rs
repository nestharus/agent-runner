//! Private deterministic provider executable. This is an ELF binary launched
//! directly by broker K; it never calls the Runner's local provider spawn.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Command, Stdio};

const CAUSAL_BASH_INTERMEDIARY_DELAY: std::time::Duration = std::time::Duration::from_millis(80);

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
    std::thread::sleep(CAUSAL_BASH_INTERMEDIARY_DELAY);
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
            .args(args.get(6))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("OULIPOLY_DATA_DIR", &args[5])
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
    if args.first().is_some_and(|arg| arg == "--interactive-only") {
        if args.len() != 2 {
            return Err(std::io::Error::other(
                "interactive fixture arguments changed",
            ));
        }
        let tty = unsafe { libc::isatty(0) } == 1
            && unsafe { libc::isatty(1) } == 1
            && unsafe { libc::tcgetsid(0) } == unsafe { libc::getsid(0) };
        std::io::stdout().write_all(b"interactive-ready\n")?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let evidence = serde_json::json!({
            "controlling_tty": tty,
            "input": input,
            "pid": unsafe { libc::getpid() },
        });
        std::fs::write(&args[1], serde_json::to_vec(&evidence)?)?;
        std::io::stdout().write_all(b"interactive-output:")?;
        std::io::stdout().write_all(input.as_bytes())?;
        return if tty {
            Ok(())
        } else {
            Err(std::io::Error::other("no controlling tty"))
        };
    }
    let marker = args
        .first()
        .ok_or_else(|| std::io::Error::other("marker absent"))?;
    let fail = std::env::args().nth(2).as_deref() == Some("--fail");
    let quota = std::env::args().nth(2).as_deref() == Some("--quota");
    let auth = std::env::args().nth(2).as_deref() == Some("--auth");
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(marker)?;
    file.write_all(b"one-provider-effect\n")?;
    file.sync_all()?;
    if args.len() == 6 || args.len() == 7 {
        return causal_bash(&args);
    }
    if args.len() != 1
        && !(args.len() == 2 && matches!(args[1].as_str(), "--fail" | "--quota" | "--auth"))
    {
        return Err(std::io::Error::other("provider fixture arguments changed"));
    }
    if quota {
        std::io::stderr().write_all(
            br#"{"type":"error","error":{"data":{"message":"quota exhausted for account"}}}"#,
        )?;
        std::process::exit(1);
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
    if auth {
        std::io::stderr().write_all(b"authentication failed: token expired")?;
        std::process::exit(1);
    }
    std::io::stderr().write_all(b"provider-stderr\n")?;
    if fail {
        std::process::exit(9);
    }
    Ok(())
}
