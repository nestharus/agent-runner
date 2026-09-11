//! Characterizes existing command-tree wait semantics inside a shared generation.
//! This is not a shared-launch-owner implementation or per-generation ECHILD proof.
use super::*;
use oulipoly_core::launch_custody::{LaunchCustody, LaunchScope};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::mpsc;

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

#[test]
fn helper_tree_wait_completes_independently_of_live_command_in_same_generation() {
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
        let outcome = run(
            helper_custody,
            helper_root,
            r#"
            echo $$ > "$ROOT/helper-root"
            /usr/bin/setsid /bin/bash -c '
                exec 3<>"$ROOT/helper-release"
                echo $$ > "$ROOT/helper-leaf"
                read -t 8 -u 3 release || exit 75
                exit 0
            ' </dev/null >/dev/null 2>&1 &
            printf 'helper-output'
            exit 7
        "#,
        );
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
    let (_, main_proxy, _) = process_row(main_owner);
    let (_, helper_proxy, _) = process_row(helper_owner);
    let children = direct_children();
    assert_eq!(children.len(), 3, "M plus two direct P children");
    assert!(children.contains(&main_proxy));
    assert!(children.contains(&helper_proxy));
    let monitor = *children
        .iter()
        .find(|&&pid| pid != main_proxy && pid != helper_proxy)
        .unwrap();
    println!(
        "held same-generation topology (7 processes): M={monitor}; main P={main_proxy} C={main_owner} W={main_pid}; helper P={helper_proxy} C={helper_owner} adopted-leaf={leaf_pid}; helper root={helper_root_pid} exited"
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
    fixture.release("main-release");
    assert_eq!(
        main.join().unwrap().status,
        ProcessStatus::Exited { code: 8 }
    );
    custody.seal();
    eventually(|| custody.quiescent());
}
