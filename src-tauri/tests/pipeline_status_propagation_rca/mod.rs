//! RCA Phase 1 reproduction harness for `rca-agent-runner-crashes-2026-05-16`,
//! Finding F1 (pipeline-status propagation). See the scenario file for the full
//! gap analysis and assertion rationale.
//!
//! This module owns the fixture + helper plumbing shared by the scenario
//! tests; the actual `#[test]` lives in
//! `rc1_abnormal_termination_under_tail_pipeline.rs`. The helpers are scoped
//! to this RCA so the test stays self-contained and does not touch any other
//! integration test's fixture surface.
//!
//! Declared roles: accessor, formatter, mapper, parser, predicate, filter,
//! orchestration, validator.

use oulipoly_state::StateDb;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

pub mod rc1_abnormal_termination_under_tail_pipeline;

#[cfg(test)]
pub mod age158_characterization;

#[cfg(test)]
pub mod recognizer_tightening_tests;

#[path = "../age153_support/mod.rs"]
mod age153_support;

/// Tempdir-isolated fixture mirroring the shape used by
/// `pr_a_invocation_integration.rs::Fixture`, with three additions for this
/// RCA: an `XDG_STATE_HOME` redirect (so any filesystem-artifact recovery
/// channel writes under the tempdir), a fixture-unique marker string for
/// pgrep-based PID discovery, and the ability to drive multiple runs against
/// the same isolated config/data home.
pub struct PipelineFixture {
    pub dir: tempfile::TempDir,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub state_home: PathBuf,
    pub models_dir: PathBuf,
    pub fixture_marker: String,
}

impl PipelineFixture {
    /// Build a fixture whose fake provider executes `script_body`. The body
    /// is wrapped in a `#!/usr/bin/env bash` shebang.
    pub fn with_script_body(script_body: &str) -> Self {
        let paths = create_pipeline_fixture_dirs();
        let script_path = write_executable_provider_script(&paths.dir, script_body);
        write_fixture_model_config(&paths.models_dir);
        write_fixture_provider_config(&paths.app_config_dir, &script_path);
        assemble_pipeline_fixture(paths)
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    pub fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    /// Apply the fixture's XDG overrides to a `Command`.
    pub fn apply_env(&self, cmd: &mut Command) {
        apply_fixture_xdg_env(cmd, self);
        remove_parent_invocation_env(cmd);
    }
}

fn apply_fixture_xdg_env(cmd: &mut Command, fixture: &PipelineFixture) {
    cmd.env("XDG_CONFIG_HOME", &fixture.config_home);
    cmd.env("XDG_DATA_HOME", &fixture.data_home);
    cmd.env(
        "OULIPOLY_DATA_DIR",
        fixture.data_home.join("oulipoly-agent-runner"),
    );
    cmd.env("XDG_STATE_HOME", &fixture.state_home);
}

fn remove_parent_invocation_env(cmd: &mut Command) {
    cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
}

struct PipelineFixturePaths {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    state_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

fn create_pipeline_fixture_dirs() -> PipelineFixturePaths {
    let paths = pipeline_fixture_paths(create_pipeline_tempdir());
    create_pipeline_fixture_path_dirs(&paths);
    paths
}

fn create_pipeline_tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn pipeline_fixture_paths(dir: tempfile::TempDir) -> PipelineFixturePaths {
    let config_home = dir.path().join("config");
    let data_home = dir.path().join("data");
    let state_home = dir.path().join("state");
    let app_config_dir = config_home.join("oulipoly-agent-runner");
    let models_dir = app_config_dir.join("models");

    PipelineFixturePaths {
        dir,
        config_home,
        data_home,
        state_home,
        app_config_dir,
        models_dir,
    }
}

fn create_pipeline_fixture_path_dirs(paths: &PipelineFixturePaths) {
    create_fixture_dir(&paths.models_dir);
    create_fixture_dir(&paths.state_home);
    create_fixture_dir(&paths.data_home);
}

fn create_fixture_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
}

fn write_executable_provider_script(dir: &tempfile::TempDir, script_body: &str) -> PathBuf {
    let script_path = provider_script_path(dir);
    write_provider_script(&script_path, script_body);
    mark_executable(&script_path);
    script_path
}

fn provider_script_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("fixture-provider.sh")
}

fn write_provider_script(path: &Path, script_body: &str) {
    write_text_file(path, &fixture_script_text(script_body));
}

fn fixture_script_text(script_body: &str) -> String {
    format!("#!/usr/bin/env bash\n{script_body}\n")
}

fn mark_executable(path: &Path) {
    write_permissions(path, executable_permissions(read_permissions(path)));
}

fn read_permissions(path: &Path) -> fs::Permissions {
    fs::metadata(path).unwrap().permissions()
}

fn executable_permissions(mut permissions: fs::Permissions) -> fs::Permissions {
    permissions.set_mode(0o755);
    permissions
}

fn write_permissions(path: &Path, permissions: fs::Permissions) {
    fs::set_permissions(path, permissions).unwrap();
}

fn write_fixture_model_config(models_dir: &Path) {
    write_text_file(&fixture_model_config_path(models_dir), fixture_model_toml());
}

fn fixture_model_config_path(models_dir: &Path) -> PathBuf {
    models_dir.join("fixture.toml")
}

fn fixture_model_toml() -> &'static str {
    r#"[[providers]]
name = "fixture-provider"
"#
}

fn write_fixture_provider_config(app_config_dir: &Path, script_path: &Path) {
    write_text_file(
        &fixture_provider_config_path(app_config_dir),
        &fixture_provider_toml(script_path),
    );
}

fn fixture_provider_config_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join("providers.toml")
}

fn fixture_provider_toml(script_path: &Path) -> String {
    format!(
        r#"[fixture-provider]
command = "{}"
args = []
prompt_mode = "arg"
"#,
        script_path.display()
    )
}

fn fixture_marker_for_dir(dir: &tempfile::TempDir) -> String {
    dir.path().display().to_string()
}

fn write_text_file(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
}

fn assemble_pipeline_fixture(paths: PipelineFixturePaths) -> PipelineFixture {
    let fixture_marker = fixture_marker_for_dir(&paths.dir);

    PipelineFixture {
        dir: paths.dir,
        config_home: paths.config_home,
        data_home: paths.data_home,
        state_home: paths.state_home,
        models_dir: paths.models_dir,
        fixture_marker,
    }
}

/// Poll the fixture's `state.db` for the first row whose `status='running'`
/// has the expected `model_name`. Returns the `invocation_uuid` once visible
/// or `None` on timeout.
pub fn poll_first_running_uuid(fixture: &PipelineFixture, timeout: Duration) -> Option<String> {
    poll_until_running_uuid(fixture, timeout)
}

fn poll_until_running_uuid(fixture: &PipelineFixture, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(uuid) = try_read_first_running_uuid(fixture) {
            return Some(uuid);
        }
        sleep_between_running_polls();
    }
    None
}

fn try_read_first_running_uuid(fixture: &PipelineFixture) -> Option<String> {
    if !state_db_exists(fixture) {
        return None;
    }
    open_fixture_sqlite(fixture)
        .ok()
        .and_then(|conn| first_running_invocation_uuid(&conn))
}

fn state_db_exists(fixture: &PipelineFixture) -> bool {
    fixture.db_path().exists()
}

fn open_fixture_sqlite(fixture: &PipelineFixture) -> rusqlite::Result<rusqlite::Connection> {
    rusqlite::Connection::open(fixture.db_path())
}

fn first_running_invocation_uuid(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row(
        first_running_invocation_uuid_sql(),
        [],
        running_invocation_uuid_from_row,
    )
    .ok()
}

fn first_running_invocation_uuid_sql() -> &'static str {
    "SELECT invocation_uuid FROM invocations \
     WHERE status = 'running' \
     ORDER BY id ASC LIMIT 1"
}

fn running_invocation_uuid_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get::<_, String>(0)
}

fn sleep_between_running_polls() {
    thread::sleep(Duration::from_millis(25));
}

/// Search `/proc/*/cmdline` for the agent-runner process belonging to this
/// fixture. We match strictly on `argv[0]` (the first null-terminated token
/// in `/proc/<pid>/cmdline`) ending in `oulipoly-agent-runner`, AND on the
/// fixture-unique tempdir marker appearing somewhere later in `cmdline`.
/// The `argv[0]` check excludes the surrounding `bash -c '<full command>'`
/// wrapper, whose `argv[0]` is `bash` but whose cmdline still contains the
/// runner path as a substring of `argv[2]`.
///
/// Returns the PID, or `None` if no matching process is currently alive.
pub fn find_runner_pid_for_fixture(fixture: &PipelineFixture) -> Option<i32> {
    let self_pid = current_process_pid();
    proc_pid_entries().into_iter().find_map(|pid| {
        if pid == self_pid {
            return None;
        }
        read_proc_cmdline(pid)
            .ok()
            .filter(|cmdline| is_runner_cmdline(cmdline))
            .filter(|cmdline| cmdline_matches_fixture_marker(cmdline, &fixture.fixture_marker))
            .map(|_| pid)
    })
}

fn current_process_pid() -> i32 {
    std::process::id() as i32
}

fn proc_pid_entries() -> Vec<i32> {
    collect_proc_pids(read_proc_dir_entries())
}

fn read_proc_dir_entries() -> Option<fs::ReadDir> {
    fs::read_dir("/proc").ok()
}

fn collect_proc_pids(entries: Option<fs::ReadDir>) -> Vec<i32> {
    entries.map(readable_proc_pids).unwrap_or_default()
}

fn readable_proc_pids(entries: fs::ReadDir) -> Vec<i32> {
    let candidates = proc_pid_candidates(entries);
    collect_into_pids(candidates)
}

fn proc_pid_candidates(entries: fs::ReadDir) -> Vec<fs::DirEntry> {
    readable_dir_entry_values(entries)
        .into_iter()
        .filter(is_proc_pid_entry)
        .collect()
}

fn collect_into_pids(entries: Vec<fs::DirEntry>) -> Vec<i32> {
    entries.into_iter().map(proc_entry_pid).collect()
}

fn is_proc_pid_entry(entry: &fs::DirEntry) -> bool {
    parse_proc_entry_pid(entry).is_some()
}

fn proc_entry_pid(entry: fs::DirEntry) -> i32 {
    parse_proc_entry_pid(&entry).expect("proc PID candidate must parse after filtering")
}

fn parse_proc_entry_pid(entry: &fs::DirEntry) -> Option<i32> {
    parse_proc_pid(&proc_entry_file_name(entry))
}

fn proc_entry_file_name(entry: &fs::DirEntry) -> String {
    entry.file_name().to_string_lossy().into_owned()
}

fn parse_proc_pid(file_name: &str) -> Option<i32> {
    file_name.parse().ok()
}

fn read_proc_cmdline(pid: i32) -> std::io::Result<Vec<u8>> {
    read_proc_cmdline_path(&proc_cmdline_path(pid))
}

fn proc_cmdline_path(pid: i32) -> PathBuf {
    PathBuf::from(format!("/proc/{pid}/cmdline"))
}

fn read_proc_cmdline_path(path: &Path) -> std::io::Result<Vec<u8>> {
    fs::read(path)
}

fn is_runner_cmdline(cmdline: &[u8]) -> bool {
    argv0_basename_from_cmdline(cmdline) == "oulipoly-agent-runner"
}

fn argv0_basename_from_cmdline(cmdline: &[u8]) -> &str {
    basename_from_argv0(parse_cmdline_argv0(cmdline))
}

fn parse_cmdline_argv0(cmdline: &[u8]) -> &str {
    let argv0_end = cmdline
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(cmdline.len());
    let argv0 = &cmdline[..argv0_end];
    std::str::from_utf8(argv0).unwrap_or("")
}

fn basename_from_argv0(argv0: &str) -> &str {
    argv0.rsplit('/').next().unwrap_or(argv0)
}

fn cmdline_matches_fixture_marker(cmdline: &[u8], marker: &str) -> bool {
    contains_subslice(cmdline, marker.as_bytes())
}

/// Send SIGKILL to `pid` via `kill(1)`. Returns `true` iff `kill` reported
/// success. The kernel marks the target terminated synchronously, but if the
/// target is a child of someone other than the test it will linger as a
/// zombie until its real parent reaps it — callers should not poll
/// `/proc/<pid>/` or `kill -0` for "gone" status; instead, wait on the
/// parent (e.g. the bash wrapper) via `Child::wait`.
pub fn sigkill_pid(pid: i32) -> bool {
    run_kill_command(kill_command(pid))
        .map(command_status_success)
        .unwrap_or(false)
}

fn kill_command(pid: i32) -> Command {
    let mut command = Command::new("kill");
    command.arg("-KILL").arg(kill_pid_arg(pid));
    command
}

fn kill_pid_arg(pid: i32) -> String {
    pid.to_string()
}

fn run_kill_command(mut command: Command) -> std::io::Result<std::process::ExitStatus> {
    command.status()
}

fn command_status_success(status: std::process::ExitStatus) -> bool {
    status.success()
}

/// Test whether `haystack` contains `needle` as a contiguous byte
/// substring. Mirrors `slice::contains` for slice-of-slice on stable.
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Walk a directory recursively and return the paths of every regular file
/// found beneath it. Used to inspect `XDG_STATE_HOME` / `XDG_DATA_HOME` for
/// any filesystem-based terminal-status artifact the runtime may produce.
pub fn list_files_recursive(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_regular_files(root, &mut out);
    out
}

fn walk_regular_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in read_dir_entries(dir) {
        if let Some(action) = walk_action_for_entry(entry) {
            apply_walk_action(action, out);
        }
    }
}

fn walk_action_for_entry(entry: fs::DirEntry) -> Option<WalkAction> {
    entry_path_and_classification(entry).map(walk_action_for_classified_path)
}

fn walk_action_for_classified_path(classified_path: (PathBuf, FsEntryKind)) -> WalkAction {
    let (path, classification) = classified_path;
    match classification {
        FsEntryKind::Directory => WalkAction::Recurse(path),
        FsEntryKind::RegularFile => WalkAction::Collect(path),
        FsEntryKind::Other => WalkAction::Ignore,
    }
}

fn apply_walk_action(action: WalkAction, out: &mut Vec<PathBuf>) {
    match action {
        WalkAction::Recurse(path) => walk_regular_files(&path, out),
        WalkAction::Collect(path) => out.push(path),
        WalkAction::Ignore => {}
    }
}

fn read_dir_entries(dir: &Path) -> Vec<fs::DirEntry> {
    collect_readable_dir_entries(read_dir_entries_result(dir))
}

fn read_dir_entries_result(dir: &Path) -> std::io::Result<fs::ReadDir> {
    fs::read_dir(dir)
}

fn collect_readable_dir_entries(entries: std::io::Result<fs::ReadDir>) -> Vec<fs::DirEntry> {
    entries.map(readable_dir_entries).unwrap_or_default()
}

fn readable_dir_entries(entries: fs::ReadDir) -> Vec<fs::DirEntry> {
    readable_dir_entry_values(entries)
}

fn readable_dir_entry_values(entries: fs::ReadDir) -> Vec<fs::DirEntry> {
    let readable_entries = readable_dir_entry_results(entries);
    collect_readable_dir_entry_values(readable_entries)
}

fn readable_dir_entry_results(entries: fs::ReadDir) -> Vec<std::io::Result<fs::DirEntry>> {
    entries.filter(is_readable_dir_entry_result).collect()
}

fn is_readable_dir_entry_result(entry: &std::io::Result<fs::DirEntry>) -> bool {
    entry.is_ok()
}

fn collect_readable_dir_entry_values(
    entries: Vec<std::io::Result<fs::DirEntry>>,
) -> Vec<fs::DirEntry> {
    entries.into_iter().map(unwrap_readable_dir_entry).collect()
}

fn unwrap_readable_dir_entry(entry: std::io::Result<fs::DirEntry>) -> fs::DirEntry {
    entry.expect("readable directory entry must unwrap after filtering")
}

fn entry_path_and_classification(entry: fs::DirEntry) -> Option<(PathBuf, FsEntryKind)> {
    entry_path_and_metadata(&entry).map(classify_entry_path)
}

fn entry_path_and_metadata(entry: &fs::DirEntry) -> Option<(PathBuf, fs::Metadata)> {
    Some((entry_path(entry), entry_metadata(entry)?))
}

fn entry_path(entry: &fs::DirEntry) -> PathBuf {
    entry.path()
}

fn entry_metadata(entry: &fs::DirEntry) -> Option<fs::Metadata> {
    entry.metadata().ok()
}

fn classify_entry_path(path_and_metadata: (PathBuf, fs::Metadata)) -> (PathBuf, FsEntryKind) {
    let (path, metadata) = path_and_metadata;
    (path, classify_fs_entry(&metadata))
}

enum WalkAction {
    Recurse(PathBuf),
    Collect(PathBuf),
    Ignore,
}

enum FsEntryKind {
    Directory,
    RegularFile,
    Other,
}

fn classify_fs_entry(metadata: &fs::Metadata) -> FsEntryKind {
    if metadata.is_dir() {
        return FsEntryKind::Directory;
    }
    if metadata.is_file() {
        return FsEntryKind::RegularFile;
    }
    FsEntryKind::Other
}

/// Recognize only shipped terminal evidence for the invocation: exact
/// `OULIPOLY_TERMINAL_SIGNAL=` marker JSON, exact `OULIPOLY_RESULT=` marker
/// JSON, or one-line raw result-envelope JSON with the strict AGE-153/154 key
/// shape.
pub fn text_contains_terminal_envelope(text: &str, invocation_uuid: &str) -> bool {
    text.lines().any(|line| {
        strict_marker_line_recovers_terminal(line, invocation_uuid)
            || strict_raw_result_json_recovers_terminal(line, invocation_uuid)
    })
}

/// Returns true if any file under `root` carries strict terminal evidence:
/// a `.result` file containing raw result-envelope JSON, or any file
/// containing an exact terminal-signal/result marker line.
pub fn filesystem_artifact_recovers_terminal(root: &Path, invocation_uuid: &str) -> bool {
    list_files_recursive(root)
        .into_iter()
        .any(|path| artifact_recovers_terminal(&path, invocation_uuid))
}

fn artifact_recovers_terminal(path: &Path, invocation_uuid: &str) -> bool {
    let role = classify_artifact_role(path);
    read_artifact_text(path)
        .map(|contents| artifact_contents_recover_terminal(role, &contents, invocation_uuid))
        .unwrap_or(false)
}

enum ArtifactRole {
    TerminalResultFile,
    OtherArtifact,
}

fn classify_artifact_role(path: &Path) -> ArtifactRole {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.ends_with(".result") {
        ArtifactRole::TerminalResultFile
    } else {
        ArtifactRole::OtherArtifact
    }
}

fn read_artifact_text(path: &Path) -> std::io::Result<String> {
    fs::read_to_string(path)
}

fn artifact_contents_recover_terminal(
    role: ArtifactRole,
    contents: &str,
    invocation_uuid: &str,
) -> bool {
    let raw_result_matches = matches!(role, ArtifactRole::TerminalResultFile)
        && (strict_raw_result_json_recovers_terminal(contents, invocation_uuid)
            || contents
                .lines()
                .any(|line| strict_raw_result_json_recovers_terminal(line, invocation_uuid)));

    raw_result_matches
        || contents
            .lines()
            .any(|line| strict_marker_line_recovers_terminal(line, invocation_uuid))
}

fn strict_marker_line_recovers_terminal(line: &str, invocation_uuid: &str) -> bool {
    strict_terminal_signal_marker_recovers_terminal(line, invocation_uuid)
        || strict_result_marker_recovers_terminal(line, invocation_uuid)
}

fn strict_terminal_signal_marker_recovers_terminal(line: &str, invocation_uuid: &str) -> bool {
    let terminal_signal_lines = age153_support::terminal_signal_lines(line);
    if terminal_signal_lines.len() != 1 || terminal_signal_lines[0] != line {
        return false;
    }

    let Some(raw) = line.strip_prefix("OULIPOLY_TERMINAL_SIGNAL=") else {
        return false;
    };
    serde_json::from_str(raw)
        .ok()
        .is_some_and(|value| is_terminal_signal_envelope_for_invocation(&value, invocation_uuid))
}

fn strict_result_marker_recovers_terminal(line: &str, invocation_uuid: &str) -> bool {
    let Some(raw) = line.strip_prefix("OULIPOLY_RESULT=") else {
        return false;
    };
    serde_json::from_str(raw)
        .ok()
        .is_some_and(|value| is_result_envelope_for_invocation(&value, invocation_uuid))
}

fn strict_raw_result_json_recovers_terminal(text: &str, invocation_uuid: &str) -> bool {
    serde_json::from_str(text)
        .ok()
        .is_some_and(|value| is_result_envelope_for_invocation(&value, invocation_uuid))
}

fn is_terminal_signal_envelope_for_invocation(value: &Value, invocation_uuid: &str) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object_key_set(object) != BTreeSet::from(["evidence", "invocation_id", "kind", "session_id"])
    {
        return false;
    }

    value["invocation_id"].as_str() == Some(invocation_uuid)
        && uuid::Uuid::parse_str(invocation_uuid).is_ok()
        && value["evidence"].is_object()
        && value["kind"].is_string()
        && session_id_is_null_or_uuid(&value["session_id"])
}

fn is_result_envelope_for_invocation(value: &Value, invocation_uuid: &str) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let Some(success) = value["success"].as_bool() else {
        return false;
    };

    let base_keys = BTreeSet::from([
        "error_category",
        "exit_code",
        "finished_at",
        "id",
        "status",
        "success",
        "terminal_reason",
    ]);
    let expected_keys = if success {
        base_keys
    } else {
        let mut expected = base_keys;
        expected.extend([
            "agent_runner_invocation_id",
            "provider_name",
            "provider_session_id",
            "agent_runner_chain_id",
        ]);
        expected
    };

    if object_key_set(object) != expected_keys {
        return false;
    }

    value["id"].as_str() == Some(invocation_uuid)
        && (success || value["agent_runner_invocation_id"].as_str() == Some(invocation_uuid))
}

fn object_key_set(object: &serde_json::Map<String, Value>) -> BTreeSet<&str> {
    object.keys().map(String::as_str).collect()
}

pub fn parse_pre_invocation_failure_marker(line: &str) -> Option<Value> {
    decode_pre_invocation_failure_marker(line).filter(pre_invocation_failure_payload_is_strict)
}

fn decode_pre_invocation_failure_marker(line: &str) -> Option<Value> {
    let raw = line.strip_prefix("OULIPOLY_FAILURE=")?;
    serde_json::from_str(raw).ok()
}

fn pre_invocation_failure_payload_is_strict(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object_key_set(object)
        != BTreeSet::from([
            "agent_runner_invocation_id",
            "provider_name",
            "provider_session_id",
            "agent_runner_chain_id",
            "failure_kind",
            "stage",
            "status",
            "success",
            "exit_code",
            "terminal_reason",
            "error_category",
            "finished_at",
            "message",
            "detail",
        ])
    {
        return false;
    }
    if value["failure_kind"] != "pre_invocation"
        || value["status"] != "failed"
        || value["success"] != false
        || value["terminal_reason"] != "pre_invocation_failure"
        || !value["exit_code"].is_null()
        || !value["error_category"].is_null()
        || !value["agent_runner_invocation_id"].is_null()
        || !value["provider_name"].is_null()
        || !value["provider_session_id"].is_null()
        || !value["agent_runner_chain_id"].is_null()
        || !value["finished_at"].is_string()
        || !value["message"].is_string()
    {
        return false;
    }
    matches!(
        value["stage"].as_str(),
        Some("provider_selection" | "provider_resolution" | "pool_exhausted")
    ) && pre_invocation_failure_detail_is_strict(&value["detail"])
}

fn pre_invocation_failure_detail_is_strict(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object_key_set(object)
        == BTreeSet::from([
            "model_name",
            "provider_index",
            "attempted_providers",
            "reason",
        ])
        && string_or_null(&value["model_name"])
        && number_or_null(&value["provider_index"])
        && string_or_null(&value["reason"])
        && value["attempted_providers"]
            .as_array()
            .is_some_and(|providers| providers.iter().all(Value::is_string))
}

fn string_or_null(value: &Value) -> bool {
    value.is_string() || value.is_null()
}

fn number_or_null(value: &Value) -> bool {
    value.is_number() || value.is_null()
}

fn session_id_is_null_or_uuid(value: &Value) -> bool {
    value.is_null()
        || value
            .as_str()
            .is_some_and(|session_id| uuid::Uuid::parse_str(session_id).is_ok())
}
