//! Private deterministic provider executable. This is an ELF binary launched
//! directly by broker K; it never calls the Runner's local provider spawn.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let marker = args
        .next()
        .ok_or_else(|| std::io::Error::other("marker absent"))?;
    let status_path = args.next();
    if let Some(path) = &status_path {
        write_status(path)?;
    }
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
            if let Some(path) = &status_path {
                if write_status(&format!("{path}.adopted")).is_err() {
                    libc::_exit(73);
                }
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

fn write_status(path: &str) -> std::io::Result<()> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let mut evidence = String::new();
    for key in [
        "Uid:",
        "Gid:",
        "NoNewPrivs:",
        "Seccomp:",
        "CapBnd:",
        "CapEff:",
        "NSpid:",
    ] {
        let line = status
            .lines()
            .find(|line| line.starts_with(key))
            .ok_or_else(|| std::io::Error::other("process status field absent"))?;
        evidence.push_str(line);
        evidence.push('\n');
    }
    evidence.push_str("uid_map=");
    evidence.push_str(&std::fs::read_to_string("/proc/self/uid_map")?.replace('\n', ";"));
    evidence.push('\n');
    evidence.push_str(&format!(
        "user_ns={:?}\n",
        std::fs::read_link("/proc/self/ns/user")?
    ));
    let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    evidence.push_str(&format!(
        "rlimit_nofile={}/{}\n",
        limit.rlim_cur, limit.rlim_max
    ));
    std::fs::write(path, evidence)
}
