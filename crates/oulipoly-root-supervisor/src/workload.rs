//! Who a root's work runs as, and where it meets its owner.
//!
//! The intent declares it ([`Workload`]); nothing is chosen from whichever
//! euid happens to run the owner. Two declarations exist:
//!
//! * `host-root`: the owner, root PID 1 and every work PID 1 run as host
//!   root (custody), in new PID namespaces of the host user namespace.
//!   Every harness and Bash run is started by its work PID 1 as the named
//!   host `user` (its uid, primary gid and host supplementary groups), with
//!   no user namespace and no `no_new_privs`, so that user's normal host
//!   capabilities (sudo, setuid programs) stay as host policy has them. The
//!   owner must be euid 0 and the user must resolve to a non-zero uid;
//!   otherwise the request is refused before anything is launched. The
//!   store stays owner-private; the work's IPC is in `ipc_dir` (see
//!   `prepare_ipc`).
//! * `unprivileged-userns`: the owner is any non-root user and isolates
//!   with a new user namespace mapping only itself; work runs as that
//!   mapped user. It does not stand for host-root semantics, and it cannot
//!   name a different identity.
//!
//! Owner IPC grants nothing by uid alone: a peer is admitted by its uid
//! **and** its membership of the exact PID namespace of a live work of
//! this owner (see the `bash` module and the harness socket connection).

use std::ffi::{CStr, CString};
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::sys::Isolation;

/// The declared identity and placement of a root's work.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "isolation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Workload {
    /// Host-root owner custody; work runs as the host `user`.
    HostRoot {
        /// Host user name every harness and Bash run is started as.
        user: String,
        /// Absolute path of the work's IPC directory, created fresh by the
        /// owner (root, `0711`) with the harness socket directory `acp`
        /// owned by `user` (`0700`) and the Bash ingress socket owned by
        /// `user` (`0600`).
        ipc_dir: String,
    },
    /// Unprivileged user-namespace isolation of a non-root owner. Not host
    /// root semantics. A struct variant so that an unknown field (a
    /// nominated `user`) is refused rather than ignored.
    UnprivilegedUserns {},
}

/// A resolved host identity work is started as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    /// Supplementary groups as the host's group database has them.
    pub groups: Vec<u32>,
}

impl Identity {
    pub(crate) fn to_json(&self) -> Value {
        json!({ "user": self.user, "uid": self.uid, "gid": self.gid, "groups": self.groups })
    }
}

/// A declaration checked against this process and the host.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub isolation: Isolation,
    /// `Some` under `host-root`: who work runs as.
    pub identity: Option<Identity>,
    /// Where the harness sockets and the Bash ingress are.
    pub ipc_dir: PathBuf,
}

impl Resolved {
    /// The uid an owner IPC peer must have, besides its namespace.
    pub(crate) fn peer_uid(&self) -> u32 {
        // SAFETY: geteuid has no preconditions.
        self.identity
            .as_ref()
            .map_or_else(|| unsafe { libc::geteuid() }, |identity| identity.uid)
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "isolation": self.isolation.label(),
            "identity": self.identity.as_ref().map(Identity::to_json),
            "ipc_dir": self.ipc_dir,
        })
    }
}

impl Workload {
    /// Checks that do not depend on this process or the host.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Self::HostRoot { user, ipc_dir } = self {
            if user.is_empty() || user.contains('\0') {
                return Err("workload user is empty".to_owned());
            }
            if !ipc_dir.starts_with('/') {
                return Err("workload ipc_dir must be absolute".to_owned());
            }
        }
        Ok(())
    }

    /// Checks the declaration against this process's euid and resolves the
    /// host user. Refuses rather than substitute anything.
    pub fn resolve(&self, store: &Path) -> Result<Resolved, String> {
        self.validate()?;
        // SAFETY: geteuid has no preconditions.
        let euid = unsafe { libc::geteuid() };
        match self {
            Self::HostRoot { user, ipc_dir } => {
                if euid != 0 {
                    return Err(format!(
                        "workload-refused: host-root declared but the owner is euid {euid}"
                    ));
                }
                let identity = lookup(user)?;
                if identity.uid == 0 {
                    return Err(format!("workload-refused: user {user} is uid 0"));
                }
                Ok(Resolved {
                    isolation: Isolation::HostRootPidns,
                    identity: Some(identity),
                    ipc_dir: PathBuf::from(ipc_dir),
                })
            }
            Self::UnprivilegedUserns {} => {
                if euid == 0 {
                    return Err(
                        "workload-refused: unprivileged-userns declared but the owner is euid 0"
                            .to_owned(),
                    );
                }
                Ok(Resolved {
                    isolation: Isolation::UnprivilegedUserns,
                    identity: None,
                    ipc_dir: store.to_path_buf(),
                })
            }
        }
    }
}

/// The host's passwd and group entries for `user`.
fn lookup(user: &str) -> Result<Identity, String> {
    let name = CString::new(user).map_err(|_| "workload user has NUL".to_owned())?;
    let mut buffer = vec![0u8; 16 * 1024];
    // SAFETY: zeroed passwd is a valid out-parameter for getpwnam_r.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwnam_r writes into `entry` and `buffer`, both ours.
    let rc = unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            &raw mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut found,
        )
    };
    if rc != 0 {
        return Err(format!(
            "workload-refused: user {user}: {}",
            io::Error::from_raw_os_error(rc)
        ));
    }
    if found.is_null() {
        return Err(format!("workload-refused: no host user {user}"));
    }
    // SAFETY: pw_name points into `buffer`, NUL-terminated.
    let resolved = unsafe { CStr::from_ptr(entry.pw_name) };
    if resolved.to_bytes() != user.as_bytes() {
        return Err(format!(
            "workload-refused: user {user} resolved differently"
        ));
    }
    let (uid, gid) = (entry.pw_uid, entry.pw_gid);
    let mut count: libc::c_int = 64;
    let mut groups: Vec<libc::gid_t> = vec![0; 64];
    loop {
        let capacity = count;
        // SAFETY: getgrouplist writes at most `count` gids into `groups`.
        let rc =
            unsafe { libc::getgrouplist(name.as_ptr(), gid, groups.as_mut_ptr(), &raw mut count) };
        if rc >= 0 {
            groups.truncate(usize::try_from(count).unwrap_or(0));
            break;
        }
        if count <= capacity {
            return Err(format!("workload-refused: groups of {user} unreadable"));
        }
        groups.resize(usize::try_from(count).unwrap_or(0), 0);
    }
    Ok(Identity {
        user: user.to_owned(),
        uid,
        gid,
        groups,
    })
}

/// The harness socket directory inside the IPC directory.
pub(crate) const ACP_DIR: &str = "acp";

/// Makes (on creation) or checks (on recovery) the IPC directory of a
/// `host-root` root. Created: `ipc_dir` must not exist; it is made `0711`
/// owned by this owner (host root), with `acp` made `0700` and owned by the
/// work identity. Recovered: `ipc_dir` must be a directory owned by this
/// owner and not group/other writable, and `acp` a directory owned by the
/// work identity; nothing existing is changed. Only paths this function
/// creates are ever chowned. Without an identity it does nothing.
pub(crate) fn prepare_ipc(resolved: &Resolved, created: bool) -> Result<(), String> {
    let Some(identity) = &resolved.identity else {
        return Ok(());
    };
    let ipc = &resolved.ipc_dir;
    let acp = ipc.join(ACP_DIR);
    if created {
        mkdir_new(ipc, 0o711)?;
        mkdir_new(&acp, 0o700)?;
        std::os::unix::fs::lchown(&acp, Some(identity.uid), Some(identity.gid))
            .map_err(|error| format!("ipc acp chown: {error}"))?;
        return Ok(());
    }
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    let meta = fs::symlink_metadata(ipc).map_err(|error| format!("ipc_dir: {error}"))?;
    if !meta.is_dir() || meta.uid() != euid || meta.mode() & 0o022 != 0 {
        return Err(format!(
            "ipc_dir is not an owner directory (uid {}, mode {:o})",
            meta.uid(),
            meta.mode() & 0o7777
        ));
    }
    let meta = fs::symlink_metadata(&acp).map_err(|error| format!("ipc acp: {error}"))?;
    if !meta.is_dir() || meta.uid() != identity.uid {
        return Err(format!(
            "ipc acp is not the work identity's directory (uid {})",
            meta.uid()
        ));
    }
    Ok(())
}

fn mkdir_new(path: &Path, mode: u32) -> Result<(), String> {
    fs::DirBuilder::new()
        .mode(mode)
        .create(path)
        .map_err(|error| format!("{}: {error} (created fresh only)", path.display()))?;
    // The umask may have narrowed the mode.
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Hands a socket this owner just bound in its own IPC directory to the
/// work identity (`0600`), so only that identity (and host root) connects.
pub(crate) fn hand_socket(resolved: &Resolved, path: &Path) -> Result<(), String> {
    let Some(identity) = &resolved.identity else {
        return Ok(());
    };
    std::os::unix::fs::lchown(path, Some(identity.uid), Some(identity.gid))
        .and_then(|()| fs::set_permissions(path, fs::Permissions::from_mode(0o600)))
        .map_err(|error| format!("ingress socket handover: {error}"))
}

/// A process's credentials as `/proc/<pid>/status` shows them now: real,
/// effective, saved and filesystem uid and gid, and `NoNewPrivs`. An
/// observation at one moment, not custody.
pub(crate) fn observe(pid: i32) -> Value {
    let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) else {
        return Value::Null;
    };
    let field = |name: &str| -> Value {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(|rest| {
                Value::Array(
                    rest.split_whitespace()
                        .filter_map(|id| id.parse::<u64>().ok())
                        .map(Value::from)
                        .collect(),
                )
            })
            .unwrap_or(Value::Null)
    };
    json!({
        "uid": field("Uid:"),
        "gid": field("Gid:"),
        "no_new_privs": field("NoNewPrivs:"),
        "observed": "proc-status",
    })
}

/// The unprivileged declaration resolved for this (non-root) test process.
#[cfg(test)]
pub(crate) fn unprivileged_for_tests(store: &Path) -> std::sync::Arc<Resolved> {
    std::sync::Arc::new(
        Workload::UnprivilegedUserns {}
            .resolve(store)
            .expect("unit tests run unprivileged"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_of_self() -> Identity {
        // SAFETY: getuid/getgid have no preconditions.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        Identity {
            user: "self".into(),
            uid,
            gid,
            groups: vec![gid],
        }
    }

    #[test]
    fn declarations_are_explicit_and_name_no_identity_when_unprivileged() {
        let parse = |value: Value| serde_json::from_value::<Workload>(value);
        assert_eq!(
            parse(json!({ "isolation": "unprivileged-userns" })).unwrap(),
            Workload::UnprivilegedUserns {}
        );
        assert!(parse(json!({ "isolation": "unprivileged-userns", "user": "root" })).is_err());
        assert!(parse(json!({ "isolation": "host-root", "user": "nes" })).is_err());
        assert!(parse(json!({ "user": "nes", "ipc_dir": "/i" })).is_err());
        let host =
            parse(json!({ "isolation": "host-root", "user": "nes", "ipc_dir": "i" })).unwrap();
        assert!(host.validate().unwrap_err().contains("absolute"));
        let empty =
            parse(json!({ "isolation": "host-root", "user": "", "ipc_dir": "/i" })).unwrap();
        assert!(empty.validate().is_err());
    }

    /// These tests run unprivileged: a host-root declaration is refused
    /// before any lookup or effect, and the unprivileged one keeps its IPC
    /// in the private store.
    #[test]
    fn resolution_follows_the_declaration_not_the_euid() {
        let store = Path::new("/store");
        let host = Workload::HostRoot {
            user: "root".into(),
            ipc_dir: "/ipc".into(),
        };
        let refused = host.resolve(store).unwrap_err();
        assert!(
            refused.contains("host-root declared but the owner is euid"),
            "{refused}"
        );
        let resolved = Workload::UnprivilegedUserns {}.resolve(store).unwrap();
        assert_eq!(resolved.isolation, Isolation::UnprivilegedUserns);
        assert!(resolved.identity.is_none());
        assert_eq!(resolved.ipc_dir, store);
        // SAFETY: geteuid has no preconditions.
        assert_eq!(resolved.peer_uid(), unsafe { libc::geteuid() });
    }

    #[test]
    fn lookup_refuses_unknown_users() {
        assert!(
            lookup("oulipoly-no-such-user")
                .unwrap_err()
                .contains("no host user")
        );
    }

    /// The host-root IPC layout, handed to this process's own identity.
    #[test]
    fn ipc_is_made_fresh_then_only_checked_and_never_taken_over() {
        let base = std::env::temp_dir().join(format!("workload-ipc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let ipc = base.join("ipc");
        let resolved = Resolved {
            isolation: Isolation::HostRootPidns,
            identity: Some(identity_of_self()),
            ipc_dir: ipc.clone(),
        };
        prepare_ipc(&resolved, true).unwrap();
        let mode = |path: &Path| fs::symlink_metadata(path).unwrap().mode() & 0o7777;
        assert_eq!(mode(&ipc), 0o711);
        assert_eq!(mode(&ipc.join(ACP_DIR)), 0o700);
        // Created fresh only: an existing directory is not adopted.
        assert!(
            prepare_ipc(&resolved, true)
                .unwrap_err()
                .contains("created fresh only")
        );
        // A recovery checks and changes nothing.
        prepare_ipc(&resolved, false).unwrap();
        fs::set_permissions(&ipc, fs::Permissions::from_mode(0o731)).unwrap();
        assert!(
            prepare_ipc(&resolved, false)
                .unwrap_err()
                .contains("not an owner directory")
        );
        assert_eq!(mode(&ipc), 0o731);
        fs::set_permissions(&ipc, fs::Permissions::from_mode(0o711)).unwrap();
        // A symlink in place of the socket directory is refused, not followed.
        fs::remove_dir(ipc.join(ACP_DIR)).unwrap();
        std::os::unix::fs::symlink(&base, ipc.join(ACP_DIR)).unwrap();
        assert!(prepare_ipc(&resolved, false).is_err());
        // The ingress socket is handed over 0600.
        let socket = ipc.join("bash.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        hand_socket(&resolved, &socket).unwrap();
        assert_eq!(mode(&socket), 0o600);
        fs::remove_dir_all(&base).unwrap();
    }
}
