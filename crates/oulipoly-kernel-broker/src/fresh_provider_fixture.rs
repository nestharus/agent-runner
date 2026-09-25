//! Private deterministic provider executable. This is an ELF binary launched
//! directly by broker K; it never calls the Runner's local provider spawn.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Command, Stdio};

const CAUSAL_BASH_INTERMEDIARY_DELAY: std::time::Duration = std::time::Duration::from_millis(80);

fn causal_bash(args: &[String], hold_survivor: bool, hold_start: bool) -> std::io::Result<()> {
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
        if hold_start {
            std::fs::write(gate.join("causal-before-c"), b"ready")?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !gate.join("causal-release-c").exists() {
                if std::time::Instant::now() >= deadline {
                    return Err(std::io::Error::other("causal C release expired"));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let status = std::fs::read_to_string("/proc/self/status")?;
        let proc_pid = status
            .lines()
            .find_map(|line| line.strip_prefix("Pid:"))
            .ok_or_else(|| std::io::Error::other("intermediary proc PID absent"))?
            .trim();
        std::fs::write(gate.join("causal-intermediary-proc-pid"), proc_pid)?;
        if hold_survivor {
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
        if args.len() != 2 && args.len() != 7 && args.len() != 8 {
            return Err(std::io::Error::other(
                "interactive fixture arguments changed",
            ));
        }
        let tty = unsafe { libc::isatty(0) } == 1
            && unsafe { libc::isatty(1) } == 1
            && unsafe { libc::tcgetsid(0) } == unsafe { libc::getsid(0) };
        let native_store = std::env::var("AGE319_PRIVATE_NATIVE_STORE").ok();
        if let Some(path) = native_store.as_deref() {
            let session_id = std::env::var("AGE319_PRIVATE_NATIVE_SESSION")
                .map_err(|_| std::io::Error::other("broker-minted native session absent"))?;
            let native = serde_json::json!({
                "format": "age319-interactive-native-session/v1",
                "session_id": session_id,
                "store_nonce": uuid::Uuid::new_v4().to_string(),
                "provider_local_pid": unsafe { libc::getpid() },
                "controlling_tty": tty,
                "turns": [],
            });
            let mut store = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            store.write_all(&serde_json::to_vec(&native)?)?;
            store.sync_all()?;
        }
        if args.len() >= 7 {
            causal_bash(&args[1..], false, true)?;
        }
        std::io::stdout().write_all(b"interactive-ready\n")?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input == "[Oulipoly native F v1]\n" {
            let mut envelope = input.clone();
            loop {
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line)? == 0 || envelope.len() > 256 * 1024 {
                    return Err(std::io::Error::other("native F envelope incomplete"));
                }
                let end = line == "[/Oulipoly native F v1]\n";
                envelope.push_str(&line);
                if end {
                    break;
                }
            }
            let path = native_store.ok_or_else(|| std::io::Error::other("native store absent"))?;
            let mut native: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            let body = envelope.trim_end_matches('\n');
            let field = |prefix: &str| -> std::io::Result<String> {
                body.lines()
                    .find_map(|line| line.strip_prefix(prefix))
                    .map(str::to_owned)
                    .ok_or_else(|| std::io::Error::other("native F field absent"))
            };
            let turn = serde_json::json!({
                "turn_id": uuid::Uuid::new_v4().to_string(),
                "body": body,
                "nonce": field("nonce: ")?,
                "session_id": field("session: ")?,
                "payload_sha256": field("payload-sha256: ")?,
                "payload_base64": field("payload-base64: ")?,
            });
            native["turns"]
                .as_array_mut()
                .ok_or_else(|| std::io::Error::other("native turns absent"))?
                .push(turn);
            let staged = format!("{path}.provider-staged");
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&staged)?;
            file.write_all(&serde_json::to_vec(&native)?)?;
            file.sync_all()?;
            std::fs::rename(staged, &path)?;
            input.clear();
            std::io::stdin().read_line(&mut input)?;
        }
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
        return causal_bash(&args, true, false);
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
