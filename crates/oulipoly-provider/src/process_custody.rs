//! Retained direct Child ownership and Linux process-group containment.
//! Roles: orchestration, validator.
use crate::custody::{OperationCustody, ProcessIdentity};
use crate::generated::ProcessStatus;
use std::process::{Child, ExitStatus};

const GROUP_DEATH_OBSERVATION_BOUND: std::time::Duration = std::time::Duration::from_secs(1);
const GROUP_DEATH_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(2);

pub(crate) struct OwnedChild {
    child: Child,
    #[cfg(target_os = "linux")]
    remote: Option<oulipoly_core::launch_custody::RemoteStatus>,
    #[cfg(target_os = "linux")]
    published_receipt: Option<std::sync::Arc<oulipoly_core::launch_custody::PublishedTreeReceipt>>,
    remote_force_delivered: bool,
    tree_termination_proven: bool,
    reaped: bool,
    custody: Option<OperationCustody>,
    confined: bool,
    #[cfg(all(test, target_os = "linux"))]
    identity_fault: u8,
}
impl std::ops::Deref for OwnedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}
impl OwnedChild {
    pub fn new(child: Child, custody: Option<OperationCustody>) -> Self {
        if let Some(c) = &custody {
            let mut r = c.0.lock().unwrap_or_else(|e| e.into_inner());
            r.spawned = true;
            r.exact_process_identity = identity(child.id()).ok();
            #[cfg(all(test, target_os = "linux"))]
            if IDENTITY_FAULT.get() == 1 {
                r.exact_process_identity = None;
            }
        }
        Self {
            #[cfg(all(test, target_os = "linux"))]
            identity_fault: IDENTITY_FAULT.get(),
            child,
            #[cfg(target_os = "linux")]
            remote: None,
            #[cfg(target_os = "linux")]
            published_receipt: None,
            remote_force_delivered: false,
            tree_termination_proven: false,
            reaped: false,
            confined: custody.as_ref().is_some_and(|c| !c.1) && containment_supported(),
            custody,
        }
    }
    #[cfg(target_os = "linux")]
    pub fn with_remote(
        mut self,
        remote: Option<oulipoly_core::launch_custody::RemoteStatus>,
    ) -> Self {
        self.remote = remote;
        self
    }
    #[cfg(target_os = "linux")]
    pub fn with_published_receipt(
        mut self,
        receipt: oulipoly_core::launch_custody::PublishedTreeReceipt,
    ) -> Self {
        let receipt = std::sync::Arc::new(receipt);
        if let Some(custody) = &self.custody {
            *custody.2.lock().unwrap_or_else(|e| e.into_inner()) = Some(receipt.clone());
        }
        self.published_receipt = Some(receipt);
        self
    }
    #[cfg(unix)]
    pub fn signal_remote(&mut self, signal: i32) -> Option<std::io::Result<bool>> {
        #[cfg(target_os = "linux")]
        if let Some(remote) = &self.remote {
            let result = remote.signal(signal);
            if signal == libc::SIGKILL {
                self.remote_force_delivered = matches!(result, Ok(true));
            }
            return Some(result);
        }
        let _ = signal;
        None
    }
    pub fn force_was_delivered(&self, admitted: bool) -> bool {
        #[cfg(target_os = "linux")]
        if self.remote.is_some() {
            return admitted && self.remote_force_delivered;
        }
        let _ = self.remote_force_delivered;
        admitted
    }
    fn current_identity(&self) -> std::io::Result<ProcessIdentity> {
        #[cfg(all(test, target_os = "linux"))]
        if self.identity_fault != 0 {
            return Err(std::io::Error::other("injected identity read unavailable"));
        }
        identity(self.child.id())
    }
    pub fn is_reaped(&self) -> bool {
        self.reaped
    }
    pub fn tree_termination_proven(&self) -> bool {
        self.tree_termination_proven
            || self.custody.as_ref().is_some_and(|custody| {
                custody
                    .0
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .process_tree_terminated
            })
    }
    #[cfg(not(unix))]
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            return self.wait().map(Some);
        }
        Ok(None)
    }
    pub fn uncertain(&self) {
        if let Some(c) = &self.custody {
            c.0.lock().unwrap_or_else(|e| e.into_inner()).uncertain = true;
        }
    }
    pub fn can_signal_group(&self) -> bool {
        self.check_signal_group().is_ok()
    }
    #[cfg(unix)]
    pub fn has_actor_custody(&self) -> bool {
        self.custody.is_some()
    }
    pub fn check_signal_group(&self) -> Result<(), (&'static str, std::io::Error)> {
        #[cfg(unix)]
        {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id() as libc::id_t,
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                // Capture errno before locks or other cleanup can overwrite it.
                let error = std::io::Error::last_os_error();
                self.uncertain();
                return Err(("cleanup_waitid_wnowait", error));
            }
            if let Some(c) = &self.custody {
                let r = c.0.lock().unwrap_or_else(|e| e.into_inner());
                if r.leader_reaped
                    || !r
                        .exact_process_identity
                        .as_ref()
                        .is_some_and(|p| self.current_identity().ok().as_ref() == Some(p))
                {
                    return Err((
                        "cleanup_identity",
                        std::io::Error::other("identity unavailable"),
                    ));
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err((
                "cleanup_unsupported",
                std::io::Error::other("unsupported group signaling"),
            ))
        }
    }
    pub fn cancellation(&self) {
        if let Some(c) = &self.custody {
            c.0.lock()
                .unwrap_or_else(|e| e.into_inner())
                .host_cancellation_requested = true;
        }
    }
    pub fn forced(&self) {
        if !self.force_was_delivered(true) {
            return;
        }
        if let Some(c) = &self.custody {
            c.0.lock().unwrap_or_else(|e| e.into_inner()).force_killed = true;
        }
    }
    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        #[cfg(target_os = "linux")]
        if !self.reaped
            && let Some(operation) = &self.custody
        {
            crate::custody::durable::retain_terminal(operation, self.child.id())?;
        }
        let result = self.child.wait();
        self.reaped |= result.is_ok();
        #[cfg(target_os = "linux")]
        let result = result.and_then(|owner_status| match &self.remote {
            Some(remote) if owner_status.success() => remote.wait_status().inspect(|_| {
                self.tree_termination_proven = true;
            }),
            Some(_) => Err(std::io::Error::other("command custodian failed")),
            None => Ok(owner_status),
        });
        if let Some(c) = &self.custody {
            let mut r = c.0.lock().unwrap_or_else(|e| e.into_inner());
            match &result {
                Ok(status) => {
                    r.leader_reaped = true;
                    r.process_status = Some(status_value(*status));
                    #[cfg(target_os = "linux")]
                    if self.remote.is_some() {
                        r.process_tree_terminated = true;
                    }
                    #[cfg(target_os = "linux")]
                    if let Some(receipt) = &self.published_receipt {
                        match receipt.wait() {
                            Ok(_) => {
                                r.process_tree_terminated = true;
                                self.tree_termination_proven = true;
                            }
                            Err(_) => r.uncertain = true,
                        }
                    }
                }
                Err(_) => r.uncertain = true,
            }
        }
        result
    }
    // Called after group SIGKILL, while the direct leader remains waitable. The
    // seccomp filter is inherited and irreversible: no descendant can escape
    // this group or create/join a PID namespace. Zombies cannot produce effects.
    pub fn confirm_group_dead(&self, signal_ok: bool) {
        // Remote group signals are acknowledgements, not tree certificates.
        // Only the owner's ECHILD-backed status consumed in wait certifies it.
        #[cfg(target_os = "linux")]
        if self.remote.is_some() {
            return;
        }
        let Some(c) = &self.custody else {
            return;
        };
        let mut r = c.0.lock().unwrap_or_else(|e| e.into_inner());
        if !self.confined || !signal_ok || r.exact_process_identity.is_none() {
            return;
        }
        let expected = r.exact_process_identity.as_ref().unwrap();
        if self.current_identity().ok().as_ref() != Some(expected) {
            r.uncertain = true;
            return;
        }
        let start = std::time::Instant::now();
        while start.elapsed() < GROUP_DEATH_OBSERVATION_BOUND {
            match group_dead(self.child.id()) {
                Ok(true) => {
                    r.process_tree_terminated = true;
                    return;
                }
                Ok(false) => std::thread::sleep(GROUP_DEATH_POLL_INTERVAL),
                Err(_) => {
                    r.uncertain = true;
                    return;
                }
            }
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(c) = &self.custody {
            let mut r = c.0.lock().unwrap_or_else(|e| e.into_inner());
            if !r.leader_reaped {
                r.uncertain = true;
            }
        }
        // No certificate is minted here. All receipt fields above are explicit
        // direct-owner observations in normal settlement, not emergency Drop.
    }
}
fn status_value(status: ExitStatus) -> ProcessStatus {
    if let Some(code) = status.code() {
        return ProcessStatus::Exited { code };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return ProcessStatus::SignalTerminated { signal };
        }
    }
    ProcessStatus::Unknown
}
#[cfg(target_os = "linux")]
fn proc_stat(pid: u32) -> std::io::Result<(char, i64, i64)> {
    let stat = std::fs::read(format!("/proc/{pid}/stat"))?;
    parse_proc_stat(&stat)
}
#[cfg(target_os = "linux")]
fn parse_proc_stat(stat: &[u8]) -> std::io::Result<(char, i64, i64)> {
    // Linux comm is an opaque byte string (and can contain ')'). Only the
    // kernel-owned numeric/state suffix has a text contract.
    let end = stat
        .iter()
        .rposition(|byte| *byte == b')')
        .ok_or_else(|| std::io::Error::other("invalid process stat"))?;
    let suffix = std::str::from_utf8(&stat[end + 1..])
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let fields: Vec<_> = suffix.split_whitespace().collect();
    let invalid = || std::io::Error::other("invalid process identity");
    Ok((
        fields
            .first()
            .and_then(|s| s.chars().next())
            .ok_or_else(invalid)?,
        fields
            .get(2)
            .and_then(|s| s.parse().ok())
            .ok_or_else(invalid)?,
        fields
            .get(19)
            .and_then(|s| s.parse().ok())
            .ok_or_else(invalid)?,
    ))
}
#[cfg(target_os = "linux")]
pub(crate) fn identity(pid: u32) -> std::io::Result<ProcessIdentity> {
    let (_, _, ticks) = proc_stat(pid)?;
    Ok(ProcessIdentity {
        os_pid: pid.into(),
        os_boot_id: std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .into(),
        os_pid_starttime_ticks: ticks,
    })
}
#[cfg(not(target_os = "linux"))]
fn identity(_: u32) -> std::io::Result<ProcessIdentity> {
    Err(std::io::Error::other("unsupported exact process custody"))
}
#[cfg(target_os = "linux")]
pub(crate) fn group_dead(group: u32) -> std::io::Result<bool> {
    require_visible_proc_tasks()?;
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if !process_group_tasks_dead(pid, group)? {
            return Ok(false);
        }
    }
    Ok(true)
}
// Enumeration must expose task IDs even when stat access is denied. A procfs
// view that hides owned entries cannot establish terminality by omission.
#[cfg(target_os = "linux")]
fn require_visible_proc_tasks() -> std::io::Result<()> {
    let own = std::fs::read("/proc/self/stat")?;
    let pid = own.split(|byte| *byte == b' ').next().unwrap_or_default();
    if pid != std::process::id().to_string().as_bytes() {
        return Err(std::io::Error::other(
            "procfs PID namespace does not match custody",
        ));
    }
    use std::os::fd::AsRawFd;
    let proc = std::fs::File::open("/proc")?;
    // fdinfo identifies the visible mount, even with namespace procfs mounts
    // stacked on an older /proc. A lower mount's options are not its authority.
    let fdinfo = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", proc.as_raw_fd()))?;
    let mount_id = fdinfo
        .lines()
        .find_map(|line| line.strip_prefix("mnt_id:").map(str::trim))
        .ok_or_else(|| std::io::Error::other("procfs mount identity is unavailable"))?;
    let mounts = std::fs::read("/proc/self/mountinfo")?;
    if !proc_mount_exposes_task_ids(&mounts, mount_id.as_bytes()) {
        return Err(std::io::Error::other(
            "procfs task enumeration visibility is unavailable",
        ));
    }
    Ok(())
}
#[cfg(target_os = "linux")]
fn proc_mount_exposes_task_ids(mounts: &[u8], mount_id: &[u8]) -> bool {
    // Paths in unrelated mount records can also contain non-UTF8 bytes. Select
    // the literal /proc mount before decoding any of its kernel option tokens.
    let Some(line) = mounts
        .split(|byte| *byte == b'\n')
        .find(|line| mount_fields(line).first() == Some(&mount_id))
    else {
        return false;
    };
    let fields = mount_fields(line);
    let Some(separator) = fields.iter().position(|field| *field == b"-") else {
        return false;
    };
    if fields.get(3) != Some(&b"/".as_slice())
        || fields.get(4) != Some(&b"/proc".as_slice())
        || fields.get(separator + 1) != Some(&b"proc".as_slice())
    {
        return false;
    }
    fields.get(separator + 3).is_some_and(|options| {
        options.split(|byte| *byte == b',').all(|option| {
            matches!(
                option.strip_prefix(b"hidepid="),
                None | Some(b"0" | b"off" | b"1" | b"noaccess")
            )
        })
    })
}
#[cfg(target_os = "linux")]
fn mount_fields(line: &[u8]) -> Vec<&[u8]> {
    line.split(u8::is_ascii_whitespace)
        .filter(|field| !field.is_empty())
        .collect()
}
// A zombie thread-group leader is not evidence that every sibling thread has
// completed exit. Inspect tasks too; ENOENT means the exact observed task left.
#[cfg(target_os = "linux")]
fn process_group_tasks_dead(pid: u32, group: u32) -> std::io::Result<bool> {
    // getpgid is a kernel membership query, independent of procfs stat access
    // and comm encoding. Never discard an observation error to guess ownership.
    // With the original leader retained and members confined, an excluded PID
    // cannot be an owned task that escaped this group. ESRCH means that task
    // has exited; PID reuse cannot replace the pinned group identity.
    if !kernel_group_member(pid, group)? {
        return Ok(true);
    }
    let tasks = match std::fs::read_dir(format!("/proc/{pid}/task")) {
        Ok(tasks) => tasks,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    for task in tasks {
        if !task_group_dead(&task?.path().join("stat"), group)? {
            return Ok(false);
        }
    }
    Ok(true)
}
#[cfg(target_os = "linux")]
fn kernel_group_member(pid: u32, group: u32) -> std::io::Result<bool> {
    let pgid = unsafe { libc::getpgid(pid as libc::pid_t) };
    if pgid >= 0 {
        return Ok(pgid as u32 == group);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }
    Err(error)
}
#[cfg(target_os = "linux")]
fn task_group_dead(path: &std::path::Path, group: u32) -> std::io::Result<bool> {
    let stat = match std::fs::read(path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    let (state, pgid, _) = parse_proc_stat(&stat)?;
    Ok(pgid != i64::from(group) || matches!(state, 'Z' | 'X'))
}
#[cfg(not(target_os = "linux"))]
fn group_dead(_: u32) -> std::io::Result<bool> {
    Ok(false)
}
pub(crate) fn containment_supported() -> bool {
    cfg!(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))
}

/// Original-tree launches retain a separate wait owner instead of relying on
/// inherited syscall restrictions. Restrictions inherited from an older lane
/// cannot be undone by fork/exec; refuse before effects rather than claim sudo
/// semantics that this process cannot provide. This also runs in the exec child.
#[cfg(target_os = "linux")]
pub(crate) fn require_unrestricted_context() -> std::io::Result<()> {
    if unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } != 0
    {
        return Err(std::io::Error::other(
            "original-tree provider inherited NNP or seccomp restriction",
        ));
    }
    Ok(())
}

pub(crate) fn configure_containment(command: &mut std::process::Command, enabled: bool) {
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    if enabled {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(install_containment);
        }
    }
    #[cfg(not(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )))]
    let _ = (command, enabled);
}
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn install_containment() -> std::io::Result<()> {
    // Deny group/namespace escape. clone3 returns ENOSYS so libc can use clone;
    // clone is filtered for CLONE_NEWPID. This is custody containment, not a
    // general sandbox. It applies only to explicit allocated-attempt clients.
    const LD: u16 = 0x20;
    const JEQ: u16 = 0x15;
    const JSET: u16 = 0x45;
    const RET: u16 = 0x06;
    const ALLOW: u32 = 0x7fff0000;
    const DENY: u32 = 0x00050000 | libc::EPERM as u32;
    #[cfg(target_arch = "x86_64")]
    const ARCH: u32 = 0xc000003e;
    #[cfg(target_arch = "aarch64")]
    const ARCH: u32 = 0xc00000b7;
    let ins = |code, jt, jf, k| libc::sock_filter { code, jt, jf, k };
    let mut filter = vec![
        ins(LD, 0, 0, 4),
        ins(JEQ, 1, 0, ARCH),
        ins(RET, 0, 0, DENY),
        ins(LD, 0, 0, 0),
    ];
    // x32 syscall numbers must not bypass the native syscall checks.
    filter.extend([ins(JSET, 0, 1, 0x40000000), ins(RET, 0, 0, DENY)]);
    for syscall in [
        libc::SYS_setsid,
        libc::SYS_setpgid,
        libc::SYS_unshare,
        libc::SYS_setns,
    ] {
        filter.extend([ins(JEQ, 0, 1, syscall as u32), ins(RET, 0, 0, DENY)]);
    }
    filter.extend([
        ins(JEQ, 0, 1, libc::SYS_clone3 as u32),
        ins(RET, 0, 0, 0x00050000 | libc::ENOSYS as u32),
        ins(JEQ, 0, 3, libc::SYS_clone as u32),
        ins(LD, 0, 0, 16),
        ins(JSET, 0, 1, libc::CLONE_NEWPID as u32),
        ins(RET, 0, 0, DENY),
        ins(RET, 0, 0, ALLOW),
    ]);
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0
        || unsafe { libc::prctl(libc::PR_SET_SECCOMP, 2, &program) } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use crate::custody::{AttemptActorCustody, ProviderOperation};
    use crate::process::{ProcessCommand, ProcessLimits, ProcessRunner};
    use crate::testkit::{FakeProvider, FakeProviderMode, LeakProbe};
    use std::path::PathBuf;
    use std::time::Duration;
    use uuid::Uuid;

    #[test]
    fn direct_owner_attests_normal_timeout_cancellation_and_forced_tree_settlement() {
        let fake = FakeProvider::compile(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/provider_client/fake_provider.rs"),
        );
        for (mode, cancel, operation) in [
            (FakeProviderMode::StdinEof, false, "describe"),
            (FakeProviderMode::ChildGrandchild, false, "policy.evaluate"),
            (FakeProviderMode::ChildGrandchild, true, "launch"),
            (
                FakeProviderMode::SigtermResistantChildGrandchild,
                true,
                "terminal.classify",
            ),
        ] {
            let custody = AttemptActorCustody::new(Uuid::new_v4());
            let guard = custody.begin(operation);
            let token = crate::process::CancellationToken::new();
            if cancel {
                token.cancel_after(Duration::from_millis(250));
            }
            let probe = LeakProbe::new();
            let result = ProcessRunner::new(ProcessLimits {
                custody: Some(guard.0.clone()),
                cancellation: Some(token),
                timeout: Duration::from_millis(500),
                kill_after_grace: Duration::from_millis(25),
                ..ProcessLimits::default()
            })
            .run(
                ProcessCommand::new(fake.path()).arg(operation),
                b"{}".to_vec(),
                mode.env_with_probe(&probe),
            );
            drop(guard);
            let receipts = custody.receipts();
            let receipt = &receipts[0];
            assert_eq!(
                receipt.operation,
                ProviderOperation::from_subcommand(operation)
            );
            assert!(
                receipt.spawned && receipt.leader_reaped && receipt.effect_incapable(),
                "{receipt:?}; {result:?}"
            );
            assert_eq!(receipt.host_cancellation_requested, cancel);
            if operation == "terminal.classify" {
                assert!(receipt.force_killed);
            }
            if operation != "describe" {
                probe.assert_no_descendants();
            }
        }
    }

    #[test]
    fn confinement_prevents_descendant_group_escape() {
        let custody = AttemptActorCustody::new(Uuid::new_v4());
        let guard = custody.begin("launch");
        let result = ProcessRunner::new(ProcessLimits { custody: Some(guard.0.clone()), ..ProcessLimits::default() })
            .run(ProcessCommand::new("/usr/bin/python3").arg("-c").arg("import os\ntry: os.setsid()\nexcept PermissionError: print('escape denied')\nelse: raise Exception('escaped')"),vec![],Vec::<(String,String)>::new()).unwrap();
        drop(guard);
        assert!(result.status.exited_successfully());
        assert!(custody.receipts()[0].effect_incapable());
        assert_eq!(result.stdout.bytes, b"escape denied\n");
    }

    #[test]
    fn pre_spawn_failure_is_not_a_spawn_and_pending_or_uncertain_is_never_safe() {
        let custody = AttemptActorCustody::new(Uuid::new_v4());
        let guard = custody.begin("describe");
        assert!(!custody.receipts()[0].effect_incapable());
        assert!(
            ProcessRunner::new(ProcessLimits {
                custody: Some(guard.0.clone()),
                ..ProcessLimits::default()
            })
            .run(
                ProcessCommand::new("/nonexistent/lt02-provider"),
                vec![],
                Vec::<(String, String)>::new()
            )
            .is_err()
        );
        drop(guard);
        assert!(!custody.receipts()[0].spawned);
        assert!(custody.receipts()[0].effect_incapable());
        let guard = custody.begin("policy.evaluate");
        guard.0.0.lock().unwrap().uncertain = true;
        drop(guard);
        assert!(!custody.receipts()[1].effect_incapable());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod uncertainty_tests {
    use super::*;
    use crate::custody::AttemptActorCustody;
    #[test]
    fn stolen_wait_and_missing_identity_never_attest_or_signal_recycled_group() {
        for stolen_wait in [false, true] {
            let custody = AttemptActorCustody::new(uuid::Uuid::new_v4());
            let guard = custody.begin("policy.evaluate");
            let child = std::process::Command::new("/bin/sleep")
                .arg("0.02")
                .spawn()
                .unwrap();
            let mut child = OwnedChild::new(child, Some(guard.0.clone()));
            if stolen_wait {
                let mut status = 0;
                assert_eq!(
                    unsafe { libc::waitpid(child.id() as i32, &mut status, 0) },
                    child.id() as i32
                );
                assert!(child.wait().is_err());
            } else {
                guard.0.0.lock().unwrap().exact_process_identity = None;
                // Direct Child cleanup, never group cleanup by a naked numeric PID.
                child.child.kill().unwrap();
                child.wait().unwrap();
            }
            assert!(!child.can_signal_group());
            drop(child);
            drop(guard);
            assert!(!custody.receipts()[0].effect_incapable());
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod pty_tests {
    use crate::custody::AttemptActorCustody;
    use crate::process::{ProcessCommand, ProcessLimits, ProcessRunner};
    #[test]
    fn owned_pty_descendant_settles_before_receipt() {
        let custody = AttemptActorCustody::new(uuid::Uuid::new_v4());
        let guard = custody.begin("launch");
        let script = "import os,pty,time,signal\nm,s=pty.openpty()\np=os.fork()\nif p==0:\n os.close(m)\n signal.signal(signal.SIGTERM,signal.SIG_IGN)\n os.write(s,b'pty ready\\n')\n time.sleep(30)\nelse:\n os.close(s)\n assert b'pty ready' in os.read(m,100)\n print('owned PTY ready',flush=True)\n time.sleep(30)\n";
        let error = ProcessRunner::new(ProcessLimits {
            custody: Some(guard.0.clone()),
            timeout: std::time::Duration::from_millis(300),
            kill_after_grace: std::time::Duration::from_millis(25),
            ..ProcessLimits::default()
        })
        .run(
            ProcessCommand::new("/usr/bin/python3")
                .arg("-c")
                .arg(script),
            vec![],
            Vec::<(String, String)>::new(),
        )
        .unwrap_err();
        drop(guard);
        assert_eq!(error.transport_kind(), "host_timeout");
        assert!(
            custody.receipts()[0].effect_incapable(),
            "{:?}",
            custody.receipts()
        );
        assert!(
            String::from_utf8_lossy(&error.diagnostics().stdout.bytes).contains("owned PTY ready")
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
thread_local! {
    // Scope-local injection at the OS identity observation boundary, never an env switch.
    static IDENTITY_FAULT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(all(test, target_os = "linux"))]
mod supervisor_identity_tests {
    use super::*;
    use crate::custody::AttemptActorCustody;
    use crate::process::{CancellationToken, ProcessCommand, ProcessLimits, ProcessRunner};
    use std::time::{Duration, Instant};

    fn unavailable_identity_terminal(cancel: bool, acquisition: bool, pipe_holder: bool) {
        let dir = std::env::temp_dir().join(format!("identity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let release = dir.join("release");
        let ready = dir.join("ready");
        let custody = AttemptActorCustody::new(uuid::Uuid::new_v4());
        let guard = custody.begin("launch");
        let token = CancellationToken::new();
        let cancel_token = token.clone();
        let ready_wait = ready.clone();
        let release_watchdog = release.clone();
        // Fixture releases itself after the observation deadline even on the red
        // implementation. No PID signal or pre-kill substitutes for supervision.
        let control = std::thread::spawn(move || {
            let start = Instant::now();
            while !ready_wait.exists() && start.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(2));
            }
            if cancel {
                cancel_token.cancel();
            }
            while !release_watchdog.exists() && start.elapsed() < Duration::from_secs(4) {
                std::thread::sleep(Duration::from_millis(2));
            }
            std::fs::write(release_watchdog, b"release").unwrap();
        });
        let script = format!(
            "import os,pathlib,sys,time,signal\nsignal.signal(signal.SIGTERM,signal.SIG_IGN)\nif {} and os.fork()!=0: sys.exit(0)\npathlib.Path({:?}).touch()\nwhile not pathlib.Path({:?}).exists(): time.sleep(0.002)\n",
            if pipe_holder { "True" } else { "False" },
            ready.to_str().unwrap(),
            release.to_str().unwrap()
        );
        IDENTITY_FAULT.set(if acquisition { 1 } else { 2 });
        let started = Instant::now();
        let result = ProcessRunner::new(ProcessLimits {
            custody: Some(guard.0.clone()),
            cancellation: Some(token),
            timeout: Duration::from_millis(300),
            kill_after_grace: Duration::from_millis(25),
            ..ProcessLimits::default()
        })
        .run(
            ProcessCommand::new("/usr/bin/python3")
                .arg("-c")
                .arg(script),
            vec![],
            Vec::<(String, String)>::new(),
        );
        IDENTITY_FAULT.set(0);
        let elapsed = started.elapsed();
        let before_release = custody.receipts()[0].clone();
        std::fs::write(&release, b"release").unwrap();
        control.join().unwrap();
        let cleanup_start = Instant::now();
        while !custody.receipts()[0].leader_reaped
            && cleanup_start.elapsed() < Duration::from_secs(2)
        {
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(guard);
        eprintln!(
            "identity acquisition={acquisition} cancel={cancel} pipe_holder={pipe_holder} elapsed={elapsed:?}; at_return={before_release:?}; result={result:?}; after_release={:?}",
            custody.receipts()
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "supervisor waited for fixture watchdog"
        );
        let error = result.unwrap_err();
        if !pipe_holder {
            assert_eq!(
                error.transport_kind(),
                if cancel {
                    "host_cancelled"
                } else {
                    "host_timeout"
                }
            );
        }
        assert!(before_release.uncertain);
        assert!(
            !before_release.force_killed,
            "refused kill must not be described as an executed force kill"
        );
        assert!(!error.diagnostics().process_was_force_killed);
        assert!(!before_release.effect_incapable());
        assert!(
            !before_release.leader_reaped,
            "owned cleanup must still retain the live child"
        );
        assert!(
            error
                .diagnostics()
                .description
                .as_deref()
                .unwrap_or("")
                .contains("cleanup_pending")
        );
        assert!(
            custody.receipts()[0].leader_reaped,
            "retained owner must reap after natural release"
        );
        assert!(
            !custody.receipts()[0].effect_incapable(),
            "uncertainty is sticky after deferred cleanup"
        );
    }
    #[test]
    fn supervisor_acquisition_unavailable_timeout() {
        unavailable_identity_terminal(false, true, false);
    }
    #[test]
    fn supervisor_acquisition_unavailable_cancel() {
        unavailable_identity_terminal(true, true, false);
    }
    #[test]
    fn supervisor_revalidation_unavailable_timeout() {
        unavailable_identity_terminal(false, false, false);
    }
    #[test]
    fn supervisor_revalidation_unavailable_cancel() {
        unavailable_identity_terminal(true, false, false);
    }
    #[test]
    fn supervisor_identity_unavailable_exited_leader_pipe_holder() {
        unavailable_identity_terminal(false, true, true);
    }
}

// Native diagnostic fixtures, not a relaxation of check_signal_group or a
// provider-tree completion contract. Each child here executes no descendants.
#[cfg(all(test, target_os = "macos"))]
mod native_group_diagnostic_tests {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            // Exact retained direct child only; never a discovered PID sweep.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn group_signal(pid: u32, signal: i32) -> (i32, Option<i32>) {
        let rc = unsafe { libc::kill(-(pid as i32), signal) };
        let errno = (rc != 0)
            .then(|| std::io::Error::last_os_error().raw_os_error())
            .flatten();
        (rc, errno)
    }

    #[test]
    fn retained_native_group_zombie_and_live_controls() {
        for exited in [true, false] {
            let mut command = if exited {
                let mut command = Command::new("/bin/sh");
                command.args(["-c", "exit 0"]);
                command
            } else {
                let mut command = Command::new("/bin/sleep");
                command.arg("2");
                command
            };
            let mut child = OwnedChild(command.env_clear().process_group(0).spawn().unwrap());
            let pid = child.0.id();
            let started = Instant::now();
            if exited {
                loop {
                    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                    let rc = unsafe {
                        libc::waitid(
                            libc::P_PID,
                            pid,
                            &mut info,
                            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                        )
                    };
                    assert_eq!(
                        rc,
                        0,
                        "native retained wait: {:?}",
                        std::io::Error::last_os_error()
                    );
                    if unsafe { info.si_pid() } == pid as i32 {
                        break;
                    }
                    assert!(started.elapsed() < Duration::from_secs(2));
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            let state = Command::new("/bin/ps")
                .args([
                    "-p",
                    &pid.to_string(),
                    "-o",
                    "pid,ppid,pgid,state,lstart,comm",
                ])
                .env_clear()
                .output()
                .unwrap();
            let probe = group_signal(pid, 0);
            let kill = group_signal(pid, libc::SIGKILL);
            let status = child.0.wait().unwrap();
            let after = group_signal(pid, 0);
            eprintln!(
                "native-group no-descendant-fixture exited={exited} retained_child={pid} ps={} probe={probe:?} kill={kill:?} collected={status:?} after_collection={after:?}",
                String::from_utf8_lossy(&state.stdout)
            );
            if exited {
                assert!(status.success());
                // EPERM and success are observations, not a product permission
                // exemption. An unexpected errno still remains a fixture failure.
                assert!(kill == (0, None) || kill == (-1, Some(libc::EPERM)));
            } else {
                assert_eq!(kill, (0, None));
                assert_eq!(status.signal(), Some(libc::SIGKILL));
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod receipt_group_observation_tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::time::{Duration, Instant};

    #[test]
    fn zombie_leader_with_live_sibling_is_not_a_terminal_group() {
        let script = "import ctypes,os,sys,threading,time\nthreading.Thread(target=lambda: time.sleep(10)).start()\nctypes.CDLL(None).syscall(int(sys.argv[1]),0)\n";
        let mut child = std::process::Command::new("/usr/bin/python3")
            .args(["-c", script, &libc::SYS_exit.to_string()])
            .env_clear()
            .process_group(0)
            .spawn()
            .unwrap();
        let started = Instant::now();
        while proc_stat(child.id()).unwrap().0 != 'Z' && started.elapsed() < Duration::from_secs(2)
        {
            std::thread::yield_now();
        }
        let leader = proc_stat(child.id());
        let before = group_dead(child.id());
        // Retain the leader across the observer and actual signal; no namespace
        // init/reaper observation is substituted for this owner's direct wait.
        let signal = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let status = child.wait().unwrap();
        eprintln!(
            "zombie leader/live sibling: leader={leader:?} terminal_before={before:?} signal={signal} waited={status:?}"
        );
        assert_eq!(leader.unwrap().0, 'Z');
        assert!(!before.unwrap(), "live sibling hidden by zombie leader");
        assert_eq!(signal, 0);
    }

    struct RetainedGroup(std::process::Child);
    impl Drop for RetainedGroup {
        fn drop(&mut self) {
            // This exact unreaped direct leader pins the only signalled group.
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            self.0.wait().unwrap();
        }
    }

    fn named_group() -> RetainedGroup {
        use std::io::BufRead;
        let mut child = std::process::Command::new("/usr/bin/python3")
            .args(["-c", "import ctypes,time\nctypes.CDLL(None).prctl(15,b'opaque\\xff)comm',0,0,0)\nprint('ready',flush=True)\ntime.sleep(10)\n"])
            .env_clear().process_group(0)
            .stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "ready\n");
        RetainedGroup(child)
    }

    #[test]
    fn opaque_comm_does_not_obstruct_owned_or_unrelated_group_observation() {
        let owned = named_group();
        let unrelated = named_group();
        assert!(std::fs::read_to_string(format!("/proc/{}/stat", unrelated.0.id())).is_err());
        assert!(kernel_group_member(owned.0.id(), owned.0.id()).unwrap());
        assert!(!kernel_group_member(unrelated.0.id(), owned.0.id()).unwrap());
        assert!(
            !group_dead(owned.0.id()).unwrap(),
            "owned live task cannot discharge"
        );
        let started = Instant::now();
        while !crate::process::settle_receipt_inspection_group(&owned.0).unwrap() {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "observation failed to progress"
            );
            std::thread::yield_now();
        }
        assert!(
            !group_dead(unrelated.0.id()).unwrap(),
            "unrelated group was signalled"
        );
        eprintln!(
            "opaque comm: owned live=false terminal; owned settled with unrelated opaque-comm task still live; both original waits retained"
        );
    }

    struct StatMask(std::ffi::CString);
    impl StatMask {
        fn new(source: &std::path::Path, target: String) -> Self {
            use std::os::unix::ffi::OsStrExt;
            let source = std::ffi::CString::new(source.as_os_str().as_bytes()).unwrap();
            let target = std::ffi::CString::new(target).unwrap();
            assert_eq!(
                unsafe {
                    libc::mount(
                        source.as_ptr(),
                        target.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND,
                        std::ptr::null(),
                    )
                },
                0,
                "private stat mask: {}",
                std::io::Error::last_os_error()
            );
            Self(target)
        }
    }
    impl Drop for StatMask {
        fn drop(&mut self) {
            assert_eq!(unsafe { libc::umount(self.0.as_ptr()) }, 0);
        }
    }

    #[test]
    #[ignore = "requires private user/mount/PID namespace and removed DAC override capabilities"]
    fn private_denied_stat_excludes_unrelated_but_retains_owned_uncertainty() {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::env::var("HOME").unwrap(), "/mnt/private/home");
        let root = std::env::temp_dir().join(format!("receipt-denial-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let denied = root.join("denied");
        std::fs::write(&denied, b"unreadable observation, not a custody fact").unwrap();
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
        let owned = named_group();
        let unrelated = named_group();
        let unrelated_path = format!("/proc/{}/stat", unrelated.0.id());
        let unrelated_mask = StatMask::new(&denied, unrelated_path.clone());
        assert_eq!(
            std::fs::read(&unrelated_path).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(process_group_tasks_dead(unrelated.0.id(), owned.0.id()).unwrap());
        let task_path = format!("/proc/{0}/task/{0}/stat", owned.0.id());
        let owned_mask = StatMask::new(&denied, task_path.clone());
        assert_eq!(
            std::fs::read(&task_path).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            process_group_tasks_dead(owned.0.id(), owned.0.id())
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        // Real kernel membership is still owned. Denial is not replaced with
        // absence, even after an actual kill while the direct child is pinned.
        assert!(crate::process::settle_receipt_inspection_group(&owned.0).is_err());
        eprintln!(
            "real denied proc observation: unrelated excluded by getpgid; owned denial refuses settlement after actual signal"
        );
        drop(owned_mask);
        let started = Instant::now();
        while !crate::process::settle_receipt_inspection_group(&owned.0).unwrap() {
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::yield_now();
        }
        assert!(!group_dead(unrelated.0.id()).unwrap());
        assert_eq!(
            std::fs::read(&unrelated_path).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        eprintln!(
            "owned cleanup progressed while unrelated stat still actually PermissionDenied and unrelated task still live"
        );
        drop(unrelated_mask);
        drop(owned);
        drop(unrelated);
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hidden_proc_tasks_cannot_be_certified_by_omission() {
        for option in ["rw", "rw,hidepid=0", "rw,hidepid=1", "rw,hidepid=noaccess"] {
            assert!(proc_mount_exposes_task_ids(
                format!("1 0 0:1 / /proc rw - proc proc {option}").as_bytes(),
                b"1"
            ));
        }
        for option in [
            "rw,hidepid=2",
            "rw,hidepid=invisible",
            "rw,hidepid=4",
            "rw,hidepid=ptraceable",
            "rw,hidepid=unknown",
        ] {
            assert!(!proc_mount_exposes_task_ids(
                format!("1 0 0:1 / /proc rw - proc proc {option}").as_bytes(),
                b"1"
            ));
        }
        assert!(!proc_mount_exposes_task_ids(b"", b"1"));
        assert!(!proc_mount_exposes_task_ids(
            b"1 0 0:1 / /proc rw - tmpfs tmpfs rw",
            b"1"
        ));
        let stacked =
            b"1 0 0:1 / /proc rw - proc proc rw\n2 1 0:2 / /proc rw - proc proc rw,hidepid=2";
        assert!(proc_mount_exposes_task_ids(stacked, b"1"));
        assert!(!proc_mount_exposes_task_ids(stacked, b"2"));
        assert!(!proc_mount_exposes_task_ids(stacked, b"3"));
        assert!(proc_mount_exposes_task_ids(
            b"7 0 0:7 / /opaque\xff rw - tmpfs tmpfs rw\n1 0 0:1 / /proc rw - proc proc rw",
            b"1"
        ));
    }

    #[test]
    fn terminal_task_parser_rejects_live_and_unknown_observations() {
        let dir = std::env::temp_dir().join(format!("receipt-stat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("stat");
        for (state, expected) in [
            ("R", false),
            ("S", false),
            ("D", false),
            ("Z", true),
            ("X", true),
        ] {
            std::fs::write(
                &path,
                format!("1 (fixture) {state} 0 7 {} 123", ["0"; 16].join(" ")),
            )
            .unwrap();
            assert_eq!(task_group_dead(&path, 7).unwrap(), expected);
            assert!(task_group_dead(&path, 8).unwrap());
        }
        std::fs::write(&path, b"unavailable identity").unwrap();
        assert!(task_group_dead(&path, 7).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(task_group_dead(&path, 7).unwrap());
        std::fs::remove_dir(dir).unwrap();
    }
}
