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
    self, AcceptedWorkSpec, LaunchAcceptedWorkSpec, OwnerWitness, ProcessWitness, SourceControlUse,
    SourceScope, SourceSocketWitness,
};
use oulipoly_kernel_broker::{RootRecord, RootRegistry};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ROOT_PROCESS: &str = r#"
import array, json, os, socket, subprocess, sys, time
def observer_pid():
    with open('/proc/self/status') as status:
        return int(next(line for line in status if line.startswith('NSpid:')).split()[1])
path, mode, script = sys.argv[1:]
if mode == 'source':
    open(path + '/source.pid', 'w').write(str(observer_pid()))
    for _ in range(600):
        command_path = path + '/source-command.json'
        if os.path.exists(command_path):
            command = json.load(open(command_path))
            control = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            control.connect(command['guardian_socket'])
            def attest(witness):
                broker = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                broker.connect(command['broker_socket'])
                challenge = b''
                while len(challenge) < 16:
                    chunk = broker.recv(16 - len(challenge))
                    if not chunk: raise RuntimeError('broker challenge ended early')
                    challenge += chunk
                body = json.dumps(witness, separators=(',', ':')).encode()
                broker.sendmsg([b's' + challenge + body], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [control.fileno()]))])
                response = b''
                while not response.endswith(b'\n') and len(response) < 256:
                    chunk = broker.recv(256 - len(response))
                    if not chunk: raise RuntimeError('broker response ended early')
                    response += chunk
                broker.close()
                return response
            false_nested = dict(command['witness'])
            false_nested['scope'] = {'kind': 'nested', 'parent_work_id': 'sibling-work'}
            false_root = dict(command['witness'])
            false_root['root_id'] = '00000000-0000-4000-8000-000000000000'
            rejected = [attest(false_nested).decode(), attest(false_root).decode()]
            open(path + '/source-rejections.json', 'w').write(json.dumps(rejected))
            response = attest(command['witness'])
            if response != ('verified-source-v2 ' + command['witness']['root_id'] + '\n').encode():
                open(path + '/source-error', 'w').write(repr(response))
                break
            nulls = [os.open('/dev/null', os.O_RDONLY) for _ in range(4)]
            control.sendmsg([b'work!\n{}\n'], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', nulls))])
            control.shutdown(socket.SHUT_WR)
            open(path + '/source-sent', 'w').write('yes')
            while not os.path.exists(path + '/source-release'):
                time.sleep(0.01)
            time.sleep(60)
            break
        time.sleep(0.1)
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

fn read_cross_namespace_frame(socket: &UnixStream, source_pid: i32) {
    let mut bytes = Vec::new();
    let mut descriptors = Vec::<File>::new();
    while bytes.iter().filter(|byte| **byte == b'\n').count() < 2 {
        let mut byte = [0u8; 1];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let mut control = [0usize; 32];
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = std::mem::size_of_val(&control);
        assert_eq!(
            unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) },
            1
        );
        assert_eq!(message.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC), 0);
        let mut credentials = None;
        let mut header = unsafe { libc::CMSG_FIRSTHDR(&message) };
        while !header.is_null() {
            let item = unsafe { &*header };
            if item.cmsg_type == libc::SCM_CREDENTIALS {
                credentials = Some(unsafe { *libc::CMSG_DATA(header).cast::<libc::ucred>() });
            } else if item.cmsg_type == libc::SCM_RIGHTS {
                let count = (item.cmsg_len as usize - unsafe { libc::CMSG_LEN(0) } as usize)
                    / std::mem::size_of::<RawFd>();
                for index in 0..count {
                    descriptors.push(unsafe {
                        File::from_raw_fd(*libc::CMSG_DATA(header).cast::<RawFd>().add(index))
                    });
                }
            }
            header = unsafe { libc::CMSG_NXTHDR(&message, header) };
        }
        assert_eq!(credentials.unwrap().pid, source_pid);
        bytes.push(byte[0]);
    }
    assert_eq!(bytes, b"work!\n{}\n");
    assert_eq!(descriptors.len(), 4);
    let mut trailing = [0u8; 1];
    assert_eq!(
        unsafe { libc::recv(socket.as_raw_fd(), trailing.as_mut_ptr().cast(), 1, 0) },
        0
    );
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

fn send_source_without_reading_reply(path: &Path, witness: &SourceSocketWitness, connected: RawFd) {
    let mut broker = UnixStream::connect(path).unwrap();
    let mut challenge = [0u8; 16];
    broker.read_exact(&mut challenge).unwrap();
    let body = serde_json::to_vec(witness).unwrap();
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(b's');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 64];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen =
        unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as _) as usize;
        *libc::CMSG_DATA(header).cast::<RawFd>() = connected;
    }
    assert_eq!(
        unsafe { libc::sendmsg(broker.as_raw_fd(), &message, libc::MSG_NOSIGNAL) },
        request.len() as isize
    );
    // Losing this broker response must not make the caller send work/cancel.
    // The fixture deliberately sends no source frame after this point.
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

fn inner(kill_case: bool, lost_reply_case: bool, cancel_case: bool, helper_probe: bool) {
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
    let session = uuid::Uuid::new_v4().to_string();
    let invocation = uuid::Uuid::new_v4().to_string();
    let registration_authority = "11".repeat(32);
    // The host entry may finish its joined child while the guardian, root
    // PID1 and an accepted work remain live for a later completion helper.
    let mut historical_entry =
        helper_probe.then(|| Command::new("sleep").arg("60").spawn().unwrap());
    let helper_driver = helper_probe.then(|| {
        PrivateProcess(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "sealed_helper_from_consumed_work_attests_owner"])
                .env("AGE319_DRIVER_IDLE", temp.path().join("driver-release"))
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
            false,
        )
    });
    let driver = helper_driver
        .as_ref()
        .map(|child| PinnedProcess::open(child.0.id() as i32).unwrap());
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
    let entry_stamp = historical_entry
        .as_ref()
        .map(|child| ProcessStamp::from(&PinnedProcess::open(child.id() as i32).unwrap()))
        .unwrap_or_else(|| guardian_stamp.clone());
    fs::write(
        state.join("entries").join(format!("{root_id}.json")),
        serde_json::to_vec(&EntryRecord {
            version: 1,
            root_id: root_id.clone(),
            owner_uid: unsafe { libc::getuid() },
            entry: entry_stamp,
            prepared_guardian: Some(guardian_stamp.clone()),
            domain_id: Some(domain.clone()),
            supervisor_authority_id: Some(supervisor.clone()),
            guardian: Some(guardian_stamp),
            join_consumed: true,
            joined_child: Some(ProcessStamp::from(&source)),
            prepared_driver: None,
        })
        .unwrap(),
    )
    .unwrap();

    let work_id = "accepted-h";
    let cwd = temp.path();
    let source_image = std::env::current_exe().unwrap();
    let helper = work_state.join("delivery-helper");
    fs::copy(&source_image, &helper).unwrap();
    if helper_probe {
        let fake = work_state.join("fake-helper");
        fs::copy(&source_image, &fake).unwrap();
    }
    let helper_metadata = serde_json::json!({
        "path": helper, "device": fs::metadata(&helper).unwrap().dev(),
        "inode": fs::metadata(&helper).unwrap().ino(),
        "size": fs::metadata(&helper).unwrap().len(),
        "sha256": digest(&fs::read(&helper).unwrap())
    });
    let intent = serde_json::to_vec(&serde_json::json!({
        "protocol": "original-work-v1", "work_id": work_id, "root_id": root_id,
        "handle": work_id, "state_root": temp.path(),
        "registration_authority": helper_probe.then(|| registration_authority.as_bytes().to_vec()),
        "meta": {
            "cwd": cwd, "owner_session_id": helper_probe.then_some(&session),
            "owner_invocation_uuid": helper_probe.then_some(&invocation),
            "delivery_helper": helper_metadata
        }
    }))
    .unwrap();
    fs::write(work_state.join("root-work-intent-v1.json"), &intent).unwrap();
    let accepted = serde_json::to_vec(&Acceptance {
        protocol: "original-work-v1".into(),
        work_id: work_id.into(),
        request_sha256: digest(&intent),
        root_id: root_id.clone(),
        supervisor_authority_id: supervisor.clone(),
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
    if helper_probe {
        fs::write(work_state.join("helper-authority"), &registration_authority).unwrap();
        let witness = OwnerWitness {
            root_id: spec.root_id.clone(),
            domain_id: domain.clone(),
            supervisor_id: supervisor,
            guardian: ProcessWitness {
                host_pid: guardian.host_pid,
                boot_id: guardian.boot_id.clone(),
                starttime_ticks: guardian.starttime_ticks,
            },
            driver: {
                let driver = driver.unwrap();
                ProcessWitness {
                    host_pid: driver.host_pid,
                    boot_id: driver.boot_id,
                    starttime_ticks: driver.starttime_ticks,
                }
            },
            owner_generation: Some(spec.owner_generation.clone()),
            work_id: Some(spec.work_id.clone()),
            owner_session_id: Some(session),
            owner_invocation_uuid: Some(invocation),
            registration_authority_sha256: Some(digest(registration_authority.as_bytes())),
        };
        fs::write(
            work_state.join("helper-witness.json"),
            serde_json::to_vec(&witness).unwrap(),
        )
        .unwrap();
        fs::write(work_state.join("probe-helper"), b"yes").unwrap();
    }
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

    if helper_probe {
        // Rebind each partial intent to a matching positive acceptance. H
        // must refuse the incomplete native owner set itself, before it can
        // persist a v2 grant that K would be able to launch.
        for fields in 2..15_u8 {
            let mut partial: serde_json::Value = serde_json::from_slice(&intent).unwrap();
            let meta = partial["meta"].as_object_mut().unwrap();
            for (bit, key) in [
                (1, "delivery_helper"),
                (2, "owner_session_id"),
                (4, "owner_invocation_uuid"),
            ] {
                if fields & bit == 0 {
                    meta.remove(key);
                }
            }
            if fields & 8 == 0 {
                partial
                    .as_object_mut()
                    .unwrap()
                    .remove("registration_authority");
            }
            let partial_intent = serde_json::to_vec(&partial).unwrap();
            let mut partial_receipt: serde_json::Value = serde_json::from_slice(&accepted).unwrap();
            partial_receipt["request_sha256"] = digest(&partial_intent).into();
            let partial_accepted = serde_json::to_vec(&partial_receipt).unwrap();
            fs::write(work_state.join("root-work-intent-v1.json"), &partial_intent).unwrap();
            fs::write(
                work_state.join("root-work-accepted-v1.json"),
                &partial_accepted,
            )
            .unwrap();
            let partial_spec = AcceptedWorkSpec {
                root_id: spec.root_id.clone(),
                work_id: spec.work_id.clone(),
                request_sha256: digest(&partial_intent),
                accepted_sha256: digest(&partial_accepted),
                owner_generation: spec.owner_generation.clone(),
            };
            let refused =
                protocol::prepare_accepted_work_at(&socket, &partial_spec, descriptors).unwrap();
            assert!(
                refused.contains("incomplete sealed helper owner binding"),
                "fields {fields:04b}: {refused}"
            );
            assert_eq!(fs::read_dir(state.join("grants")).unwrap().count(), 0);
            assert_eq!(fs::read_dir(state.join("works")).unwrap().count(), 0);
        }
        let (_guardian_control, worker_control) = UnixStream::pair().unwrap();
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        let capability = unsafe { File::from_raw_fd(pipe[0]) };
        drop(unsafe { File::from_raw_fd(pipe[1]) });
        let launch = protocol::launch_accepted_work_at(
            &socket,
            &LaunchAcceptedWorkSpec {
                grant_id: uuid::Uuid::new_v4().to_string(),
            },
            [
                descriptors[0],
                descriptors[1],
                descriptors[2],
                descriptors[3],
                descriptors[4],
                worker_control.as_raw_fd(),
                capability.as_raw_fd(),
            ],
        )
        .unwrap();
        assert!(
            launch.contains("unavailable one-use accepted grant"),
            "{launch}"
        );
        assert_eq!(fs::read_dir(state.join("works")).unwrap().count(), 0);
        fs::write(work_state.join("root-work-intent-v1.json"), &intent).unwrap();
        fs::write(work_state.join("root-work-accepted-v1.json"), &accepted).unwrap();
        let helper = work_state.join("delivery-helper");
        let file = File::options()
            .read(true)
            .write(true)
            .open(&helper)
            .unwrap();
        let mut original = [0u8; 1];
        file.read_exact_at(&mut original, 0).unwrap();
        file.write_all_at(&[original[0] ^ 1], 0).unwrap();
        let bad = protocol::prepare_accepted_work_at(&socket, &spec, descriptors).unwrap();
        assert!(bad.contains("sealed helper image differs"), "{bad}");
        assert_eq!(fs::read_dir(state.join("grants")).unwrap().count(), 0);
        file.write_all_at(&original, 0).unwrap();
        file.sync_all().unwrap();
    }

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
    assert_eq!(
        grants.records()[0].version,
        if helper_probe { 3 } else { 2 }
    );
    assert_eq!(fs::read_dir(state.join("works")).unwrap().count(), 0);
    assert!(root.verify().is_ok());
    assert!(source.verify().is_ok());
    let guardian_listener = UnixListener::bind(temp.path().join("guardian.sock")).unwrap();
    let enabled: libc::c_int = 1;
    assert_eq!(
        unsafe {
            libc::setsockopt(
                guardian_listener.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PASSCRED,
                (&enabled as *const libc::c_int).cast(),
                std::mem::size_of_val(&enabled) as _,
            )
        },
        0
    );
    let guardian_socket = UnixStream::connect(temp.path().join("guardian.sock")).unwrap();
    let (mut guardian_server, _) = guardian_listener.accept().unwrap();
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
    protocol::verify_source_socket_v2_at(&socket, &cancel_witness, guardian_socket.as_raw_fd())
        .unwrap();
    let mut marker = [0u8; 17];
    guardian_server.read_exact(&mut marker).unwrap();
    assert_eq!(marker[0], b'@');
    let mut ticket = [0u8; 16];
    ticket.copy_from_slice(&marker[1..]);
    let use_cancel = || SourceControlUse::Cancel {
        root_id: spec.root_id.clone(),
        work_id: spec.work_id.clone(),
    };
    protocol::consume_source_ticket_at(&socket, ticket, use_cancel(), guardian_server.as_raw_fd())
        .unwrap();
    assert!(
        protocol::consume_source_ticket_at(
            &socket,
            ticket,
            use_cancel(),
            guardian_server.as_raw_fd()
        )
        .is_err()
    );
    protocol::verify_source_socket_v2_at(&socket, &cancel_witness, guardian_socket.as_raw_fd())
        .unwrap();
    guardian_server.read_exact(&mut marker).unwrap();
    ticket.copy_from_slice(&marker[1..]);
    assert!(
        protocol::consume_source_ticket_at(
            &socket,
            ticket,
            SourceControlUse::Cancel {
                root_id: spec.root_id.clone(),
                work_id: uuid::Uuid::new_v4().to_string()
            },
            guardian_server.as_raw_fd()
        )
        .is_err()
    );
    // A distinct same-UID process may inherit the already connected source
    // end and legitimately pass S for its own outside cancel scope. T must
    // still refuse because the guardian endpoint's original connector is the
    // parent, not that live S requester.
    let mut ready = [-1; 2];
    let mut release = [-1; 2];
    assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0);
    assert_eq!(unsafe { libc::pipe(release.as_mut_ptr()) }, 0);
    let inherited = unsafe { libc::fork() };
    assert!(inherited >= 0);
    if inherited == 0 {
        unsafe {
            libc::close(ready[0]);
            libc::close(release[1]);
        }
        let child = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let mut claim = cancel_witness.clone();
        claim.source = ProcessWitness {
            host_pid: child.host_pid,
            boot_id: child.boot_id,
            starttime_ticks: child.starttime_ticks,
        };
        let passed =
            protocol::verify_source_socket_v2_at(&socket, &claim, guardian_socket.as_raw_fd())
                .is_ok();
        let flag = [u8::from(passed)];
        unsafe {
            libc::write(ready[1], flag.as_ptr().cast(), 1);
        }
        let mut release_byte = [0u8; 1];
        unsafe {
            libc::read(release[0], release_byte.as_mut_ptr().cast(), 1);
            libc::_exit(0);
        }
    }
    unsafe {
        libc::close(ready[1]);
        libc::close(release[0]);
    }
    let mut passed = [0u8; 1];
    assert_eq!(
        unsafe { libc::read(ready[0], passed.as_mut_ptr().cast(), 1) },
        1
    );
    assert_eq!(passed, [1]);
    guardian_server.read_exact(&mut marker).unwrap();
    assert_eq!(marker[0], b'@');
    ticket.copy_from_slice(&marker[1..]);
    let transferred = protocol::consume_source_ticket_at(
        &socket,
        ticket,
        use_cancel(),
        guardian_server.as_raw_fd(),
    );
    assert!(transferred.is_err());
    unsafe {
        libc::write(release[1], b"x".as_ptr().cast(), 1);
        libc::close(ready[0]);
        libc::close(release[1]);
        libc::waitpid(inherited, std::ptr::null_mut(), 0);
    }
    if !kill_case && !lost_reply_case {
        let root_witness = SourceSocketWitness {
            root_id: spec.root_id.clone(),
            domain_id: domain.clone(),
            supervisor_id: grants.records()[0].supervisor_authority_id.clone(),
            guardian: cancel_witness.guardian.clone(),
            source: ProcessWitness {
                host_pid: source.host_pid,
                boot_id: source.boot_id.clone(),
                starttime_ticks: source.starttime_ticks,
            },
            scope: SourceScope::Root,
        };
        fs::write(
            temp.path().join("source-command.json"),
            serde_json::to_vec(&serde_json::json!({
                "guardian_socket": temp.path().join("guardian.sock"),
                "broker_socket": socket,
                "witness": root_witness,
            }))
            .unwrap(),
        )
        .unwrap();
        let (mut cross_server, _) = guardian_listener.accept().unwrap();
        cross_server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        cross_server.read_exact(&mut marker).unwrap();
        assert_eq!(marker[0], b'@');
        ticket.copy_from_slice(&marker[1..]);
        protocol::consume_source_ticket_at(
            &socket,
            ticket,
            SourceControlUse::WorkRoot {
                root_id: spec.root_id.clone(),
                work_id: "new-work".into(),
            },
            cross_server.as_raw_fd(),
        )
        .unwrap();
        wait_for(&temp.path().join("source-sent"));
        let rejections: Vec<String> =
            serde_json::from_slice(&fs::read(temp.path().join("source-rejections.json")).unwrap())
                .unwrap();
        assert_eq!(rejections.len(), 2);
        assert!(
            rejections
                .iter()
                .all(|response| response.starts_with("error "))
        );
        read_cross_namespace_frame(&cross_server, source_pid);
        fs::write(temp.path().join("source-release"), b"yes").unwrap();
    }
    let mut sibling_cancel = cancel_witness.clone();
    sibling_cancel.scope = SourceScope::CancelOutside {
        work_id: "sibling-work".into(),
    };
    assert!(
        protocol::verify_source_socket_at(&socket, &sibling_cancel, guardian_socket.as_raw_fd())
            .is_err()
    );
    assert!(
        protocol::verify_source_socket_v2_at(&socket, &sibling_cancel, guardian_socket.as_raw_fd())
            .is_err()
    );
    let mut stale_source = cancel_witness.clone();
    stale_source.source.starttime_ticks += 1;
    assert!(
        protocol::verify_source_socket_v2_at(&socket, &stale_source, guardian_socket.as_raw_fd())
            .is_err()
    );
    let mut stale_guardian = cancel_witness.clone();
    stale_guardian.guardian.starttime_ticks += 1;
    assert!(
        protocol::verify_source_socket_v2_at(&socket, &stale_guardian, guardian_socket.as_raw_fd())
            .is_err()
    );
    // A connector can die after a successful S. Its frozen SO_PEERCRED PID
    // must not turn a later T into a live source, including after PID reuse.
    let stale_child = unsafe { libc::fork() };
    assert!(stale_child >= 0);
    if stale_child == 0 {
        let connected = UnixStream::connect(temp.path().join("guardian.sock")).unwrap();
        let child = PinnedProcess::open(unsafe { libc::getpid() }).unwrap();
        let mut claim = cancel_witness.clone();
        claim.source = ProcessWitness {
            host_pid: child.host_pid,
            boot_id: child.boot_id,
            starttime_ticks: child.starttime_ticks,
        };
        let passed =
            protocol::verify_source_socket_v2_at(&socket, &claim, connected.as_raw_fd()).is_ok();
        unsafe {
            libc::_exit(if passed { 0 } else { 1 });
        }
    }
    let (mut stale_server, _) = guardian_listener.accept().unwrap();
    stale_server
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stale_server.read_exact(&mut marker).unwrap();
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(stale_child, &mut status, 0) },
        stale_child
    );
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    ticket.copy_from_slice(&marker[1..]);
    assert!(
        protocol::consume_source_ticket_at(&socket, ticket, use_cancel(), stale_server.as_raw_fd())
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
if os.path.exists(state + '/probe-helper'):
    import subprocess
    while not os.path.exists(state + '/entry-exited'): time.sleep(.02)
    for role, image in [('sealed', state + '/delivery-helper'), ('fake', state + '/fake-helper')]:
        env = dict(os.environ, AGE319_HELPER_PROBE=role, AGE319_HELPER_STATE=state, OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY=open(state + '/helper-authority').read().strip())
        probe = subprocess.run([image, '--exact', 'sealed_helper_from_consumed_work_attests_owner', '--nocapture'], env=env, capture_output=True, text=True)
        open(state + '/' + role + '-probe', 'w').write(str(probe.returncode) + '\n' + probe.stdout + '\n' + probe.stderr)
    open(state + '/first-probe-done', 'w').write('yes')
    while not os.path.exists(state + '/restart-broker'): time.sleep(.02)
    env = dict(os.environ, AGE319_HELPER_PROBE='restart', AGE319_HELPER_STATE=state, OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY=open(state + '/helper-authority').read().strip())
    probe = subprocess.run([state + '/delivery-helper', '--exact', 'sealed_helper_from_consumed_work_attests_owner', '--nocapture'], env=env, capture_output=True, text=True)
    open(state + '/restart-probe', 'w').write(str(probe.returncode) + '\n' + probe.stdout + '\n' + probe.stderr)
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
    if helper_probe {
        let mut entry = historical_entry.take().unwrap();
        entry.kill().unwrap();
        entry.wait().unwrap();
        assert!(!RootRegistry::open(&state).unwrap().has_debt());
        let reopened_entries =
            oulipoly_kernel_broker::entry_registry::EntryRegistry::open(state.join("entries"))
                .unwrap();
        assert!(reopened_entries.has_debt());
        assert!(!reopened_entries.has_uncertain_write());
        fs::write(work_state.join("entry-exited"), b"yes").unwrap();
        wait_for(&work_state.join("first-probe-done"));
        broker.0.kill().unwrap();
        broker.0.wait().unwrap();
        broker = PrivateProcess(
            Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
                .env(
                    "OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1",
                    std::env::current_exe().unwrap(),
                )
                .stdout(Stdio::null())
                .stderr(Stdio::from(
                    File::create(temp.path().join("broker-helper-restart.log")).unwrap(),
                ))
                .spawn()
                .unwrap(),
            false,
        );
        let until = Instant::now() + Duration::from_secs(20);
        while protocol::observe_accepted_work_at(&socket, grant_id).ok()
            != Some(format!("work-live {incarnation}\n"))
        {
            assert!(
                Instant::now() < until,
                "broker did not reattach consumed helper work"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        fs::write(work_state.join("restart-broker"), b"yes").unwrap();
    }
    wait_for(&work_state.join("worker-done"));
    if helper_probe {
        for role in ["sealed", "fake", "restart"] {
            let outcome = fs::read_to_string(work_state.join(format!("{role}-probe"))).unwrap();
            assert!(outcome.starts_with("0\n"), "{role}: {outcome}");
            assert!(outcome.contains("1 passed"), "{role}: {outcome}");
        }
    }
    let live = protocol::observe_accepted_work_at(&socket, grant_id).unwrap();
    assert_eq!(live, format!("work-live {incarnation}\n"));
    if cancel_case {
        let signalled = protocol::cancel_accepted_work_at(&socket, grant_id).unwrap();
        assert_eq!(signalled, format!("cancel-signalled {incarnation}\n"));
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let observed = protocol::observe_accepted_work_at(&socket, grant_id).unwrap();
            if observed.starts_with(&format!("work-drained {incarnation} ")) {
                break;
            }
            assert!(
                Instant::now() < until,
                "cancel did not drain work: {observed}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!work_state.join("grandchild-done").exists());
        root.verify().unwrap();
        source.verify().unwrap();
        let replay = protocol::launch_accepted_work_at(&socket, &launch, launch_fds).unwrap();
        assert!(
            replay.contains("unavailable one-use accepted grant"),
            "{replay}"
        );
        return;
    }
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
    send_source_without_reading_reply(&socket, &cancel_witness, guardian_socket.as_raw_fd());
    guardian_server.read_exact(&mut marker).unwrap();
    ticket.copy_from_slice(&marker[1..]);
    assert_eq!(
        GrantRegistry::open(state.join("grants"))
            .unwrap()
            .records()
            .len(),
        1
    );
    assert_eq!(fs::read_dir(state.join("works")).unwrap().count(), 1);
    assert!(!work_state.join("root-work-cancel-v1.json").exists());
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
    // A challenged S result from the previous broker incarnation is never
    // reusable after restart, even while the source and guardian stay live.
    assert!(
        protocol::consume_source_ticket_at(
            &socket,
            ticket,
            use_cancel(),
            guardian_server.as_raw_fd()
        )
        .is_err()
    );
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
        inner(false, false, false, false);
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
        inner(true, false, false, false);
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
        inner(false, true, false, false);
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

#[test]
fn work_specific_cancel_drains_adopted_setsid_descendant() {
    if std::env::var_os("AGE319_PRIVATE_H_INNER").is_some() {
        inner(false, false, true, false);
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "work_specific_cancel_drains_adopted_setsid_descendant",
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
}

#[test]
fn sealed_helper_from_consumed_work_attests_owner() {
    if let Some(path) = std::env::var_os("AGE319_DRIVER_IDLE") {
        let until = Instant::now() + Duration::from_secs(40);
        while !Path::new(&path).exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        return;
    }
    if let Ok(role) = std::env::var("AGE319_HELPER_PROBE") {
        let state = std::path::PathBuf::from(std::env::var("AGE319_HELPER_STATE").unwrap());
        let base = state.parent().unwrap();
        let socket = UnixStream::connect(base.join("guardian.sock")).unwrap();
        let witness: OwnerWitness =
            serde_json::from_slice(&fs::read(state.join("helper-witness.json")).unwrap()).unwrap();
        let authority = std::env::var("OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY").unwrap();
        assert_eq!(
            witness.registration_authority_sha256.as_deref(),
            Some(digest(authority.as_bytes()).as_str())
        );
        let verify = |claim: &OwnerWitness| {
            protocol::verify_owner_at(&base.join("broker.sock"), claim, socket.as_raw_fd())
        };
        if role == "fake" {
            assert!(
                verify(&witness).is_err(),
                "byte-identical but unpinned executable was admitted"
            );
            return;
        }
        verify(&witness).unwrap();
        let mut missing = witness.clone();
        missing.work_id = None;
        assert!(verify(&missing).is_err(), "missing work ID was admitted");
        let mut malformed = witness.clone();
        malformed.work_id = Some(String::new());
        assert!(verify(&malformed).is_err(), "empty work ID was admitted");
        let mut sibling = witness.clone();
        sibling.work_id = Some("sibling-work".into());
        assert!(verify(&sibling).is_err(), "sibling work ID was admitted");
        let mut missing = witness.clone();
        missing.owner_session_id = None;
        assert!(
            verify(&missing).is_err(),
            "missing pinned session was admitted"
        );
        let mut missing = witness.clone();
        missing.owner_invocation_uuid = None;
        assert!(
            verify(&missing).is_err(),
            "missing pinned invocation was admitted"
        );
        let mut missing = witness.clone();
        missing.owner_session_id = None;
        missing.owner_invocation_uuid = None;
        assert!(
            verify(&missing).is_err(),
            "missing owner identity was admitted"
        );
        let mut wrong = witness.clone();
        wrong.root_id = uuid::Uuid::new_v4().to_string();
        assert!(verify(&wrong).is_err());
        let mut wrong = witness.clone();
        wrong.owner_generation = Some(uuid::Uuid::new_v4().to_string());
        assert!(verify(&wrong).is_err());
        let mut wrong = witness.clone();
        wrong.owner_session_id = Some(uuid::Uuid::new_v4().to_string());
        assert!(verify(&wrong).is_err());
        let mut wrong = witness.clone();
        wrong.owner_invocation_uuid = Some(uuid::Uuid::new_v4().to_string());
        assert!(verify(&wrong).is_err());
        let mut wrong = witness.clone();
        wrong.registration_authority_sha256 = Some(digest(b"wrong native session capability"));
        assert!(verify(&wrong).is_err());
        let mut wrong = witness.clone();
        wrong.registration_authority_sha256 = None;
        assert!(verify(&wrong).is_err());
        let mut wrong = witness.clone();
        wrong.driver.starttime_ticks += 1;
        assert!(verify(&wrong).is_err());
        let mut wrong = witness.clone();
        wrong.guardian.starttime_ticks += 1;
        assert!(verify(&wrong).is_err());
        let (wrong_socket, _other_end) = UnixStream::pair().unwrap();
        assert!(
            protocol::verify_owner_at(
                &base.join("broker.sock"),
                &witness,
                wrong_socket.as_raw_fd()
            )
            .is_err(),
            "a socket without the pinned guardian peer was admitted"
        );
        return;
    }
    if std::env::var_os("AGE319_PRIVATE_H_INNER").is_some() {
        inner(false, false, false, true);
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "sealed_helper_from_consumed_work_attests_owner",
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
