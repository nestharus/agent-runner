//! Private deterministic provider executable. This is an ELF binary launched
//! directly by broker K; it never calls the Runner's local provider spawn.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

fn main() -> std::io::Result<()> {
    let marker = std::env::args()
        .nth(1)
        .ok_or_else(|| std::io::Error::other("marker absent"))?;
    let fail = std::env::args().nth(2).as_deref() == Some("--fail");
    let quota = std::env::args().nth(2).as_deref() == Some("--quota");
    let auth = std::env::args().nth(2).as_deref() == Some("--auth");
    let capacity = std::env::args().nth(2).as_deref() == Some("--capacity");
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
    if quota {
        std::io::stderr().write_all(
            br#"{"type":"error","error":{"data":{"message":"quota exhausted for account"}}}"#,
        )?;
        std::process::exit(1);
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
