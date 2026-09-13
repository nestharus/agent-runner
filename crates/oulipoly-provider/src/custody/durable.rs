//! Durable per-operation producer evidence. One directory per operation, no
//! terminal-only wait collection or process-count-sized in-memory ledger.
use super::{ActorSettlementReceipt, OperationCustody, ProcessIdentity};
use std::{fs::File, io::Write, path::Path, sync::Arc};

pub fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("journal parent absent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())?;
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

/// Initialize the dispatch owner's required operation gates before runtime
/// admission. Each explicit not-admitted receipt is superseded durably before
/// its operation can create an effect-capable process.
pub fn initialize_admissions(root: &Path, attempt_id: uuid::Uuid) -> Result<(), String> {
    // A repeated allocated-call setup must never reset an already spent gate
    // before runtime-generation exclusivity rejects that second call.
    std::fs::create_dir(root.join("admissions")).map_err(|e| e.to_string())?;
    for operation in ["describe", "policy.evaluate", "launch"] {
        write_json(
            &root.join("admissions").join(format!("{operation}.json")),
            &serde_json::json!({"attempt_id":attempt_id,"operation":operation,"admitted":false}),
        )?;
    }
    Ok(())
}

pub fn unadmitted_operations(
    root: &Path,
    attempt_id: uuid::Uuid,
) -> Result<Vec<ActorSettlementReceipt>, String> {
    let mut receipts = Vec::new();
    for operation in ["describe", "policy.evaluate", "launch"] {
        let value: serde_json::Value =
            read_json(&root.join("admissions").join(format!("{operation}.json")))?;
        if value["attempt_id"] != attempt_id.to_string() || value["operation"] != operation {
            return Err("native operation admission identity conflict".into());
        }
        match value["admitted"].as_bool() {
            Some(false) => receipts.push(ActorSettlementReceipt {
                attempt_id,
                operation: super::ProviderOperation::from_subcommand(operation),
                spawned: false,
                exact_process_identity: None,
                process_status: None,
                process_tree_terminated: false,
                leader_reaped: false,
                force_killed: false,
                host_cancellation_requested: false,
                operation_finished: true,
                uncertain: false,
            }),
            Some(true) => {}
            None => return Err("native operation admission state absent".into()),
        }
    }
    Ok(receipts)
}

#[cfg(target_os = "linux")]
pub(crate) fn prepare(operation: &OperationCustody) -> std::io::Result<Option<Arc<File>>> {
    let Some(path) = &operation.3 else {
        return Ok(None);
    };
    use std::os::unix::fs::OpenOptionsExt;
    let intent = operation
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let name = match intent.operation {
        super::ProviderOperation::Describe => Some("describe"),
        super::ProviderOperation::Policy => Some("policy.evaluate"),
        super::ProviderOperation::Launch => Some("launch"),
        _ => None,
    };
    if let Some(name) = name {
        let root = path
            .parent()
            .ok_or_else(|| std::io::Error::other("actor journal root absent"))?;
        write_json(
            &root.join("admissions").join(format!("{name}.json")),
            &serde_json::json!({"attempt_id":intent.attempt_id,"operation":name,"admitted":true}),
        )
        .map_err(std::io::Error::other)?;
    }
    std::fs::create_dir_all(path)?;
    write_json(&path.join("intent.json"), &intent).map_err(std::io::Error::other)?;
    write_json(
        &path.join("boot.json"),
        &std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim(),
    )
    .map_err(std::io::Error::other)?;
    let file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.join("proxy.stat"))?;
    file.sync_all()?;
    File::open(path)?.sync_all()?;
    Ok(Some(Arc::new(file)))
}

/// Observe a terminal owned child without consuming it. Retention precedes reap,
/// so death in the write/reap window leaves the next original owner a waitable
/// child. This is an actual wait observation, not a PID disappearance inference.
#[cfg(target_os = "linux")]
pub(crate) fn retain_terminal(operation: &OperationCustody, pid: u32) -> std::io::Result<()> {
    let Some(path) = &operation.3 else {
        return Ok(());
    };
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        let rc =
            unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
        if rc == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    let identity = super::super::process_custody::identity(pid)?;
    let status = if info.si_code == libc::CLD_EXITED {
        (unsafe { info.si_status() }) << 8
    } else {
        (unsafe { info.si_status() })
            | if info.si_code == libc::CLD_DUMPED {
                128
            } else {
                0
            }
    };
    write_json(&path.join("terminal.json"), &(identity, status)).map_err(std::io::Error::other)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file()
        || file.metadata().map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024
    {
        return Err("unbounded or non-file custody record".into());
    }
    serde_json::from_reader(file).map_err(|e| e.to_string())
}

pub fn attributed_proxy(path: &Path) -> Result<ProcessIdentity, String> {
    let stat = std::fs::read_to_string(path.join("proxy.stat")).map_err(|e| e.to_string())?;
    let end = stat.rfind(')').ok_or("invalid proxy stat")?;
    let fields: Vec<_> = stat[end + 1..].split_whitespace().collect();
    Ok(ProcessIdentity {
        os_pid: stat
            .split_whitespace()
            .next()
            .ok_or("absent proxy pid")?
            .parse::<i64>()
            .map_err(|e| e.to_string())?,
        os_boot_id: read_json(&path.join("boot.json"))?,
        os_pid_starttime_ticks: fields
            .get(19)
            .ok_or("absent proxy start time")?
            .parse::<i64>()
            .map_err(|e| e.to_string())?,
    })
}

pub fn intent(path: &Path) -> Result<ActorSettlementReceipt, String> {
    read_json(&path.join("intent.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_reinitialization_cannot_erase_an_entered_operation() {
        let root = std::env::temp_dir().join(format!("age360-admission-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let attempt = uuid::Uuid::new_v4();
        initialize_admissions(&root, attempt).unwrap();
        let path = root.join("admissions/describe.json");
        write_json(
            &path,
            &serde_json::json!({"attempt_id":attempt,"operation":"describe","admitted":true}),
        )
        .unwrap();
        assert!(initialize_admissions(&root, attempt).is_err());
        let receipts = unadmitted_operations(&root, attempt).unwrap();
        assert_eq!(receipts.len(), 2);
        assert!(
            !receipts
                .iter()
                .any(|a| a.operation == crate::custody::ProviderOperation::Describe)
        );
        std::fs::remove_file(path).unwrap();
        assert!(unadmitted_operations(&root, attempt).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
