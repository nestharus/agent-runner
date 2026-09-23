//! Private paired Bash/Runner guardian exercise. This never uses a production
//! database, an installed broker, provider traffic, or host-root sudo.
#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn eventually(mut done: impl FnMut() -> bool, detail: impl Fn() -> String) {
    // The private path hashes and verifies the debug Bash and Runner images
    // several times (each image is over 150 MiB on this target). This bounds
    // test observation only; it imposes no work lifetime or broker cutoff.
    let until = Instant::now() + Duration::from_secs(180);
    while !done() {
        assert!(Instant::now() < until, "{}", detail());
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

struct PrivateDir(Option<tempfile::TempDir>);

impl PrivateDir {
    fn new() -> Self {
        Self(Some(tempfile::tempdir().unwrap()))
    }

    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        if std::thread::panicking() && std::env::var_os("AGE319_KEEP_PRIVATE_FAILURE").is_some() {
            let path = self.path().to_path_buf();
            let _ = self.0.take().unwrap().keep();
            eprintln!("AGE319_PRIVATE_FAILURE_DIR={}", path.display());
        }
    }
}

fn json(path: impl AsRef<Path>) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn one_file(dir: &Path) -> PathBuf {
    let entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    assert_eq!(entries.len(), 1, "expected one file in {}", dir.display());
    entries[0].clone()
}

fn recipient_cli(runner: &Path, data: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(runner)
        .args(args)
        .env_clear()
        .env("HOME", data.parent().unwrap().join("home"))
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_DATA_DIR", data)
        .env(
            "OULIPOLY_CONFIG_HOME",
            data.parent().unwrap().join("config"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "recipient CLI {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn recipient() {
    use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};

    let data = PathBuf::from(std::env::var_os("AGE319_RECIPIENT_DATA").unwrap());
    let runner = PathBuf::from(std::env::var_os("AGE319_RECIPIENT_RUNNER").unwrap());
    let session = std::env::var("AGE319_RECIPIENT_SESSION").unwrap();
    let handle = std::env::var("AGE319_RECIPIENT_HANDLE").unwrap();
    let source_invocation = std::env::var("AGE319_SOURCE_INVOCATION").unwrap();
    let effect = PathBuf::from(std::env::var_os("AGE319_RECIPIENT_EFFECT").unwrap());
    let receipt_path = PathBuf::from(std::env::var_os("AGE319_RECIPIENT_RECEIPT").unwrap());
    let recipient_invocation = uuid::Uuid::new_v4().to_string();
    assert_ne!(recipient_invocation, source_invocation);

    // The recipient is a separate process and a separate State invocation in
    // the same provider session. It receives no source registration authority.
    let state = StateDb::open(&data.join("state.db")).unwrap();
    let row_id = state
        .start_invocation(&InvocationStart {
            invocation_uuid: recipient_invocation.clone(),
            model_name: "age319-private-mailbox-recipient".into(),
            provider_name: "manual-mailbox-recipient".into(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .bind_invocation_provider_session_start(
            oulipoly_state::InvocationMutationAuthority::Standalone,
            row_id,
            &ProviderSessionBinding {
                provider_session_id: session.clone(),
                capture_method: "age319-private-manual-recipient",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
    drop(state);

    let listed = recipient_cli(
        &runner,
        &data,
        &["mailbox", "list", "--session-id", &session, "--json"],
    );
    let rows = listed["rows"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "recipient must receive exactly one pending row"
    );
    let seq = rows[0]["seq"].as_i64().unwrap();
    assert_eq!(rows[0]["handle"], handle);
    assert_eq!(rows[0]["session_id"], session);
    assert_eq!(rows[0]["owner_invocation_uuid"], source_invocation);
    assert!(rows[0]["delivered_at"].is_null());
    let seq_arg = seq.to_string();
    let shown = recipient_cli(
        &runner,
        &data,
        &[
            "mailbox",
            "show",
            "--session-id",
            &session,
            "--seq",
            &seq_arg,
            "--json",
        ],
    );
    assert_eq!(shown["row"], rows[0]);
    let payload_path = Path::new(rows[0]["payload_file_path"].as_str().unwrap());
    let payload_bytes = fs::read(payload_path).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&payload_bytes)),
        rows[0]["payload_sha256"]
    );
    assert_eq!(
        i64::try_from(payload_bytes.len()).unwrap(),
        rows[0]["payload_byte_len"]
    );
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).unwrap();
    assert_eq!(payload["handle"], handle);
    assert_eq!(payload["completion_protocol"], "completion-continuation-v2");
    fs::write(
        &receipt_path,
        serde_json::to_vec(&serde_json::json!({
            "recipient_invocation_uuid": recipient_invocation,
            "source_invocation_uuid": source_invocation,
            "session_id": session,
            "handle": handle,
            "seq": seq,
            "payload_sha256": rows[0]["payload_sha256"],
            "consumed": true
        }))
        .unwrap(),
    )
    .unwrap();

    // A different session cannot ACK this row, even with the exact sequence.
    let foreign = recipient_cli(
        &runner,
        &data,
        &[
            "mailbox",
            "ack",
            "--session-id",
            "age319-foreign-session",
            "--from-seq",
            &seq_arg,
            "--to-seq",
            &seq_arg,
            "--delivered-by",
            &recipient_invocation,
            "--json",
        ],
    );
    assert_eq!(foreign["acknowledged_count"], 0);
    assert_eq!(fs::read_to_string(&effect).unwrap(), "executed\n");
    let before =
        oulipoly_state::mailbox::MailboxDb::open_read_only(&data.join("pid-identity.db")).unwrap();
    assert_eq!(before.list_pending(&session).unwrap().len(), 1);
    assert!(
        before
            .completion_event_listeners(&handle)
            .unwrap()
            .iter()
            .all(|listener| listener.acknowledged_at.is_none())
    );
    assert!(
        before
            .list_pending(&session)
            .unwrap()
            .iter()
            .all(|row| row.delivered_at.is_none())
    );
    drop(before);

    // Discard the CLI reply. Durable readback, not the response, proves ACK.
    let ack_status = Command::new(&runner)
        .args([
            "mailbox",
            "ack",
            "--session-id",
            &session,
            "--from-seq",
            &seq_arg,
            "--to-seq",
            &seq_arg,
            "--delivered-by",
            &recipient_invocation,
            "--json",
        ])
        .env_clear()
        .env("HOME", data.parent().unwrap().join("home"))
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_DATA_DIR", &data)
        .env(
            "OULIPOLY_CONFIG_HOME",
            data.parent().unwrap().join("config"),
        )
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(ack_status.success());
    let readback = recipient_cli(
        &runner,
        &data,
        &[
            "mailbox",
            "list",
            "--session-id",
            &session,
            "--all",
            "--json",
        ],
    );
    let delivered = &readback["rows"][0];
    assert_eq!(delivered["seq"], seq);
    assert_eq!(
        delivered["delivered_by_invocation_uuid"],
        recipient_invocation
    );
    assert!(delivered["delivered_at"].as_str().is_some());
    assert!(
        recipient_cli(
            &runner,
            &data,
            &["mailbox", "list", "--session-id", &session, "--json"]
        )["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let replay = recipient_cli(
        &runner,
        &data,
        &[
            "mailbox",
            "ack",
            "--session-id",
            &session,
            "--from-seq",
            &seq_arg,
            "--to-seq",
            &seq_arg,
            "--delivered-by",
            &recipient_invocation,
            "--json",
        ],
    );
    assert_eq!(replay["acknowledged_count"], 0);
}

// This process is launched by Bash's actual accepted work script. It is a
// descendant in the consumed work namespace, but its executable is the test
// binary rather than the helper image sealed at H.
fn paired_owner_v_negative_probe() {
    use oulipoly_kernel_broker::protocol::{self, OwnerWitness, ProcessWitness};

    let data = PathBuf::from(std::env::var_os("AGE319_V_PROBE_DATA").unwrap());
    let bash_state = PathBuf::from(std::env::var_os("AGE319_V_PROBE_BASH_STATE").unwrap());
    let broker_state = PathBuf::from(std::env::var_os("AGE319_V_PROBE_BROKER_STATE").unwrap());
    let broker_socket = PathBuf::from(std::env::var_os("AGE319_V_PROBE_BROKER_SOCKET").unwrap());
    let ran = PathBuf::from(std::env::var_os("AGE319_V_PROBE_EFFECT").unwrap());
    eventually(
        || {
            fs::read_dir(bash_state.join("agent-bash"))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry.file_name().to_string_lossy().starts_with("ab_")
                        && entry.path().join("source-registration-v2.json").exists()
                })
        },
        || "accepted Bash source registration did not appear for V probe".into(),
    );
    let handle = fs::read_dir(bash_state.join("agent-bash"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().starts_with("ab_"))
        .unwrap()
        .path();
    let source = json(handle.join("source-registration-v2.json"));
    let grant = fs::read_dir(broker_state.join("grants"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| json(entry.path()))
        .find(|record| record["version"] == 3 && record["consumed"] == true)
        .expect("consumed H/K grant for the live source");
    assert_eq!(grant["work_id"], source["handle"]);
    let mailbox_path = data.join("pid-identity.db");
    let mailbox = oulipoly_state::mailbox::MailboxDb::open_read_only(&mailbox_path).unwrap();
    let owner = mailbox.completion_continuation_owner().unwrap().unwrap();
    drop(mailbox);
    let stamp = |identity: &oulipoly_state::completion_continuation::SourceProcessIdentity| {
        ProcessWitness {
            host_pid: i32::try_from(identity.pid).unwrap(),
            boot_id: identity.boot_id.clone(),
            starttime_ticks: u64::try_from(identity.starttime_ticks).unwrap(),
        }
    };
    let witness = OwnerWitness {
        root_id: grant["root_id"].as_str().unwrap().into(),
        domain_id: owner.domain_id.clone(),
        supervisor_id: owner.supervisor_authority_id.clone(),
        guardian: stamp(&owner.guardian_identity),
        driver: stamp(&owner.driver_identity),
        owner_generation: Some(owner.owner_generation.clone()),
        work_id: Some(grant["work_id"].as_str().unwrap().into()),
        owner_session_id: Some(source["owner_session_id"].as_str().unwrap().into()),
        owner_invocation_uuid: Some(source["owner_invocation_uuid"].as_str().unwrap().into()),
        registration_authority_sha256: Some(
            grant["sealed_helper"]["registration_authority_sha256"]
                .as_str()
                .unwrap()
                .into(),
        ),
    };
    let session = source["owner_session_id"].as_str().unwrap();
    let source_handle = source["handle"].as_str().unwrap();
    let probe = |label: &str, claim: OwnerWitness, expected: &str| {
        let socket = UnixStream::connect(&owner.endpoint).unwrap();
        let failure = protocol::verify_owner_at(&broker_socket, &claim, socket.as_raw_fd())
            .expect_err(label)
            .to_string();
        assert!(failure.contains(expected), "{label}: {failure}");
        assert!(!ran.exists(), "{label} caused work effect before release");
        let mailbox = oulipoly_state::mailbox::MailboxDb::open_read_only(&mailbox_path).unwrap();
        let listeners = mailbox.completion_event_listeners(source_handle).unwrap();
        assert_eq!(
            listeners.len(),
            1,
            "{label} lost the accepted source listener"
        );
        assert!(
            listeners
                .iter()
                .all(|listener| listener.acknowledged_at.is_none()),
            "{label} produced an ACK"
        );
        let historical =
            oulipoly_state::mailbox::MailboxDb::open_historical(&mailbox_path).unwrap();
        assert!(
            historical
                .list_mailbox(session, true)
                .unwrap()
                .iter()
                .all(|row| row.delivered_at.is_none()),
            "{label} delivered a mailbox row"
        );
    };
    let mut missing = witness.clone();
    missing.owner_session_id = None;
    probe(
        "missing owner session",
        missing,
        "registration witness mismatch",
    );
    let mut missing = witness.clone();
    missing.owner_invocation_uuid = None;
    probe(
        "missing owner invocation",
        missing,
        "registration witness mismatch",
    );
    let mut wrong = witness.clone();
    wrong.owner_session_id = Some(uuid::Uuid::new_v4().to_string());
    probe(
        "wrong owner session",
        wrong,
        "registration witness mismatch",
    );
    let mut wrong = witness.clone();
    wrong.owner_invocation_uuid = Some(uuid::Uuid::new_v4().to_string());
    probe(
        "wrong owner invocation",
        wrong,
        "registration witness mismatch",
    );
    let mut stale = witness.clone();
    stale.owner_generation = Some(uuid::Uuid::new_v4().to_string());
    probe(
        "stale owner generation",
        stale,
        "grant incarnation mismatch",
    );
    let mut sibling = witness.clone();
    sibling.work_id = Some("sibling-work".into());
    probe("sibling work", sibling, "work ID mismatch");
    let mut foreign = witness.clone();
    foreign.root_id = uuid::Uuid::new_v4().to_string();
    probe("cross-root", foreign, "root absent");
    probe("wrong helper image", witness, "pinned image mismatch");
}

#[test]
fn pinned_notify_rejects_missing_and_sibling_work_before_bootstrap() {
    let private = tempfile::tempdir().unwrap();
    for claim in [None, Some("sibling-work")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        command
            .args(["notify", "agent-bash-activate", "--handle", "accepted-work"])
            .env_clear()
            .env("HOME", private.path())
            .env("OULIPOLY_DATA_DIR", private.path().join("data"))
            .env(
                "OULIPOLY_KERNEL_EXPECTED_ROOT_V1",
                "00000000-0000-4000-8000-000000000000",
            );
        if let Some(work) = claim {
            command.env("AGENT_BASH_OWNER_WORK_ID_V1", work);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("pinned owner work ID conflict"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!private.path().join("data").exists());
    }
}

fn inner() {
    // The normal Bash image uses the installed pathname. Give it an alias to
    // this fixture's broker only after unshare has created a distinct mount
    // namespace; nothing is written to the host's /run.
    let outer_mount = std::env::var("AGE319_OUTER_MOUNT_NS").unwrap();
    assert_ne!(
        fs::read_link("/proc/self/ns/mnt")
            .unwrap()
            .to_string_lossy(),
        outer_mount
    );
    let private = unsafe {
        libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        )
    };
    assert_eq!(
        private,
        0,
        "private mount propagation: {}",
        std::io::Error::last_os_error()
    );
    let mounted = unsafe {
        libc::mount(
            c"tmpfs".as_ptr(),
            c"/run".as_ptr(),
            c"tmpfs".as_ptr(),
            0,
            c"mode=0755,size=1m".as_ptr().cast(),
        )
    };
    assert_eq!(
        mounted,
        0,
        "private /run mount: {}",
        std::io::Error::last_os_error()
    );
    let runner = std::env::var("OULIPOLY_AGE319_RUNNER_IMAGE").unwrap();
    let bash = std::env::var("OULIPOLY_AGE319_BASH_IMAGE").unwrap();
    let broker_image = std::env::var("OULIPOLY_AGE319_BROKER_IMAGE").unwrap();
    assert!(Path::new(&runner).is_file());
    assert!(Path::new(&bash).is_file());
    assert!(Path::new(&broker_image).is_file());
    let temp = PrivateDir::new();
    let temp_path = temp.path().to_path_buf();
    let data = temp_path.join("data");
    let bash_state = temp_path.join("bash-state");
    let broker_state = temp_path.join("broker-state");
    let config = temp_path.join("config");
    let home = temp_path.join("home");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&bash_state).unwrap();
    fs::create_dir(&broker_state).unwrap();
    fs::create_dir(&config).unwrap();
    fs::create_dir(&home).unwrap();
    let mailbox = oulipoly_state::mailbox::MailboxDb::open_completion_continuation_domain(
        &data.join("pid-identity.db"),
    )
    .unwrap();
    assert!(mailbox.completion_continuation_domain().unwrap().is_some());
    drop(mailbox);
    drop(oulipoly_state::StateDb::open(&data.join("state.db")).unwrap());
    let socket = temp_path.join("broker.sock");
    fs::create_dir("/run/oulipoly-kernel-broker").unwrap();
    std::os::unix::fs::symlink(&socket, "/run/oulipoly-kernel-broker/control.sock").unwrap();
    let broker_log = temp_path.join("broker.log");
    let mut broker = Command::new(broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&broker_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(
        || socket.exists() || broker.try_wait().unwrap().is_some(),
        || fs::read_to_string(&broker_log).unwrap_or_default(),
    );
    assert!(
        socket.exists(),
        "{}",
        fs::read_to_string(&broker_log).unwrap()
    );
    let ran = temp_path.join("ran");
    // Preaccept gate in the very same private broker/State domain: an
    // unsupported direct notify entry cannot reserve or launch work.
    let preaccept = Command::new(&runner)
        .args([
            "notify",
            "agent-bash-activate",
            "--handle",
            "unaccepted-work",
        ])
        .env_clear()
        .env("HOME", &home)
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .output()
        .unwrap();
    assert!(!preaccept.status.success());
    assert!(
        String::from_utf8_lossy(&preaccept.stderr)
            .contains("OULIPOLY_KERNEL_ENTRY_GAP=unsupported kernel CLI mode")
    );
    assert_eq!(fs::read_dir(broker_state.join("works")).unwrap().count(), 0);
    assert_eq!(
        fs::read_dir(broker_state.join("grants")).unwrap().count(),
        0
    );
    assert!(!ran.exists());
    let preaccept_mailbox =
        oulipoly_state::mailbox::MailboxDb::open_read_only(&data.join("pid-identity.db")).unwrap();
    assert!(
        preaccept_mailbox
            .completion_event_listeners("unaccepted-work")
            .unwrap()
            .is_empty()
    );
    let preaccept_history =
        oulipoly_state::mailbox::MailboxDb::open_historical(&data.join("pid-identity.db")).unwrap();
    assert!(
        preaccept_history
            .list_mailbox("unaccepted-work", true)
            .unwrap()
            .is_empty()
    );
    drop(preaccept_mailbox);

    let test_image = std::env::current_exe().unwrap();
    let script = format!(
        "AGE319_V_PROBE_INNER=1 AGE319_V_PROBE_DATA={} AGE319_V_PROBE_BASH_STATE={} AGE319_V_PROBE_BROKER_STATE={} AGE319_V_PROBE_BROKER_SOCKET={} AGE319_V_PROBE_EFFECT={} {} --exact paired_owner_v_negative_probe_node --ignored --nocapture && printf 'executed\\n' >> {}",
        data.display(),
        bash_state.display(),
        broker_state.display(),
        socket.display(),
        ran.display(),
        test_image.display(),
        ran.display(),
    );
    let out = temp_path.join("entry.out");
    let err = temp_path.join("entry.err");
    let mut entry = Command::new(&runner)
        .arg("__age319-private-bash-work-v1")
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_CONFIG_HOME", &config)
        .env("XDG_CONFIG_HOME", &config)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("AGE319_PRIVATE_BASH_IMAGE", &bash)
        .env("AGE319_PRIVATE_WORK_SCRIPT", script)
        .env("AGENT_BASH_AGENT_RUNNER_BIN", &runner)
        .env("XDG_STATE_HOME", &bash_state)
        .stdin(Stdio::null())
        .stdout(Stdio::from(File::create(&out).unwrap()))
        .stderr(Stdio::from(File::create(&err).unwrap()))
        .spawn()
        .unwrap();
    eventually(
        || ran.exists(),
        || {
            let handles = fs::read_dir(bash_state.join("agent-bash"))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("ab_"))
                .map(|entry| {
                    let path = entry.path();
                    (
                        path.clone(),
                        fs::read_to_string(path.join("meta.json")).ok(),
                    )
                })
                .collect::<Vec<_>>();
            format!(
                "private={} entry={} broker={} handles={handles:?}",
                temp_path.display(),
                fs::read_to_string(&err).unwrap_or_default(),
                fs::read_to_string(&broker_log).unwrap_or_default()
            )
        },
    );
    if !ran.exists() {
        let handle_debug = fs::read_dir(bash_state.join("agent-bash"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| {
                let path = entry.path();
                (
                    path.clone(),
                    fs::read_to_string(path.join("meta.json"))
                        .ok()
                        .and_then(|bytes| serde_json::from_str::<serde_json::Value>(&bytes).ok())
                        .and_then(|meta| meta["error"].as_str().map(str::to_owned)),
                    fs::read_to_string(path.join("root-work-diagnostic-v1.jsonl")).ok(),
                    fs::read_to_string(path.join("log")).ok(),
                )
            })
            .collect::<Vec<_>>();
        panic!(
            "INCOMPLETE_PAIRED_SIGNAL: Bash/Runner work did not execute; entry={} broker={} handle={handle_debug:?}",
            fs::read_to_string(&err).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default(),
        );
    }
    eventually(
        || {
            fs::read_dir(broker_state.join("works"))
                .is_ok_and(|mut entries| entries.next().is_some())
        },
        || fs::read_to_string(&err).unwrap_or_default(),
    );
    let work_record = json(one_file(&broker_state.join("works")));
    // The shared registry may also contain a separate v4 native prepare for
    // this Runner invocation. Only the v3 original-work grant can authorize
    // Bash's consumed K work or its sealed helper at V.
    let grant_records: Vec<_> = fs::read_dir(broker_state.join("grants"))
        .unwrap()
        .map(|entry| json(entry.unwrap().path()))
        .collect();
    let (original_grants, native_grants): (Vec<_>, Vec<_>) = grant_records
        .into_iter()
        .partition(|record| record["version"] == 3);
    assert_eq!(original_grants.len(), 1, "expected one Bash v3 grant");
    assert!(native_grants.iter().all(|record| {
        record["version"] == 4
            && record["kind"] == "native-continuation-v1"
            && record["state"] == "prepared"
            && record.get("sealed_helper").is_none()
    }));
    let grant = &original_grants[0];
    assert_eq!(grant["consumed"], true, "K must consume exactly one grant");
    assert_eq!(work_record["accepted_grant_id"], grant["grant_id"]);
    assert_eq!(work_record["work_id"], grant["work_id"]);
    let handles: Vec<_> = fs::read_dir(bash_state.join("agent-bash"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("ab_"))
        .collect();
    assert_eq!(handles.len(), 1);
    let handle = handles[0].path();
    assert!(handle.join("root-work-accepted-v1.json").exists());
    assert!(handle.join("root-work-broker-grant-v1.json").exists());
    let source = json(handle.join("source-registration-v2.json"));
    let receipt = json(handle.join("registration-receipt-v2.json"));
    let launch = json(handle.join("source-launch-v2.json"));
    assert_eq!(receipt["registration_id"], source["registration_id"]);
    assert_eq!(launch["phase"], "launched");
    assert!(
        launch["workload_identity"]["pid"]
            .as_i64()
            .is_some_and(|pid| pid > 0)
    );
    let session = source["owner_session_id"].as_str().unwrap();
    let invocation = source["owner_invocation_uuid"].as_str().unwrap();
    let state = rusqlite::Connection::open_with_flags(
        data.join("state.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let bound_session: String = state
        .query_row(
            "SELECT provider_session_id FROM invocations WHERE invocation_uuid=?1",
            [invocation],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        bound_session, session,
        "Bash must resolve the real State owner"
    );
    assert_eq!(grant["sealed_helper"]["owner_session_id"], session);
    assert_eq!(grant["sealed_helper"]["owner_invocation_uuid"], invocation);
    assert_eq!(fs::read_to_string(&ran).unwrap(), "executed\n");
    eventually(
        || handle.join("source-retention-release-v1.json").exists(),
        || {
            format!(
                "source acceptance missing: private={} meta={} launch={} broker={}",
                temp_path.display(),
                fs::read_to_string(handle.join("meta.json")).unwrap_or_default(),
                fs::read_to_string(handle.join("source-launch-v2.json")).unwrap_or_default(),
                fs::read_to_string(&broker_log).unwrap_or_default()
            )
        },
    );
    let incarnation = work_record["work_incarnation"].as_str().unwrap();
    let terminal = broker_state
        .join("terminals")
        .join(format!("{incarnation}.json"));
    eventually(
        || terminal.exists(),
        || {
            format!(
                "no broker terminal: entry={} broker={}",
                fs::read_to_string(&err).unwrap_or_default(),
                fs::read_to_string(&broker_log).unwrap_or_default()
            )
        },
    );
    let terminal: serde_json::Value = serde_json::from_slice(&fs::read(terminal).unwrap()).unwrap();
    assert_eq!(terminal["physical_tree_drained"], true);
    assert_eq!(
        fs::read_to_string(&ran).unwrap(),
        "executed\n",
        "work launched more than once"
    );
    eventually(
        || handle.join("root-work-result-v1.json").exists(),
        || {
            format!(
                "no Runner result: entry={} broker={}",
                fs::read_to_string(&err).unwrap_or_default(),
                fs::read_to_string(&broker_log).unwrap_or_default()
            )
        },
    );
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(handle.join("root-work-result-v1.json")).unwrap())
            .unwrap();
    assert!(handle.join("root-work-broker-drain-v1.json").exists());
    assert_eq!(result["physical_tree_drained"], true);
    assert_eq!(result["outcome"], "terminal");
    let release = json(handle.join("source-retention-release-v1.json"));
    assert_eq!(release["release_protocol"], "source-retention-release-v1");
    let mailbox =
        oulipoly_state::mailbox::MailboxDb::open_read_only(&data.join("pid-identity.db")).unwrap();
    let source_handle = source["handle"].as_str().unwrap();
    let listeners_before = mailbox.completion_event_listeners(source_handle).unwrap();
    assert_eq!(listeners_before.len(), 1);
    let seq = listeners_before[0].mailbox_seq.unwrap();
    assert!(listeners_before[0].acknowledged_at.is_none());
    let pending = mailbox.list_pending(session).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].seq, seq);
    assert_eq!(pending[0].handle, source_handle);
    assert!(pending[0].delivered_at.is_none());
    drop(mailbox);

    // Paired negative: the live source handle and exact root exist, but a
    // missing or sibling work claim cannot invoke Runner activation.
    for claim in [None, Some("sibling-work")] {
        let mut probe = Command::new(&runner);
        probe
            .args(["notify", "agent-bash-activate", "--handle", source_handle])
            .env_clear()
            .env("HOME", &home)
            .env("OULIPOLY_DATA_DIR", &data)
            .env("OULIPOLY_CONFIG_HOME", &config)
            .env(
                "OULIPOLY_KERNEL_EXPECTED_ROOT_V1",
                work_record["root_id"].as_str().unwrap(),
            );
        if let Some(claim) = claim {
            probe.env("AGENT_BASH_OWNER_WORK_ID_V1", claim);
        }
        let output = probe.output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("pinned owner work ID conflict"));
        assert_eq!(fs::read_to_string(&ran).unwrap(), "executed\n");
        let unchanged =
            oulipoly_state::mailbox::MailboxDb::open_read_only(&data.join("pid-identity.db"))
                .unwrap();
        let listeners = unchanged.completion_event_listeners(source_handle).unwrap();
        assert_eq!(listeners.len(), 1);
        assert!(
            listeners
                .iter()
                .all(|listener| listener.acknowledged_at.is_none())
        );
        let pending = unchanged.list_pending(session).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, seq);
        assert!(pending[0].delivered_at.is_none());
    }
    assert_eq!(fs::read_to_string(&ran).unwrap(), "executed\n");

    let consumption_receipt = temp_path.join("recipient-consumption.json");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "real_bash_source_reaches_guardian_h_k_q",
            "--ignored",
            "--nocapture",
        ])
        .env_remove("AGE319_PRIVATE_PAIRED_INNER")
        .env("AGE319_PRIVATE_RECIPIENT_INNER", "1")
        .env("AGE319_RECIPIENT_DATA", &data)
        .env("AGE319_RECIPIENT_RUNNER", &runner)
        .env("AGE319_RECIPIENT_SESSION", session)
        .env("AGE319_RECIPIENT_HANDLE", source_handle)
        .env("AGE319_SOURCE_INVOCATION", invocation)
        .env("AGE319_RECIPIENT_EFFECT", &ran)
        .env("AGE319_RECIPIENT_RECEIPT", &consumption_receipt)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "recipient stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let consumed = json(&consumption_receipt);
    assert_eq!(consumed["consumed"], true);
    assert_eq!(consumed["seq"], seq);
    assert_eq!(consumed["session_id"], session);
    assert_eq!(consumed["handle"], source_handle);
    assert_ne!(consumed["recipient_invocation_uuid"], invocation);
    let recipient_bound_session: String = state
        .query_row(
            "SELECT provider_session_id FROM invocations WHERE invocation_uuid=?1",
            [consumed["recipient_invocation_uuid"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recipient_bound_session, session);
    let mailbox =
        oulipoly_state::mailbox::MailboxDb::open_historical(&data.join("pid-identity.db")).unwrap();
    let delivered = mailbox.list_mailbox(session, true).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].seq, seq);
    assert!(delivered[0].delivered_at.is_some());
    assert_eq!(
        delivered[0].delivered_by_invocation_uuid.as_deref(),
        consumed["recipient_invocation_uuid"].as_str()
    );
    let listeners = mailbox.completion_event_listeners(source_handle).unwrap();
    assert!(
        !listeners.is_empty()
            && listeners
                .iter()
                .all(|listener| listener.acknowledged_at.is_some()),
        "recipient ACK missing after consumption: listeners={listeners:?}"
    );
    assert_eq!(fs::read_to_string(&ran).unwrap(), "executed\n");
    stop(&mut entry);
    stop(&mut broker);
}

#[test]
#[ignore = "private Bash/Runner positive with in-work V negative controls; provider turn and host sudo remain open"]
fn real_bash_source_reaches_guardian_h_k_q() {
    if std::env::var_os("AGE319_PRIVATE_RECIPIENT_INNER").is_some() {
        recipient();
        return;
    }
    if std::env::var_os("AGE319_PRIVATE_PAIRED_INNER").is_some() {
        inner();
        return;
    }
    if std::env::var_os("OULIPOLY_AGE319_BASH_IMAGE").is_none()
        || std::env::var_os("OULIPOLY_AGE319_RUNNER_IMAGE").is_none()
        || std::env::var_os("OULIPOLY_AGE319_BROKER_IMAGE").is_none()
    {
        panic!("INCOMPLETE_PAIRED_SIGNAL: set exact built Bash, Runner, and broker image paths");
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "real_bash_source_reaches_guardian_h_k_q",
            "--ignored",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_PAIRED_INNER", "1")
        .env(
            "AGE319_OUTER_MOUNT_NS",
            fs::read_link("/proc/self/ns/mnt")
                .unwrap()
                .to_string_lossy()
                .as_ref(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "launched only inside the accepted Bash work by the paired fixture"]
fn paired_owner_v_negative_probe_node() {
    assert!(std::env::var_os("AGE319_V_PROBE_INNER").is_some());
    paired_owner_v_negative_probe();
}
