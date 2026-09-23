//! Exercise challenged H/K/Q against a real private root and accepted source
//! image. H alone is debt; K launches one nested PID1/worker and Q observes
//! its receipt and exact drain or uncertainty.
#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
use oulipoly_kernel_broker::accepted_grant::{
    Acceptance, GrantRegistry, Registration, SourceIdentity,
};
use oulipoly_kernel_broker::entry_registry::{EntryRecord, ProcessStamp};
use oulipoly_kernel_broker::identity::PinnedProcess;
use oulipoly_kernel_broker::protocol::{
    self, AcceptedWorkSpec, LaunchAcceptedWorkSpec, ProcessWitness, SourceScope,
    SourceSocketWitness,
};
use oulipoly_kernel_broker::{RootRecord, RootRegistry};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
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

struct PrivateProcess(Child, bool);

impl Drop for PrivateProcess {
    fn drop(&mut self) {
        if self.1 {
            unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
        }
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

fn sibling_launch_probe() {
    let socket = std::env::var("AGE319_K_SIBLING_SOCKET").unwrap();
    let work_state = std::env::var("AGE319_K_SIBLING_STATE").unwrap();
    let source_pid = std::env::var("AGE319_K_SIBLING_SOURCE")
        .unwrap()
        .parse::<i32>()
        .unwrap();
    let grant_id = std::env::var("AGE319_K_SIBLING_GRANT").unwrap();
    let image = File::open(format!("/proc/{source_pid}/exe")).unwrap();
    let intent = File::open(format!("{work_state}/root-work-intent-v1.json")).unwrap();
    let cwd = File::open(std::env::var("AGE319_K_SIBLING_CWD").unwrap()).unwrap();
    let state = File::open(&work_state).unwrap();
    let accepted = File::open(format!("{work_state}/root-work-accepted-v1.json")).unwrap();
    let (_guardian, worker) = UnixStream::pair().unwrap();
    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    let capability = unsafe { File::from_raw_fd(pipe[0]) };
    drop(unsafe { File::from_raw_fd(pipe[1]) });
    let response = protocol::launch_accepted_work_at(
        Path::new(&socket),
        &LaunchAcceptedWorkSpec { grant_id },
        [
            image.as_raw_fd(),
            intent.as_raw_fd(),
            cwd.as_raw_fd(),
            state.as_raw_fd(),
            accepted.as_raw_fd(),
            worker.as_raw_fd(),
            capability.as_raw_fd(),
        ],
    )
    .unwrap();
    assert!(response.starts_with("error "), "{response}");
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

fn send_k_without_reply(path: &Path, spec: &LaunchAcceptedWorkSpec, descriptors: [RawFd; 7]) {
    let mut stream = UnixStream::connect(path).unwrap();
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge).unwrap();
    let mut request = Vec::new();
    request.push(b'K');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&serde_json::to_vec(spec).unwrap());
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 128];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of_val(&descriptors) as _) } as _;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&descriptors) as _) as _;
        std::ptr::copy_nonoverlapping(descriptors.as_ptr(), libc::CMSG_DATA(header).cast(), 7);
    }
    assert_eq!(
        unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) },
        request.len() as isize
    );
    drop(stream);
}

fn inner(kill_case: bool, lost_reply_case: bool) {
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
        true,
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
    let domain = uuid::Uuid::new_v4().to_string();
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
            domain_id: Some(domain.clone()),
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
    let mut broker = PrivateProcess(
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
        false,
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
    let guardian_listener = UnixListener::bind(temp.path().join("guardian.sock")).unwrap();
    let guardian_socket = UnixStream::connect(temp.path().join("guardian.sock")).unwrap();
    let (_guardian_server, _) = guardian_listener.accept().unwrap();
    let stamp = ProcessWitness {
        host_pid: guardian.host_pid,
        boot_id: guardian.boot_id.clone(),
        starttime_ticks: guardian.starttime_ticks,
    };
    let cancel_witness = SourceSocketWitness {
        root_id: spec.root_id.clone(),
        domain_id: domain.clone(),
        supervisor_id: grants.records()[0].supervisor_authority_id.clone(),
        guardian: stamp.clone(),
        source: stamp,
        scope: SourceScope::CancelOutside {
            work_id: spec.work_id.clone(),
        },
    };
    protocol::verify_source_socket_at(&socket, &cancel_witness, guardian_socket.as_raw_fd())
        .unwrap();
    let mut sibling_cancel = cancel_witness.clone();
    sibling_cancel.scope = SourceScope::CancelOutside {
        work_id: "sibling-work".into(),
    };
    assert!(
        protocol::verify_source_socket_at(&socket, &sibling_cancel, guardian_socket.as_raw_fd())
            .is_err()
    );
    let replay = protocol::prepare_accepted_work_at(&socket, &spec, descriptors).unwrap();
    assert!(
        replay.starts_with("error ") && replay.contains("accepted grant preparation denied"),
        "{replay}"
    );
    assert_eq!(fs::read_dir(state.join("grants")).unwrap().count(), 1);
    // The accepted source is a real, live Python image. The broker must run
    // that exact image with its fixed worker argv in a nested work namespace.
    // The script below leaves an adopted grandchild after the worker exits.
    fs::write(
        cwd.join("__root-original-work-v1"),
        r#"import os, sys, time
state = os.readlink('/proc/self/fd/' + sys.argv[3])
assert 'NoNewPrivs:\t0' in open('/proc/self/status').read()
assert os.getpid() == 2
if os.fork() == 0:
    if os.fork() == 0:
        os.environ.clear()
        os.setsid()
        for fd in range(3, 256):
            try: os.close(fd)
            except OSError: pass
        while not os.path.exists(state + '/release-grandchild'): time.sleep(.02)
        open(state + '/grandchild-done', 'w').write('done')
        os._exit(0)
    os._exit(0)
os.waitpid(-1, 0)
open(state + '/worker-done', 'w').write('done')
"#,
    )
    .unwrap();
    let (guardian_control, worker_control) = UnixStream::pair().unwrap();
    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    let capability = unsafe { File::from_raw_fd(pipe[0]) };
    let mut capability_writer = unsafe { File::from_raw_fd(pipe[1]) };
    capability_writer.write_all(b"fixture capability").unwrap();
    drop(capability_writer);
    let launch_fds = [
        descriptors[0],
        descriptors[1],
        descriptors[2],
        descriptors[3],
        descriptors[4],
        worker_control.as_raw_fd(),
        capability.as_raw_fd(),
    ];
    let launch = LaunchAcceptedWorkSpec {
        grant_id: grant_id.to_owned(),
    };
    let bad_fds = [
        state_fd.as_raw_fd(),
        descriptors[1],
        descriptors[2],
        descriptors[3],
        descriptors[4],
        worker_control.as_raw_fd(),
        capability.as_raw_fd(),
    ];
    let bad = protocol::launch_accepted_work_at(&socket, &launch, bad_fds).unwrap();
    assert!(bad.contains("descriptor identity changed"), "{bad}");
    assert!(!GrantRegistry::open(state.join("grants")).unwrap().records()[0].consumed);
    let sibling = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "challenged_h_prepares_and_k_launches_one_nested_worker",
            "--nocapture",
        ])
        .env("AGE319_K_SIBLING_SOCKET", &socket)
        .env("AGE319_K_SIBLING_STATE", &work_state)
        .env("AGE319_K_SIBLING_SOURCE", source_pid.to_string())
        .env("AGE319_K_SIBLING_GRANT", grant_id)
        .env("AGE319_K_SIBLING_CWD", cwd)
        .output()
        .unwrap();
    assert!(
        sibling.status.success(),
        "{}",
        String::from_utf8_lossy(&sibling.stderr)
    );
    assert!(!GrantRegistry::open(state.join("grants")).unwrap().records()[0].consumed);
    let (incarnation, init_pid, worker_pid) = if lost_reply_case {
        send_k_without_reply(&socket, &launch, launch_fds);
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let paths: Vec<_> = fs::read_dir(state.join("works"))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            if paths.len() == 1 {
                let record: serde_json::Value =
                    serde_json::from_slice(&fs::read(paths[0].path()).unwrap()).unwrap();
                break (
                    record["work_incarnation"].as_str().unwrap().to_owned(),
                    record["init_host_pid"].as_i64().unwrap() as i32,
                    None,
                );
            }
            assert!(
                Instant::now() < until,
                "lost K response did not leave a work record: {}",
                fs::read_to_string(&log).unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    } else {
        let launched = protocol::launch_accepted_work_at(&socket, &launch, launch_fds).unwrap();
        let parts: Vec<_> = launched.split_whitespace().collect();
        assert_eq!(
            parts[0],
            "launched-work",
            "{launched}: {}",
            fs::read_to_string(&log).unwrap()
        );
        assert_eq!(parts.len(), 4);
        (
            parts[1].to_owned(),
            parts[2].parse().unwrap(),
            Some(parts[3].parse::<i32>().unwrap()),
        )
    };
    let incarnation = incarnation.as_str();
    uuid::Uuid::parse_str(incarnation).unwrap();
    assert_ne!(init_pid, root_pid);
    if let Some(worker_pid) = worker_pid {
        assert_ne!(worker_pid, source_pid);
    }
    assert!(GrantRegistry::open(state.join("grants")).unwrap().records()[0].consumed);
    wait_for(&work_state.join("worker-done"));
    let live = protocol::observe_accepted_work_at(&socket, grant_id).unwrap();
    assert_eq!(live, format!("work-live {incarnation}\n"));
    if kill_case {
        let init = PinnedProcess::open(init_pid).unwrap();
        assert_eq!(unsafe { libc::kill(init_pid, libc::SIGKILL) }, 0);
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let observed = protocol::observe_accepted_work_at(&socket, grant_id).unwrap();
            if observed == format!("work-uncertain {incarnation} missing-terminal-receipt\n") {
                assert!(init.exited().unwrap());
                break;
            }
            assert!(
                Instant::now() < until,
                "killed PID1 was not uncertain: {observed}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !state
                .join("terminals")
                .join(format!("{incarnation}.json"))
                .exists()
        );
        let replay = protocol::launch_accepted_work_at(&socket, &launch, launch_fds).unwrap();
        assert!(
            replay.contains("unavailable one-use accepted grant"),
            "{replay}"
        );
        return;
    }
    // The worker and adopted grandchild remain owned by PID1 when the broker
    // itself exits. Reattach the exact record and keep replay spent.
    broker.0.kill().unwrap();
    broker.0.wait().unwrap();
    let mut broker = PrivateProcess(
        Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
            .env(
                "OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1",
                std::env::current_exe().unwrap(),
            )
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                File::create(temp.path().join("broker-restart.log")).unwrap(),
            ))
            .spawn()
            .unwrap(),
        false,
    );
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let result = protocol::observe_accepted_work_at(&socket, grant_id);
        if let Ok(observed) = &result {
            assert_eq!(observed.as_str(), format!("work-live {incarnation}\n"));
            break;
        }
        assert!(
            Instant::now() < until,
            "broker did not reattach live work: {result:?}; {}",
            fs::read_to_string(temp.path().join("broker-restart.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !state
            .join("terminals")
            .join(format!("{incarnation}.json"))
            .exists()
    );
    let replay_launch = protocol::launch_accepted_work_at(&socket, &launch, launch_fds).unwrap();
    assert!(
        replay_launch.contains("unavailable one-use accepted grant"),
        "{replay_launch}"
    );
    fs::write(work_state.join("release-grandchild"), b"R").unwrap();
    wait_for(&work_state.join("grandchild-done"));
    let terminal = state.join("terminals").join(format!("{incarnation}.json"));
    wait_for(&terminal);
    let receipt: serde_json::Value = serde_json::from_slice(&fs::read(&terminal).unwrap()).unwrap();
    assert_eq!(receipt["physical_tree_drained"], true);
    assert_eq!(receipt["worker_local_pid"], 2);
    assert_eq!(receipt["init_host_pid"], init_pid);
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let observed = protocol::observe_accepted_work_at(&socket, grant_id).unwrap();
        if observed.starts_with("work-drained ") {
            assert!(observed.contains(incarnation), "{observed}");
            break;
        }
        assert!(Instant::now() < until, "drain was not observed: {observed}");
        std::thread::sleep(Duration::from_millis(20));
    }
    broker.0.kill().unwrap();
    broker.0.wait().unwrap();
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
            .stderr(Stdio::from(
                File::create(temp.path().join("broker-drain-restart.log")).unwrap(),
            ))
            .spawn()
            .unwrap(),
        false,
    );
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        match protocol::observe_accepted_work_at(&socket, grant_id) {
            Ok(observed) => {
                assert!(
                    observed.starts_with(&format!("work-drained {incarnation} ")),
                    "{observed}"
                );
                break;
            }
            Err(error) => {
                assert!(
                    Instant::now() < until,
                    "drain restart failed: {error}; {}",
                    fs::read_to_string(temp.path().join("broker-drain-restart.log"))
                        .unwrap_or_default()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    drop(guardian_control);
    drop(broker);
    drop(root_process);
}

#[test]
fn challenged_h_prepares_and_k_launches_one_nested_worker() {
    if std::env::var_os("AGE319_K_SIBLING_SOCKET").is_some() {
        sibling_launch_probe();
        return;
    }
    if std::env::var_os("AGE319_PRIVATE_H_INNER").is_some() {
        inner(false, false);
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "challenged_h_prepares_and_k_launches_one_nested_worker",
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

#[test]
fn killed_work_pid1_is_uncertain_without_terminal_receipt() {
    if std::env::var_os("AGE319_PRIVATE_H_INNER").is_some() {
        inner(true, false);
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "killed_work_pid1_is_uncertain_without_terminal_receipt",
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

#[test]
fn lost_k_response_cannot_replay_accepted_worker() {
    if std::env::var_os("AGE319_PRIVATE_H_INNER").is_some() {
        inner(false, true);
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "lost_k_response_cannot_replay_accepted_worker",
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
