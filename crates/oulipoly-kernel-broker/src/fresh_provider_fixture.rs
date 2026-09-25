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
        if args.get(5).map(String::as_str) != Some("ordinary-sync-parent-output") {
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
        if !args
            .get(5)
            .is_some_and(|option| option.starts_with("ordinary-"))
            && unsafe { libc::unshare(libc::CLONE_NEWPID) } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if args
            .get(5)
            .is_some_and(|option| option.starts_with("ordinary-"))
        {
            if args[5] == "ordinary-refuse" {
                use std::os::unix::fs::PermissionsExt;
                let refused_shebang = gate.join("ordinary-refused-shebang");
                std::fs::write(&refused_shebang, "#!/bin/sh\nprintf effect > \"$1\"\n")?;
                std::fs::set_permissions(&refused_shebang, std::fs::Permissions::from_mode(0o755))?;
                let non_executable = gate.join("ordinary-non-executable");
                std::fs::write(&non_executable, "#!/bin/sh\nprintf effect > \"$1\"\n")?;
                std::fs::set_permissions(&non_executable, std::fs::Permissions::from_mode(0o644))?;
                let plain_script = gate.join("ordinary-plain-script");
                std::fs::write(&plain_script, "printf effect > \"$1\"\n")?;
                std::fs::set_permissions(&plain_script, std::fs::Permissions::from_mode(0o755))?;
                let malformed_elf = gate.join("ordinary-malformed-elf");
                std::fs::write(&malformed_elf, b"\x7fELFbroken")?;
                std::fs::set_permissions(&malformed_elf, std::fs::Permissions::from_mode(0o755))?;
                let missing_interp = gate.join("ordinary-missing-interp");
                let mut elf = vec![0u8; 256];
                elf[..4].copy_from_slice(b"\x7fELF");
                elf[4] = 2;
                elf[5] = 1;
                elf[6] = 1;
                elf[16..18].copy_from_slice(&2u16.to_le_bytes());
                elf[18..20].copy_from_slice(&62u16.to_le_bytes());
                elf[20..24].copy_from_slice(&1u32.to_le_bytes());
                elf[32..40].copy_from_slice(&64u64.to_le_bytes());
                elf[52..54].copy_from_slice(&64u16.to_le_bytes());
                elf[54..56].copy_from_slice(&56u16.to_le_bytes());
                elf[56..58].copy_from_slice(&2u16.to_le_bytes());
                elf[64..68].copy_from_slice(&1u32.to_le_bytes());
                elf[68..72].copy_from_slice(&5u32.to_le_bytes());
                elf[96..104].copy_from_slice(&256u64.to_le_bytes());
                elf[104..112].copy_from_slice(&256u64.to_le_bytes());
                elf[120..124].copy_from_slice(&3u32.to_le_bytes());
                elf[128..136].copy_from_slice(&200u64.to_le_bytes());
                let missing_loader = b"/no/such/age319-elf-loader\0";
                elf[152..160].copy_from_slice(&(missing_loader.len() as u64).to_le_bytes());
                elf[200..200 + missing_loader.len()].copy_from_slice(missing_loader);
                std::fs::write(&missing_interp, elf)?;
                std::fs::set_permissions(&missing_interp, std::fs::Permissions::from_mode(0o755))?;
                let mut statuses = Vec::new();
                for case in [
                    "root",
                    "ready",
                    "cancel",
                    "env-drift",
                    "cwd-drift",
                    "argv-before-c",
                    "argv-after-c",
                    "shebang",
                    "malformed-elf",
                    "missing-interp",
                    "non-executable",
                    "plain-script",
                ] {
                    let mut command = Command::new(&args[1]);
                    command.args(["run", "--delivery", "sync"]);
                    match case {
                        "root" => {
                            command.args(["--completion-scope", "root"]);
                        }
                        "ready" => {
                            command.args(["--ready-sentinel", "READY"]);
                        }
                        "cancel" => {
                            command
                                .args(["--cancel-on-owner-exit", "--owner-pid"])
                                .arg(std::process::id().to_string());
                        }
                        _ => {}
                    }
                    if matches!(
                        case,
                        "shebang"
                            | "malformed-elf"
                            | "missing-interp"
                            | "non-executable"
                            | "plain-script"
                    ) {
                        let image = match case {
                            "shebang" => &refused_shebang,
                            "malformed-elf" => &malformed_elf,
                            "non-executable" => &non_executable,
                            "plain-script" => &plain_script,
                            _ => &missing_interp,
                        };
                        command.arg("--").arg(image);
                    } else {
                        command.args(["--", "sh", "-c", "printf effect > \"$1\"", "sh"]);
                    }
                    let status = command
                        .arg(gate.join(match case {
                            "shebang" => "ordinary-shebang-effect",
                            "plain-script" => "ordinary-plain-effect",
                            _ => "ordinary-refused-effect",
                        }))
                        .env_clear()
                        .env("PATH", "/usr/bin:/bin")
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &args[2])
                        .env("AGE319_ORDINARY_EFFECTIVE_ENV", "original-value")
                        .envs(
                            (case == "env-drift")
                                .then_some(("AGE319_PRIVATE_ORDINARY_MUTATE_ENV_AFTER_C_V1", "1")),
                        )
                        .envs(
                            (case == "cwd-drift")
                                .then_some(("AGE319_PRIVATE_ORDINARY_MUTATE_CWD_AFTER_C_V1", "1")),
                        )
                        .envs(
                            (case == "argv-before-c").then_some((
                                "AGE319_PRIVATE_ORDINARY_MUTATE_ARGV_BEFORE_C_V1",
                                "1",
                            )),
                        )
                        .envs(
                            (case == "argv-after-c")
                                .then_some(("AGE319_PRIVATE_ORDINARY_MUTATE_ARGV_AFTER_C_V1", "1")),
                        )
                        .stdin(Stdio::null())
                        .stdout(Stdio::from(std::fs::File::create(
                            gate.join(format!("ordinary-{case}-output")),
                        )?))
                        .stderr(Stdio::from(std::fs::File::create(
                            gate.join(format!("ordinary-{case}-error")),
                        )?))
                        .status()?;
                    statuses.push((case, status.code().unwrap_or(70)));
                }
                std::fs::write(
                    gate.join("ordinary-refuse-statuses"),
                    serde_json::to_vec(&statuses)?,
                )?;
            } else {
                let script_case = args[5].starts_with("ordinary-script");
                let script_path = gate.join("script-bin/scriptcmd");
                if script_case {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::create_dir_all(script_path.parent().unwrap())?;
                    std::fs::write(
                        &script_path,
                        "#!/bin/sh\nprintf 'old|%s|%s|%s|%s|%s\\n' \"$0\" \"$1\" \"$2\" \"$PWD\" \"$AGE319_ORDINARY_EFFECTIVE_ENV\"\nprintf old > \"$3\"\n",
                    )?;
                    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
                }
                let mode = if args[5] == "ordinary-async" || args[5] == "ordinary-restart" {
                    "async"
                } else {
                    "sync"
                };
                let delay = if args[5] == "ordinary-restart" || args[5] == "ordinary-cancel" {
                    "2"
                } else {
                    "0.3"
                };
                let ending = if args[5] == "ordinary-failure" {
                    "exit 37"
                } else if args[5] == "ordinary-sync-signal" {
                    "kill -TERM $$"
                } else if args[5] == "ordinary-sync-large" {
                    "head -c 200000 /dev/zero"
                } else {
                    ":"
                };
                let script = format!(
                    "test -c /dev/stdin || exit 90; test \"$AGE319_ORDINARY_EFFECTIVE_ENV\" = original-value || exit 91; printf '\\001\\377ordinary\\000'; printf 'err\\000\\376' >&2; printf effect > \"$1\"; (sleep {delay}; printf background > \"$2\") & {ending}"
                );
                let image = gate.join("ordinary-elf-image");
                if args[5] == "ordinary-elf" {
                    use std::os::unix::ffi::OsStrExt;
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::copy("/bin/sh", &image)?;
                    std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o755))?;
                    let path = std::ffi::CString::new(image.as_os_str().as_bytes())?;
                    let name = c"user.age319-ordinary";
                    let value = b"original-xattr";
                    if unsafe {
                        libc::setxattr(
                            path.as_ptr(),
                            name.as_ptr(),
                            value.as_ptr().cast(),
                            value.len(),
                            0,
                        )
                    } != 0
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                let mut command = Command::new(&args[1]);
                command.args(["run", "--delivery", mode, "--"]);
                if script_case {
                    command.args(["scriptcmd", "alpha", "beta"]);
                } else if args[5] == "ordinary-elf" {
                    command.arg(&image).args(["-c", script.as_str(), "sh"]);
                } else {
                    command.args(["sh", "-c", script.as_str(), "sh"]);
                }
                let mut child =
                    command
                        .arg(gate.join("ordinary-effect"))
                        .args((!script_case).then_some(gate.join("ordinary-background")))
                        .args((!script_case).then_some(""))
                        .env_clear()
                        .env(
                            "PATH",
                            if script_case {
                                format!("{}:/usr/bin:/bin", script_path.parent().unwrap().display())
                            } else {
                                "/usr/bin:/bin".into()
                            },
                        )
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &args[2])
                        .env("AGE319_ORDINARY_EFFECTIVE_ENV", "original-value")
                        .env(
                            "AGE319_ORDINARY_SECRET_SENTINEL",
                            "age319-secret-must-stay-in-memfd-319",
                        )
                        .envs(
                            (args[5] == "ordinary-loss" || args[5] == "ordinary-script-loss")
                                .then_some(("AGE319_PRIVATE_ORDINARY_DROP_C_REPLY_V1", "1")),
                        )
                        .envs(
                            (args[5] == "ordinary-loss" || args[5] == "ordinary-script-loss")
                                .then_some(("AGE319_PRIVATE_ORDINARY_DROP_K_REPLY_V1", "1")),
                        )
                        .envs(
                            (args[5] == "ordinary-loss" || args[5] == "ordinary-script-loss")
                                .then_some(("AGE319_PRIVATE_ORDINARY_DROP_Q_REPLY_V1", "1")),
                        )
                        .envs(
                            (args[5] == "ordinary-loss" || args[5] == "ordinary-script-loss")
                                .then_some(("AGE319_PRIVATE_ORDINARY_DROP_W_REPLY_V1", "1")),
                        )
                        .envs(
                            (args[5] == "ordinary-cancel")
                                .then_some(("AGE319_PRIVATE_ORDINARY_CANCEL_AFTER_K_V1", "1")),
                        )
                        .envs(
                            (args[5] == "ordinary-async")
                                .then_some(("AGE319_PRIVATE_ASYNC_PROBE_SYNC_REFUSAL_V1", "1")),
                        )
                        .envs(
                            (args[5] == "ordinary-sync-reply-loss")
                                .then_some(("AGE319_PRIVATE_SYNC_DROP_BEGIN_REPLY_V1", "1")),
                        )
                        .envs(
                            (args[5] == "ordinary-sync-partial")
                                .then_some(("AGE319_PRIVATE_SYNC_PARTIAL_CALLER_WRITE_V1", "1")),
                        )
                        .envs(
                            (args[5] == "ordinary-sync-repeat")
                                .then_some(("AGE319_PRIVATE_SYNC_REPEAT_BEGIN_V1", "1")),
                        )
                        .envs((args[5] == "ordinary-sync-tamper").then_some((
                            "AGE319_PRIVATE_SYNC_PAUSE_AFTER_W_DIR_V1",
                            gate.as_os_str(),
                        )))
                        .envs((args[5] == "ordinary-sync-post-tamper").then_some((
                            "AGE319_PRIVATE_SYNC_PAUSE_AFTER_BEGIN_DIR_V1",
                            gate.as_os_str(),
                        )))
                        .envs((args[5] == "ordinary-sync-encode-tamper").then_some((
                            "AGE319_PRIVATE_SYNC_PAUSE_AFTER_VERIFY_DIR_V1",
                            gate.as_os_str(),
                        )))
                        .envs(
                            (args[5] == "ordinary-parent-tamper"
                                || args[5] == "ordinary-script-replace"
                                || args[5] == "ordinary-script-remove")
                                .then_some((
                                    "AGE319_PRIVATE_ORDINARY_PAUSE_AFTER_C_DIR_V1",
                                    gate.as_os_str(),
                                )),
                        )
                        .stdin(Stdio::null())
                        .stdout(Stdio::from(std::fs::File::create(
                            gate.join("bash-causal-output"),
                        )?))
                        .stderr(Stdio::from(std::fs::File::create(
                            gate.join("bash-causal-error"),
                        )?))
                        .spawn()?;
                std::fs::write(gate.join("causal-bash-pid"), child.id().to_string())?;
                let status = child.wait()?;
                std::fs::write(
                    gate.join("ordinary-bash-status"),
                    status.code().unwrap_or(70).to_string(),
                )?;
                if args[5] == "ordinary-copy" && status.success() {
                    let report: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(gate.join("bash-causal-output"))?)?;
                    let copied = report["publication"]["child"]["request_id"]
                        .as_str()
                        .ok_or_else(|| {
                            std::io::Error::other("ordinary copied request id absent")
                        })?;
                    let sibling = Command::new(&args[1])
                        .args(["__age319-private-probe-sibling-read-v1", &args[2], copied])
                        .env_clear()
                        .env("PATH", "/usr/bin:/bin")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::from(std::fs::File::create(
                            gate.join("ordinary-sibling-error"),
                        )?))
                        .status()?;
                    std::fs::write(
                        gate.join("ordinary-sibling-status"),
                        sibling.code().unwrap_or(70).to_string(),
                    )?;
                    let status = Command::new(&args[1])
                        .args([
                            "run",
                            "--delivery",
                            "sync",
                            "--",
                            "sh",
                            "-c",
                            script.as_str(),
                            "sh",
                        ])
                        .arg(gate.join("ordinary-effect"))
                        .arg(gate.join("ordinary-background"))
                        .arg("")
                        .env_clear()
                        .env("PATH", "/usr/bin:/bin")
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &args[2])
                        .env("AGE319_ORDINARY_EFFECTIVE_ENV", "original-value")
                        .env("AGE319_PRIVATE_ORDINARY_COPIED_REQUEST_ID_V1", copied)
                        .stdin(Stdio::null())
                        .stdout(Stdio::from(std::fs::File::create(
                            gate.join("ordinary-copy-output"),
                        )?))
                        .stderr(Stdio::from(std::fs::File::create(
                            gate.join("ordinary-copy-error"),
                        )?))
                        .status()?;
                    std::fs::write(
                        gate.join("ordinary-copy-status"),
                        status.code().unwrap_or(70).to_string(),
                    )?;
                }
            }
        } else {
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
        }
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
    let fail = matches!(
        std::env::args().nth(2).as_deref(),
        Some("--fail" | "--fail-clean")
    );
    let quota = matches!(
        std::env::args().nth(2).as_deref(),
        Some("--quota" | "--quota-clean" | "--quota-single")
    );
    let single_process_quota = std::env::args().nth(2).as_deref() == Some("--quota-single");
    let auth = std::env::args().nth(2).as_deref() == Some("--auth");
    let binary = std::env::args().nth(2).as_deref() == Some("--binary");
    let clean = matches!(
        std::env::args().nth(2).as_deref(),
        Some("--clean" | "--fail-clean" | "--quota-clean" | "--capacity-clean")
    );
    let capacity = matches!(
        std::env::args().nth(2).as_deref(),
        Some("--capacity" | "--capacity-clean")
    );
    let process = serde_json::json!({
        "argv": std::env::args().skip(1).collect::<Vec<_>>(),
        "cwd": std::env::current_dir()?,
        "selected_account": std::env::var("AGE319_SELECTED_ACCOUNT").ok(),
        "env": std::env::vars().collect::<std::collections::BTreeMap<_, _>>(),
        "uid": unsafe { libc::getuid() },
        "no_new_privs": unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) },
        "seccomp": unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) },
        "forbidden_environment": std::env::vars().any(|(key, _)| key.starts_with("OULIPOLY_KERNEL_") || key.starts_with("LD_") || key.starts_with("DYLD_")),
    });
    std::fs::write(
        format!("{marker}.process.json"),
        serde_json::to_vec(&process)?,
    )?;
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
    if args.len() != 1
        && !(args.len() == 2
            && matches!(
                args[1].as_str(),
                "--fail"
                    | "--quota"
                    | "--auth"
                    | "--binary"
                    | "--clean"
                    | "--fail-clean"
                    | "--capacity"
                    | "--quota-clean"
                    | "--quota-single"
                    | "--capacity-clean"
            ))
    {
        return Err(std::io::Error::other("provider fixture arguments changed"));
    }
    if !clean && !binary && !single_process_quota {
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
    }
    if quota {
        std::io::stderr().write_all(
            br#"{"type":"error","error":{"data":{"message":"quota exhausted for account"}}}"#,
        )?;
        std::process::exit(1);
    }
    if binary {
        std::io::stdout().write_all(b"\0\xffstdout\n")?;
        std::io::stderr().write_all(b"err\0\xfestderr")?;
        return Ok(());
    }
    if capacity {
        std::io::stderr().write_all(
            br#"{"type":"error","error":{"data":{"code":"model_at_capacity","message":"model busy"}}}"#,
        )?;
        std::process::exit(1);
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
