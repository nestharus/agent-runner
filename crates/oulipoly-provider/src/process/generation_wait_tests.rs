//! Actual provider caller independent command-tree waits and measured owner topology.
use super::*;
use oulipoly_core::launch_custody::{LaunchCustody, LaunchScope};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::mpsc;

const HELPER_SCRIPT: &str = r#"
            echo $$ > "$ROOT/helper-root"
            /usr/bin/setsid /bin/bash -c '
                exec 3<>"$ROOT/helper-release"
                echo $$ > "$ROOT/helper-leaf"
                read -t 8 -u 3 release || exit 75
                exit 0
            ' </dev/null >/dev/null 2>&1 &
            printf 'helper-output'
            exit 7
        "#;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("generation-waits-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        make_fifo(&root.join("main-release"));
        make_fifo(&root.join("helper-release"));
        Self(root)
    }
    fn release(&self, name: &str) {
        write_release(&self.0.join(name)).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Nonblocking panic cleanup; native read -t bounds a missed release.
        let _ = write_release(&self.0.join("main-release"));
        let _ = write_release(&self.0.join("helper-release"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn make_fifo(path: &Path) {
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
}
fn write_release(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?
        .write_all(b"release\n")
}
fn eventually(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "private generation-wait deadline"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn run(custody: Arc<LaunchCustody>, root: PathBuf, script: &'static str) -> ProcessOutcome {
    let _scope = LaunchScope::enter(Some(custody));
    ProcessRunner::new(ProcessLimits {
        timeout: Duration::from_secs(10),
        ..ProcessLimits::default()
    })
    .run(
        ProcessCommand::new("/bin/bash").arg("-c").arg(script),
        Vec::new(),
        [("ROOT", root)],
    )
    .unwrap()
}
fn process_row(pid: i32) -> (char, i32, i32) {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    (
        fields[0].chars().next().unwrap(),
        fields[1].parse().unwrap(),
        fields[2].parse().unwrap(),
    )
}
fn fixture_pid(root: &Path, name: &str) -> i32 {
    std::fs::read_to_string(root.join(name))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn direct_children() -> std::collections::BTreeSet<i32> {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|children| {
            children
                .split_whitespace()
                .map(|pid| pid.parse().unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn descendant_set(mut pending: Vec<i32>) -> std::collections::BTreeSet<i32> {
    let mut observed = std::collections::BTreeSet::new();
    while let Some(pid) = pending.pop() {
        if !observed.insert(pid) {
            continue;
        }
        let children = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap();
        pending.extend(
            children
                .split_whitespace()
                .map(|p| p.parse::<i32>().unwrap()),
        );
    }
    observed
}

#[test]
fn helper_tree_wait_completes_independently_of_live_command_in_same_generation() {
    // Count only this fixture's children, even when the outer library suite
    // runs unrelated process tests in parallel.
    const ISOLATED: &str = "OULIPOLY_TEST_GENERATION_WAIT_ISOLATED";
    if std::env::var_os(ISOLATED).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process::generation_wait_tests::helper_tree_wait_completes_independently_of_live_command_in_same_generation", "--nocapture"])
            .env(ISOLATED, "1")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        return;
    }
    let fixture = Fixture::new();
    let custody = Arc::new(LaunchCustody::start(fixture.0.join("proof")).unwrap());
    let main_custody = Arc::clone(&custody);
    let main_root = fixture.0.clone();
    let main = thread::spawn(move || {
        run(
            main_custody,
            main_root,
            r#"
        exec 3<>"$ROOT/main-release"
        echo $$ > "$ROOT/main-ready"
        read -t 8 -u 3 release || exit 75
        exit 8
    "#,
        )
    });
    eventually(|| fixture.0.join("main-ready").exists());
    let main_pid = fixture_pid(&fixture.0, "main-ready");
    let helper_custody = Arc::clone(&custody);
    let helper_root = fixture.0.clone();
    let (sender, receiver) = mpsc::channel();
    let helper = thread::spawn(move || {
        let outcome = run(helper_custody, helper_root, HELPER_SCRIPT);
        sender.send(outcome).unwrap();
    });
    eventually(|| fixture.0.join("helper-leaf").exists());
    let leaf_pid = fixture_pid(&fixture.0, "helper-leaf");
    // Root output descriptors are closed, leaf has escaped, main remains held.
    // Wait for adoption by the dedicated C: a native fork label is not an oracle
    // for attributing arbitrary children in a proposed shared owner.
    let helper_root_pid = fixture_pid(&fixture.0, "helper-root");
    let (_, main_owner, _) = process_row(main_pid);
    eventually(|| {
        let (_, parent, group) = process_row(leaf_pid);
        parent != helper_root_pid && parent != main_owner && group == leaf_pid
    });
    let (_, helper_owner, _) = process_row(leaf_pid);
    assert_eq!(process_row(main_owner).1, std::process::id() as i32);
    assert_eq!(process_row(helper_owner).1, std::process::id() as i32);
    assert_eq!(
        process_row(helper_root_pid).0,
        'Z',
        "remote owner pins workload group until completion"
    );
    let children = direct_children();
    assert_eq!(children.len(), 3, "M plus two direct C children");
    assert!(children.contains(&main_owner));
    assert!(children.contains(&helper_owner));
    let monitor = *children
        .iter()
        .find(|&&pid| pid != main_owner && pid != helper_owner)
        .unwrap();
    println!(
        "held same-generation topology (6 processes, 5 non-zombies): M={monitor}; main C={main_owner} W={main_pid}; helper C={helper_owner} adopted-leaf={leaf_pid}; helper root={helper_root_pid} retained zombie; no P proxies"
    );
    let observed = descendant_set(children.iter().copied().collect());
    assert_eq!(
        observed,
        [
            monitor,
            main_owner,
            main_pid,
            helper_owner,
            helper_root_pid,
            leaf_pid
        ]
        .into_iter()
        .collect()
    );
    println!(
        "measured total={} non_zombies={}",
        observed.len(),
        observed
            .iter()
            .filter(|&&pid| process_row(pid).0 != 'Z')
            .count()
    );
    // A second arbitrary escaped helper overlaps the first, not merely a native
    // sibling. Releasing one helper must leave both other operations pending.
    let second_fixture = Fixture::new();
    let second_root = second_fixture.0.clone();
    let second_custody = Arc::clone(&custody);
    let second = thread::spawn(move || run(second_custody, second_root, HELPER_SCRIPT));
    eventually(|| second_fixture.0.join("helper-leaf").exists());
    let second_leaf = fixture_pid(&second_fixture.0, "helper-leaf");
    let second_root_pid = fixture_pid(&second_fixture.0, "helper-root");
    eventually(|| process_row(second_leaf).1 != second_root_pid);
    let second_owner = process_row(second_leaf).1;
    let overlapping = descendant_set(direct_children().into_iter().collect());
    let mut expected = observed.clone();
    expected.extend([second_leaf, second_root_pid, second_owner]);
    assert_eq!(overlapping, expected);
    println!(
        "two overlapping escaped helpers: measured total={} non_zombies={}",
        overlapping.len(),
        overlapping
            .iter()
            .filter(|&&pid| process_row(pid).0 != 'Z')
            .count()
    );
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(!custody.quiescent());
    fixture.release("helper-release");
    let outcome = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(outcome.status, ProcessStatus::Exited { code: 7 });
    assert_eq!(outcome.stdout.bytes, b"helper-output");
    helper.join().unwrap();
    assert!(
        !main.is_finished(),
        "helper wait must not require main completion"
    );
    assert!(!custody.quiescent());
    println!("helper completed while main pid={main_pid} remained live");
    assert!(
        !second.is_finished(),
        "first helper must not settle second helper"
    );
    second_fixture.release("helper-release");
    let second_outcome = second.join().unwrap();
    assert_eq!(second_outcome.status, ProcessStatus::Exited { code: 7 });
    assert_eq!(second_outcome.stdout.bytes, b"helper-output");
    assert!(!main.is_finished());
    fixture.release("main-release");
    assert_eq!(
        main.join().unwrap().status,
        ProcessStatus::Exited { code: 8 }
    );
    custody.seal();
    eventually(|| custody.quiescent());
}

#[test]
fn remote_provider_wait_signal_timeout_cancel_and_spawn_failure() {
    use crate::custody::AttemptActorCustody;
    for mode in [
        "success",
        "signal",
        "timeout",
        "cancel",
        "spawn-failure",
        "exited-root",
    ] {
        let fixture = Fixture::new();
        let generation = Arc::new(LaunchCustody::start(fixture.0.join("proof")).unwrap());
        let scope = LaunchScope::enter(Some(Arc::clone(&generation)));
        let actor = AttemptActorCustody::new(uuid::Uuid::new_v4());
        let guard = actor.begin("session.capture");
        let token = CancellationToken::new();
        if mode == "cancel" {
            token.cancel_after(Duration::from_millis(150));
        }
        let script = match mode {
            "success" => "printf 'exact-output'; exit 7",
            "signal" => "kill -TERM $$",
            "exited-root" => "(trap '' TERM; /bin/sleep 4) & exit 9",
            _ => "trap '' TERM; /bin/sleep 4",
        };
        let result = ProcessRunner::new(ProcessLimits {
            custody: Some(guard.0.clone()),
            cancellation: Some(token),
            timeout: Duration::from_millis(300),
            kill_after_grace: Duration::from_millis(25),
            ..ProcessLimits::default()
        })
        .run(
            if mode == "spawn-failure" {
                ProcessCommand::new("/nonexistent/age354-command")
            } else {
                ProcessCommand::new("/bin/bash").arg("-c").arg(script)
            },
            vec![],
            Vec::<(String, String)>::new(),
        );
        drop(guard);
        let receipt = actor.receipts().remove(0);
        println!("remote mode={mode}: {result:?}; {receipt:?}");
        assert!(receipt.effect_incapable(), "{receipt:?}");
        match mode {
            "success" => {
                let output = result.unwrap();
                assert_eq!(output.stdout.bytes, b"exact-output");
                assert_eq!(output.status, ProcessStatus::Exited { code: 7 });
            }
            "signal" => assert_eq!(
                result.unwrap().status,
                ProcessStatus::SignalTerminated {
                    signal: libc::SIGTERM
                }
            ),
            "spawn-failure" => {
                assert!(result.is_err());
                assert!(!receipt.spawned);
            }
            _ => {
                assert_eq!(
                    result.unwrap_err().transport_kind(),
                    if mode == "cancel" {
                        "host_cancelled"
                    } else {
                        "host_timeout"
                    }
                );
                assert!(receipt.force_killed);
                if mode == "exited-root" {
                    assert_eq!(
                        receipt.process_status,
                        Some(ProcessStatus::Exited { code: 9 })
                    );
                }
            }
        }
        drop(scope);
        generation.seal();
        eventually(|| generation.quiescent());
    }
}

#[test]
fn published_spawn_observer_retains_legacy_kill_group_proxy() {
    let fixture = Fixture::new();
    let generation = Arc::new(LaunchCustody::start(fixture.0.join("proof")).unwrap());
    let scope = LaunchScope::enter(Some(Arc::clone(&generation)));
    let result = ProcessRunner::new(ProcessLimits {
        spawn_observer: Some(ProcessSpawnObserver::new(|pid| {
            assert_eq!(process_row(pid as i32).2, pid as i32);
            assert_eq!(unsafe { libc::killpg(pid as i32, libc::SIGKILL) }, 0);
            Err("private external cancellation".into())
        })),
        ..ProcessLimits::default()
    })
    .run(
        ProcessCommand::new("/bin/sleep").arg("0.5"),
        vec![],
        Vec::<(String, String)>::new(),
    );
    assert_eq!(
        result.unwrap_err().transport_kind(),
        "spawn_observer_failed"
    );
    drop(scope);
    generation.seal();
    eventually(|| generation.quiescent());
}

#[test]
fn remote_timeout_retains_escaped_tree_and_consuming_cleanup_owner() {
    let fixture = Fixture::new();
    let generation = Arc::new(LaunchCustody::start(fixture.0.join("proof")).unwrap());
    let scope = LaunchScope::enter(Some(Arc::clone(&generation)));
    let started = Instant::now();
    let error = ProcessRunner::new(ProcessLimits {
        timeout: Duration::from_millis(150),
        kill_after_grace: Duration::from_millis(25),
        ..ProcessLimits::default()
    })
    .run(
        ProcessCommand::new("/bin/bash")
            .arg("-c")
            .arg(HELPER_SCRIPT),
        vec![],
        [("ROOT", &fixture.0)],
    )
    .unwrap_err();
    let elapsed = started.elapsed();
    let leaf = fixture_pid(&fixture.0, "helper-leaf");
    let owner = process_row(leaf).1;
    println!("escaped timeout elapsed={elapsed:?}; {error:?}; retained C={owner}, leaf={leaf}");
    assert_eq!(error.transport_kind(), "host_timeout");
    assert!(
        elapsed < Duration::from_secs(3),
        "must not wait for leaf watchdog"
    );
    assert!(
        error
            .diagnostics()
            .description
            .as_deref()
            .unwrap_or("")
            .contains("cleanup_pending")
    );
    assert!(!error.diagnostics().process_was_reaped);
    assert!(
        !error.diagnostics().process_was_force_killed,
        "empty group acknowledgement is not an executed kill"
    );
    assert_eq!(process_row(owner).1, std::process::id() as i32);
    drop(scope);
    generation.seal();
    assert!(!generation.quiescent());
    fixture.release("helper-release");
    eventually(|| generation.quiescent());
    eventually(|| !Path::new(&format!("/proc/{owner}")).exists());
}
