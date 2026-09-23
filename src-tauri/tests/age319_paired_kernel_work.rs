//! Private paired Bash/Runner guardian exercise. This never uses a production
//! database, an installed broker, provider traffic, or host-root sudo.
#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use std::fs::{self, File};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn eventually(mut done: impl FnMut() -> bool, detail: impl Fn() -> String) {
    let until = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < until, "{}", detail());
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn inner() {
    let runner = std::env::var("OULIPOLY_AGE319_RUNNER_IMAGE").unwrap();
    let bash = std::env::var("OULIPOLY_AGE319_BASH_IMAGE").unwrap();
    let broker_image = std::env::var("OULIPOLY_AGE319_BROKER_IMAGE").unwrap();
    assert!(Path::new(&runner).is_file());
    assert!(Path::new(&bash).is_file());
    assert!(Path::new(&broker_image).is_file());
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let bash_state = temp.path().join("bash-state");
    let broker_state = temp.path().join("broker-state");
    let config = temp.path().join("config");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&bash_state).unwrap();
    fs::create_dir(&broker_state).unwrap();
    fs::create_dir(&config).unwrap();
    let mailbox = oulipoly_state::mailbox::MailboxDb::open_completion_continuation_domain(
        &data.join("pid-identity.db"),
    )
    .unwrap();
    assert!(mailbox.completion_continuation_domain().unwrap().is_some());
    drop(mailbox);
    drop(oulipoly_state::StateDb::open(&data.join("state.db")).unwrap());
    let socket = temp.path().join("broker.sock");
    let broker_log = temp.path().join("broker.log");
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
    let ran = temp.path().join("ran");
    let script = format!("printf 'executed\\n' >> {}", ran.display());
    let out = temp.path().join("entry.out");
    let err = temp.path().join("entry.err");
    let mut entry = Command::new(&runner)
        .arg("__age319-private-bash-work-v1")
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_CONFIG_HOME", &config)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("AGE319_PRIVATE_BASH_IMAGE", &bash)
        .env("AGE319_PRIVATE_WORK_SCRIPT", script)
        .env("AGENT_BASH_AGENT_RUNNER_BIN", &runner)
        .env("XDG_STATE_HOME", &bash_state)
        .env_remove("LD_LIBRARY_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::from(File::create(&out).unwrap()))
        .stderr(Stdio::from(File::create(&err).unwrap()))
        .spawn()
        .unwrap();
    eventually(
        || ran.exists() || entry.try_wait().unwrap().is_some(),
        || {
            format!(
                "entry={} broker={}",
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
                )
            })
            .collect::<Vec<_>>();
        if std::env::var_os("AGE319_KEEP_PRIVATE_FAILURE").is_some() {
            let _ = temp.keep();
        }
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
    let work_record: serde_json::Value = serde_json::from_slice(
        &fs::read(
            fs::read_dir(broker_state.join("works"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap(),
    )
    .unwrap();
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
    let handles: Vec<_> = fs::read_dir(bash_state.join("agent-bash"))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(handles.len(), 1);
    let handle = handles[0].path();
    assert!(handle.join("root-work-accepted-v1.json").exists());
    assert!(handle.join("root-work-broker-grant-v1.json").exists());
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
    stop(&mut entry);
    stop(&mut broker);
}

#[test]
#[ignore = "requires a native owner session in the pinned Runner entry; run explicitly to record the paired gap"]
fn real_bash_source_reaches_guardian_h_k_q() {
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
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
