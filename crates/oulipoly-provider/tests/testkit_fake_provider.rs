pub mod support {
    pub mod provider_client;
}

use support::provider_client::{
    fake_provider_source, temp_fixture_dir,
    testkit::{FakeProvider, FakeProviderMode, LeakProbe},
};

#[test]
fn fake_provider_fixture_compiles_to_executable_and_cleans_up() {
    let fake = FakeProvider::compile(fake_provider_source());
    assert!(fake.path().is_file());
    assert!(fake.is_executable());

    let output = fake.run(FakeProviderMode::Success, "describe", "{}");

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"provider_id\":\"fake-provider\""));
    let path = fake.path();
    fake.cleanup();
    assert!(
        !path.exists(),
        "fake-provider cleanup should remove executable"
    );
}

#[test]
fn fake_provider_modes_are_deterministic() {
    let fake = FakeProvider::compile(fake_provider_source());
    let first = fake.run(FakeProviderMode::ProviderTimeoutError, "describe", "{}");
    let second = fake.run(FakeProviderMode::ProviderTimeoutError, "describe", "{}");

    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stderr, second.stderr);
    assert_eq!(first.status.code(), second.status.code());
}

#[test]
fn fake_provider_records_argv_and_stdin_for_assertions() {
    let fake = FakeProvider::compile(fake_provider_source());
    let record = temp_fixture_dir("testkit-record").join("record.txt");

    let output = fake.run_with_env(
        FakeProviderMode::RecordArgvStdin,
        "describe",
        "{\"example\":true}",
        [("FAKE_PROVIDER_RECORD_PATH", record.as_os_str())],
    );

    assert!(output.status.success());
    let recorded = std::fs::read_to_string(record).expect("record should exist");
    assert!(recorded.contains("describe"));
    assert!(recorded.contains("{\"example\":true}"));
}

#[test]
fn leak_probe_reports_no_remaining_descendants_after_cleanup() {
    let fake = FakeProvider::compile(fake_provider_source());
    let leak_probe = LeakProbe::new();

    let mut child = fake.spawn(FakeProviderMode::ChildGrandchild.env_with_probe(&leak_probe));
    leak_probe.wait_for_descendants();
    leak_probe.terminate_process_tree(&mut child);
    leak_probe.assert_no_descendants();
}
