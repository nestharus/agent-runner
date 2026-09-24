//! Thin installed CLI/GUI entry. No Runner or State work occurs in this process.
#[cfg(target_os = "linux")]
fn main() {
    use oulipoly_kernel_broker::{
        installed_launch,
        installed_pair::{self, InstalledPair},
        protocol,
    };
    use std::os::fd::AsRawFd;
    use std::path::Path;

    let stdio_snapshot = match snapshot_stdio() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("OULIPOLY_INSTALLED_LAUNCH_GAP=cannot capture stdio: {error}");
            std::process::exit(70);
        }
    };
    let stdio = stdio_snapshot
        .each_ref()
        .map(|fd| fd.as_ref().map_or(-1, AsRawFd::as_raw_fd));

    #[cfg(feature = "age319-private-broker-fixture")]
    if private_fixture() {
        let code = private_main(stdio);
        std::process::exit(code);
    }

    let result = (|| -> std::io::Result<()> {
        let pair = InstalledPair::load(Path::new(installed_pair::MANIFEST), true)?;
        let digest = pair
            .launcher_sha256
            .as_deref()
            .ok_or_else(|| std::io::Error::other("installed pair lacks launcher image"))?;
        pair.verify_image(Path::new(installed_pair::LAUNCHER), digest, true)?;
        let captured = installed_launch::capture_from(
            &pair.generation,
            std::env::args_os().collect(),
            std::env::vars_os().collect(),
            stdio,
        )?;
        let descriptors: Vec<_> = captured
            .descriptors
            .iter()
            .map(AsRawFd::as_raw_fd)
            .collect();
        let response = protocol::submit_installed_launch_at(
            Path::new(protocol::INSTALLED_SOCKET),
            &captured.spec,
            &descriptors,
        )?;
        Err(std::io::Error::other(format!(
            "broker did not admit entry: {}",
            response.trim()
        )))
    })();
    if let Err(error) = result {
        eprintln!("OULIPOLY_INSTALLED_LAUNCH_GAP={error}");
        std::process::exit(70);
    }
}

#[cfg(target_os = "linux")]
fn snapshot_stdio() -> std::io::Result<[Option<std::os::fd::OwnedFd>; 3]> {
    use std::os::fd::FromRawFd;
    let mut snapshot = [None, None, None];
    for (index, slot) in snapshot.iter_mut().enumerate() {
        let fd = unsafe { libc::fcntl(index as i32, libc::F_DUPFD_CLOEXEC, 3) };
        if fd >= 0 {
            *slot = Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) });
        } else if std::io::Error::last_os_error().raw_os_error() != Some(libc::EBADF) {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(snapshot)
}

#[cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
fn private_fixture() -> bool {
    std::fs::read_to_string("/proc/self/uid_map")
        .ok()
        .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some()
}

#[cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
fn private_main(mut stdio: [std::os::fd::RawFd; 3]) -> i32 {
    use oulipoly_kernel_broker::{installed_launch, protocol};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    let result = (|| -> std::io::Result<i32> {
        if let Some(mask) = std::env::var_os("OULIPOLY_AGE319_PRIVATE_STDIO_MASK_V1") {
            let mask = mask.as_bytes();
            if mask.len() != 3 || mask.iter().any(|byte| !matches!(byte, b'0' | b'1')) {
                return Err(std::io::Error::other("invalid private stdio mask"));
            }
            for (index, byte) in mask.iter().enumerate() {
                if *byte == b'0' {
                    stdio[index] = -1;
                } else if stdio[index] < 0 {
                    return Err(std::io::Error::other(
                        "private stdio mask invented descriptor",
                    ));
                }
            }
        }
        let socket = std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
            .map_err(|_| std::io::Error::other("private broker socket missing"))?;
        let generation = std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1")
            .map_err(|_| std::io::Error::other("private launch generation missing"))?;
        let args: Vec<_> = std::env::args_os().collect();
        if args.len() == 3
            && (args[1] == "__age319-private-status-v1" || args[1] == "__age319-private-cancel-v1")
        {
            let id = args[2]
                .to_str()
                .ok_or_else(|| std::io::Error::other("invalid request ID"))?;
            let response = protocol::private_installed_control_at(
                Path::new(&socket),
                id,
                &generation,
                args[1] == "__age319-private-cancel-v1",
            )?;
            print!("{response}");
            return Ok(if response.starts_with("error ") {
                70
            } else {
                0
            });
        }
        let environment = std::env::vars_os()
            .filter(|(key, _)| {
                !key.as_bytes()
                    .starts_with(b"OULIPOLY_KERNEL_BROKER_FIXTURE_")
                    && key.as_bytes() != b"OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1"
                    && key.as_bytes() != b"OULIPOLY_AGE319_PRIVATE_STDIO_MASK_V1"
            })
            .collect();
        let mut captured = installed_launch::capture_from(&generation, args, environment, stdio)?;
        if let Some(id) = std::env::var_os("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1") {
            captured.spec.request_id = id.to_string_lossy().into_owned();
        }
        let descriptors: Vec<_> = captured
            .descriptors
            .iter()
            .map(AsRawFd::as_raw_fd)
            .collect();
        let response =
            protocol::submit_installed_launch_at(Path::new(&socket), &captured.spec, &descriptors)?;
        let expected = format!(" drained {}\n", captured.spec.request_id);
        if let Some(exit) = response
            .strip_suffix(&expected)
            .and_then(|prefix| prefix.strip_prefix("exit "))
        {
            let code: i32 = exit
                .parse()
                .map_err(|_| std::io::Error::other("invalid private Runner exit"))?;
            return Ok(code);
        }
        Err(std::io::Error::other(format!(
            "private broker launch refused: {}",
            response.trim()
        )))
    })();
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("OULIPOLY_PRIVATE_INSTALLED_LAUNCH_GAP={error}");
            70
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("oulipoly-installed-launcher requires Linux");
    std::process::exit(1);
}
