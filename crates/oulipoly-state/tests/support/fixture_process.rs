//! Reexecute one fixture with its own installation roots, without mutating the
//! parallel harness environment or changing production path selection.

pub fn completed_in_fixture_process() -> bool {
    const CHILD: &str = "OULIPOLY_FIXTURE_PROCESS_TEST";
    let test = std::thread::current().name().unwrap().to_owned();
    if std::env::var(CHILD).as_deref() == Ok(test.as_str()) {
        return false;
    }
    let current = std::env::current_exe().unwrap();
    let dir = tempfile::tempdir_in(current.parent().unwrap()).unwrap();
    let executable = dir.path().join("fixture-tests");
    // A unique location selects only this fixture's adjacent roots. Link the
    // already-built image on the same mount; never write either link. Copying
    // executable bytes in parallel harness threads can leave a write descriptor
    // inherited by another concurrent fork until exec, causing ETXTBSY. A symlink
    // would instead canonicalize back to the shared installation's config.
    std::fs::hard_link(&current, &executable).unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!(
            "data_dir = {:?}\nconfig_home = {:?}\n",
            dir.path().join("data").to_str().unwrap(),
            dir.path().join("config").to_str().unwrap(),
        ),
    )
    .unwrap();
    let output = std::process::Command::new(&executable)
        .args(["--exact", &test, "--nocapture"])
        .env(CHILD, &test)
        .env_remove("OULIPOLY_DATA_DIR")
        .env_remove("OULIPOLY_CONFIG_HOME")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!(
        "fixture-process {test} root={}\n{stdout}",
        dir.path().display()
    );
    eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "fixture child {test}: {}",
        output.status
    );
    // Exact selection must execute the original body, not a zero-test success.
    assert!(
        stdout
            .lines()
            .any(|line| line == format!("test {test} ... ok"))
    );
    true
}
