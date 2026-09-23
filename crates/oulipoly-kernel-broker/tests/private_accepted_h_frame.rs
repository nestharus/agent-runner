//! Exercise the serving broker's challenged H socket with a private root and
//! source process. A prepared grant is durable never-forked debt, not a launch.
#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
use oulipoly_kernel_broker::accepted_grant::{
    Acceptance, GrantRegistry, Registration, SourceIdentity,
};
use oulipoly_kernel_broker::entry_registry::{EntryRecord, ProcessStamp};
use oulipoly_kernel_broker::identity::PinnedProcess;
use oulipoly_kernel_broker::protocol::{self, AcceptedWorkSpec};
use oulipoly_kernel_broker::{RootRecord, RootRegistry};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ROOT_PROCESS: &str = r#"
import os, subprocess, sys, time
def observer_pid():
    with open('/proc/self/status') as status:
        return int(next(line for line in status if line.startswith('NSpid:')).split()[1])
path, mode, script = sys.argv[1:]
if mode == 'source':
    open(path + '/source.pid', 'w').write(str(observer_pid()))
    time.sleep(60)
else:
    assert os.getpid() == 1
    open(path + '/root.pid', 'w').write(str(observer_pid()))
    source = subprocess.Popen([sys.executable, '-c', script, path, 'source', script])
    source.wait()
"#;

struct PrivateProcess(Child);

impl Drop for PrivateProcess {
    fn drop(&mut self) {
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(path: &Path) {
    let until = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(
            Instant::now() < until,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn send_h(path: &Path, body: &[u8], descriptors: &[RawFd], valid_nonce: bool) -> String {
    let mut stream = UnixStream::connect(path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut challenge = [0; 16];
    stream.read_exact(&mut challenge).unwrap();
    if !valid_nonce {
        challenge[0] ^= 0xff;
    }
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(b'H');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(body);
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if !descriptors.is_empty() {
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen =
            unsafe { libc::CMSG_SPACE(std::mem::size_of_val(descriptors) as _) } as _;
        let header = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        assert!(!header.is_null());
        unsafe {
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(descriptors) as _) as _;
            std::ptr::copy_nonoverlapping(
                descriptors.as_ptr(),
                libc::CMSG_DATA(header).cast(),
                descriptors.len(),
            );
        }
    }
    assert_eq!(
        unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) },
        request.len() as isize
    );
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn inner() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("broker-state");
    let work_state = temp.path().join("accepted-h");
    for directory in [
        &state,
        &state.join("entries"),
        &state.join("works"),
        &state.join("grants"),
        &work_state,
    ] {
        fs::create_dir(directory).unwrap();
    }
    let root_process = PrivateProcess(
        Command::new("unshare")
            .args(["--pid", "--fork", "python3", "-c", ROOT_PROCESS])
            .arg(temp.path())
            .arg("root")
            .arg(ROOT_PROCESS)
            .process_group(0)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for(&temp.path().join("root.pid"));
    wait_for(&temp.path().join("source.pid"));
    let root_pid: i32 = fs::read_to_string(temp.path().join("root.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let source_pid: i32 = fs::read_to_string(temp.path().join("source.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let root = PinnedProcess::open(root_pid).unwrap();
    let source = PinnedProcess::open(source_pid).unwrap();
    assert!(root.is_namespace_init().unwrap());
    assert!(source.in_namespace(root.namespace()).unwrap());
    let guardian = PinnedProcess::open(std::process::id() as i32).unwrap();
    let root_id = uuid::Uuid::new_v4().to_string();
    let supervisor = uuid::Uuid::new_v4().to_string();
    let generation = uuid::Uuid::new_v4().to_string();
    RootRegistry::open(&state)
        .unwrap()
        .insert(RootRecord {
            version: 1,
            boot_id: root.boot_id.clone(),
            root_id: root_id.clone(),
            owner_uid: unsafe { libc::getuid() },
            init_host_pid: root.host_pid,
            init_starttime_ticks: root.starttime_ticks,
            pidns_dev: root.pidns_dev,
            pidns_ino: root.pidns_ino,
        })
        .unwrap();
    let guardian_stamp = ProcessStamp::from(&guardian);
    fs::write(
        state.join("entries").join(format!("{root_id}.json")),
        serde_json::to_vec(&EntryRecord {
            version: 1,
            root_id: root_id.clone(),
            owner_uid: unsafe { libc::getuid() },
            entry: guardian_stamp.clone(),
            prepared_guardian: Some(guardian_stamp.clone()),
            domain_id: Some(uuid::Uuid::new_v4().to_string()),
            supervisor_authority_id: Some(supervisor.clone()),
            guardian: Some(guardian_stamp),
            join_consumed: true,
            joined_child: Some(ProcessStamp::from(&source)),
        })
        .unwrap(),
    )
    .unwrap();

    let work_id = "accepted-h";
    let cwd = temp.path();
    let intent = serde_json::to_vec(&serde_json::json!({
        "protocol": "original-work-v1", "work_id": work_id, "root_id": root_id,
        "handle": work_id, "state_root": temp.path(), "meta": {"cwd": cwd}
    }))
    .unwrap();
    fs::write(work_state.join("root-work-intent-v1.json"), &intent).unwrap();
    let accepted = serde_json::to_vec(&Acceptance {
        protocol: "original-work-v1".into(),
        work_id: work_id.into(),
        request_sha256: digest(&intent),
        root_id: root_id.clone(),
        supervisor_authority_id: supervisor,
        owner_generation: generation.clone(),
        initiator: SourceIdentity {
            pid: source_pid.into(),
            boot_id: source.boot_id.clone(),
            starttime_ticks: source.starttime_ticks as i64,
        },
        registration: Registration::Root,
        cancel_capability_sha256: digest(b"fixture cancel capability"),
    })
    .unwrap();
    fs::write(work_state.join("root-work-accepted-v1.json"), &accepted).unwrap();
    let executable = File::open(format!("/proc/{source_pid}/exe")).unwrap();
    let intent_fd = File::open(work_state.join("root-work-intent-v1.json")).unwrap();
    let cwd_fd = File::open(cwd).unwrap();
    let state_fd = File::open(&work_state).unwrap();
    let accepted_fd = File::open(work_state.join("root-work-accepted-v1.json")).unwrap();
    let descriptors = [
        executable.as_raw_fd(),
        intent_fd.as_raw_fd(),
        cwd_fd.as_raw_fd(),
        state_fd.as_raw_fd(),
        accepted_fd.as_raw_fd(),
    ];
    let spec = AcceptedWorkSpec {
        root_id,
        work_id: work_id.into(),
        request_sha256: digest(&intent),
        accepted_sha256: digest(&accepted),
        owner_generation: generation,
    };
    let socket = temp.path().join("broker.sock");
    let log = temp.path().join("broker.log");
    let broker = PrivateProcess(
        Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
            .env(
                "OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1",
                std::env::current_exe().unwrap(),
            )
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&log).unwrap()))
            .spawn()
            .unwrap(),
    );
    wait_for(&socket);

    // These all reach the serving broker socket, before any registry mutation.
    let body = serde_json::to_vec(&spec).unwrap();
    let extra_descriptors = [
        descriptors[0],
        descriptors[1],
        descriptors[2],
        descriptors[3],
        descriptors[4],
        descriptors[0],
    ];
    for (label, frame, fds, valid_nonce, reason) in [
        (
            "empty",
            Vec::new(),
            descriptors.as_slice(),
            true,
            "invalid challenged request",
        ),
        ("short", b"{".to_vec(), descriptors.as_slice(), true, "EOF"),
        (
            "oversized",
            vec![b' '; 2049],
            descriptors.as_slice(),
            true,
            "invalid challenged request",
        ),
        (
            "missing fd",
            body.clone(),
            &descriptors[..4],
            true,
            "unsupported request ancillary data",
        ),
        (
            "extra fd",
            body.clone(),
            extra_descriptors.as_slice(),
            true,
            "unsupported request ancillary data",
        ),
        (
            "wrong shape",
            b"{}".to_vec(),
            descriptors.as_slice(),
            true,
            "missing field",
        ),
        (
            "wrong challenge",
            body.clone(),
            descriptors.as_slice(),
            false,
            "invalid challenged request",
        ),
    ] {
        let response = send_h(&socket, &frame, fds, valid_nonce);
        assert!(
            response.starts_with("error ") && response.contains(reason),
            "{label}: {response}"
        );
        assert_eq!(
            fs::read_dir(state.join("grants")).unwrap().count(),
            0,
            "{label}"
        );
    }
    let rejected_binding = AcceptedWorkSpec {
        root_id: spec.root_id.clone(),
        work_id: spec.work_id.clone(),
        request_sha256: spec.request_sha256.clone(),
        accepted_sha256: spec.accepted_sha256.clone(),
        owner_generation: uuid::Uuid::new_v4().to_string(),
    };
    let rejected =
        protocol::prepare_accepted_work_at(&socket, &rejected_binding, descriptors).unwrap();
    assert!(
        rejected.starts_with("error ") && rejected.contains("receipt or intent mismatch"),
        "{rejected}"
    );
    assert_eq!(fs::read_dir(state.join("grants")).unwrap().count(), 0);

    let response = protocol::prepare_accepted_work_at(&socket, &spec, descriptors).unwrap();
    let grant_id = response
        .trim()
        .strip_prefix("prepared-work ")
        .unwrap_or_else(|| {
            panic!(
                "valid H was refused: {response}; broker: {}",
                fs::read_to_string(&log).unwrap()
            )
        });
    uuid::Uuid::parse_str(grant_id).unwrap();
    let grants = GrantRegistry::open(state.join("grants")).unwrap();
    assert_eq!(grants.records().len(), 1);
    assert_eq!(grants.records()[0].grant_id, grant_id);
    assert!(!grants.records()[0].consumed);
    assert_eq!(fs::read_dir(state.join("works")).unwrap().count(), 0);
    assert!(root.verify().is_ok());
    assert!(source.verify().is_ok());
    let replay = protocol::prepare_accepted_work_at(&socket, &spec, descriptors).unwrap();
    assert!(
        replay.starts_with("error ") && replay.contains("accepted grant preparation denied"),
        "{replay}"
    );
    assert_eq!(fs::read_dir(state.join("grants")).unwrap().count(), 1);
    drop(broker);
    drop(root_process);
}

#[test]
fn challenged_h_prepares_only_durable_debt_and_rejects_bad_frames() {
    if std::env::var_os("AGE319_PRIVATE_H_INNER").is_some() {
        inner();
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "challenged_h_prepares_only_durable_debt_and_rejects_bad_frames",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_H_INNER", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}
