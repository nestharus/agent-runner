//! Versioned, inert installed-entry handoff. The host broker must own any
//! eventual execution; this module only captures and validates the request.
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;

pub const PROTOCOL: &str = "oulipoly-installed-launch/v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Cli,
    Gui,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledLaunchSpec {
    pub protocol: String,
    pub generation: String,
    pub request_id: String,
    pub kind: EntryKind,
    /// Argument bytes after argv[0]. The broker supplies the fixed Runner image.
    pub args: Vec<Vec<u8>>,
    pub environment: Vec<(Vec<u8>, Vec<u8>)>,
    /// Absent standard descriptors stay absent; cwd is always the final FD.
    pub stdio_present: [bool; 3],
}

pub struct CapturedLaunch {
    pub spec: InstalledLaunchSpec,
    pub descriptors: Vec<OwnedFd>,
}

pub fn capture(
    generation: &str,
    argv: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
) -> io::Result<CapturedLaunch> {
    capture_from(generation, argv, environment, [0, 1, 2])
}

/// Also used by private tests to hand over PTY and GUI descriptors without
/// changing the test process's own standard streams.
pub fn capture_from(
    generation: &str,
    argv: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    stdio: [RawFd; 3],
) -> io::Result<CapturedLaunch> {
    let kind = if argv.first().is_some_and(|arg| {
        arg.as_bytes().rsplit(|byte| *byte == b'/').next() == Some(b"oulipoly-plane".as_slice())
    }) {
        EntryKind::Gui
    } else {
        EntryKind::Cli
    };
    let mut descriptors = Vec::with_capacity(4);
    let mut stdio_present = [false; 3];
    for (index, present) in stdio_present.iter_mut().enumerate() {
        let duplicate = unsafe { libc::fcntl(stdio[index], libc::F_DUPFD_CLOEXEC, 3) };
        if duplicate >= 0 {
            *present = true;
            descriptors.push(unsafe { OwnedFd::from_raw_fd(duplicate) });
        } else if io::Error::last_os_error().raw_os_error() != Some(libc::EBADF) {
            return Err(io::Error::last_os_error());
        }
    }
    let cwd = unsafe { libc::open(c".".as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if cwd < 0 {
        return Err(io::Error::last_os_error());
    }
    descriptors.push(unsafe { OwnedFd::from_raw_fd(cwd) });
    let spec = InstalledLaunchSpec {
        protocol: PROTOCOL.into(),
        generation: generation.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        kind,
        args: argv
            .into_iter()
            .skip(1)
            .map(|arg| arg.as_bytes().to_vec())
            .collect(),
        environment: environment
            .into_iter()
            .map(|(key, value)| (key.as_bytes().to_vec(), value.as_bytes().to_vec()))
            .collect(),
        stdio_present,
    };
    validate(
        &spec,
        &descriptors
            .iter()
            .map(|fd| fd.as_raw_fd())
            .collect::<Vec<_>>(),
    )?;
    Ok(CapturedLaunch { spec, descriptors })
}

pub fn validate(spec: &InstalledLaunchSpec, descriptors: &[RawFd]) -> io::Result<()> {
    if spec.protocol != PROTOCOL
        || uuid::Uuid::parse_str(&spec.generation)
            .ok()
            .is_none_or(|id| id.to_string() != spec.generation)
        || uuid::Uuid::parse_str(&spec.request_id)
            .ok()
            .is_none_or(|id| id.to_string() != spec.request_id)
        || spec.args.len() > 256
        || spec.args.iter().any(|arg| arg.contains(&0))
        || spec.environment.len() > 512
        || descriptors.len()
            != spec
                .stdio_present
                .iter()
                .filter(|present| **present)
                .count()
                + 1
    {
        return Err(io::Error::other("invalid installed launch request"));
    }
    let mut names = HashSet::new();
    for (name, value) in &spec.environment {
        if name.is_empty()
            || name.contains(&0)
            || name.contains(&b'=')
            || value.contains(&0)
            || !names.insert(name)
            || name.starts_with(b"OULIPOLY_KERNEL_")
            || name.starts_with(b"LD_")
            || name.starts_with(b"DYLD_")
            || matches!(name.as_slice(), b"GLIBC_TUNABLES" | b"GCONV_PATH")
        {
            return Err(io::Error::other("unsafe installed launch environment"));
        }
    }
    let mut stdio_index = 0;
    for (index, fd) in descriptors.iter().enumerate() {
        let mut status: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(*fd, &mut status) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if index + 1 == descriptors.len() {
            if status.st_mode & libc::S_IFMT != libc::S_IFDIR {
                return Err(io::Error::other("installed launch cwd is not a directory"));
            }
        } else if !matches!(
            status.st_mode & libc::S_IFMT,
            libc::S_IFREG | libc::S_IFIFO | libc::S_IFCHR | libc::S_IFSOCK
        ) {
            return Err(io::Error::other(
                "unsupported installed launch standard descriptor",
            ));
        } else {
            while !spec.stdio_present[stdio_index] {
                stdio_index += 1;
            }
            let flags = unsafe { libc::fcntl(*fd, libc::F_GETFL) };
            if flags < 0
                || flags & libc::O_PATH != 0
                || (stdio_index == 0 && flags & libc::O_ACCMODE == libc::O_WRONLY)
                || (stdio_index != 0 && flags & libc::O_ACCMODE == libc::O_RDONLY)
            {
                return Err(io::Error::other(
                    "invalid installed launch standard descriptor direction",
                ));
            }
            stdio_index += 1;
        }
    }
    Ok(())
}

pub fn files_as_raw(descriptors: &[File]) -> Vec<RawFd> {
    descriptors.iter().map(AsRawFd::as_raw_fd).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::net::UnixStream;

    fn generation() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    #[test]
    fn exact_cli_bytes_and_cwd_fd_are_captured_before_any_runner_work() {
        let (input, _writer) = UnixStream::pair().unwrap();
        let (output, _reader) = UnixStream::pair().unwrap();
        let args = vec![
            OsString::from("agents"),
            OsString::from("-m"),
            OsString::from("model with space"),
            OsString::from_vec(vec![b'x', 0xff]),
        ];
        let captured = capture_from(
            &generation(),
            args,
            vec![(OsString::from("HOME"), OsString::from("/example"))],
            [input.as_raw_fd(), output.as_raw_fd(), -1],
        )
        .unwrap();
        assert_eq!(captured.spec.kind, EntryKind::Cli);
        assert_eq!(
            captured.spec.args,
            [
                b"-m".to_vec(),
                b"model with space".to_vec(),
                vec![b'x', 0xff]
            ]
        );
        assert_eq!(captured.spec.stdio_present, [true, true, false]);
        assert_eq!(
            captured.spec.environment,
            [(b"HOME".to_vec(), b"/example".to_vec())]
        );
        assert_eq!(captured.descriptors.len(), 3);
        let mut cwd_stat: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe {
                libc::fstat(
                    captured.descriptors.last().unwrap().as_raw_fd(),
                    &mut cwd_stat,
                )
            },
            0
        );
        assert_eq!(
            File::open(".").unwrap().metadata().unwrap().ino(),
            cwd_stat.st_ino
        );
        assert!(
            validate(
                &captured.spec,
                &captured
                    .descriptors
                    .iter()
                    .map(AsRawFd::as_raw_fd)
                    .collect::<Vec<_>>()
            )
            .is_ok()
        );
    }

    #[test]
    fn tty_fd_handoff_retains_terminal_and_window_size() {
        let mut master = -1;
        let mut slave = -1;
        let size = libc::winsize {
            ws_row: 37,
            ws_col: 91,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &size,
                )
            },
            0
        );
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let slave = unsafe { OwnedFd::from_raw_fd(slave) };
        let captured = capture_from(
            &generation(),
            vec![
                OsString::from("agents"),
                OsString::from("-m"),
                OsString::from("x"),
            ],
            vec![],
            [slave.as_raw_fd(); 3],
        )
        .unwrap();
        assert_eq!(captured.spec.stdio_present, [true; 3]);
        for fd in captured.descriptors.iter().take(3) {
            assert_eq!(unsafe { libc::isatty(fd.as_raw_fd()) }, 1);
            let mut observed: libc::winsize = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { libc::ioctl(fd.as_raw_fd(), libc::TIOCGWINSZ, &mut observed) },
                0
            );
            assert_eq!((observed.ws_row, observed.ws_col), (37, 91));
        }
        drop(master);
    }

    #[test]
    fn gui_entry_keeps_display_environment_and_absent_stdio() {
        let captured = capture_from(
            &generation(),
            vec![OsString::from("/usr/local/bin/oulipoly-plane")],
            vec![
                (
                    OsString::from("WAYLAND_DISPLAY"),
                    OsString::from("wayland-0"),
                ),
                (
                    OsString::from("XDG_RUNTIME_DIR"),
                    OsString::from("/run/user/1000"),
                ),
            ],
            [-1; 3],
        )
        .unwrap();
        assert_eq!(captured.spec.kind, EntryKind::Gui);
        assert!(captured.spec.args.is_empty());
        assert_eq!(captured.spec.stdio_present, [false; 3]);
        assert_eq!(captured.descriptors.len(), 1);
        assert_eq!(captured.spec.environment[0].1, b"wayland-0");
    }

    #[test]
    fn copied_environment_cannot_supply_kernel_authority() {
        let result = capture_from(
            &generation(),
            vec![OsString::from("agents")],
            vec![(
                OsString::from("OULIPOLY_KERNEL_CHILD_JOIN_FD_V1"),
                OsString::from("3"),
            )],
            [-1; 3],
        );
        assert!(result.is_err());
    }
}
