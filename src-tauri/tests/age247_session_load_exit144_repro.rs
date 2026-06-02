#![cfg(target_os = "linux")]
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`, `validator`, `parser`, `accessor`, `predicate`

use rusqlite::{Connection, Row, params};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MODEL_NAME: &str = "age247-repro";
const PROVIDER_NAME: &str = "age247-provider";

struct ScratchPaths {
    root: PathBuf,
    data_home: PathBuf,
    config_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    db_path: PathBuf,
    provider_script: PathBuf,
    preload_source: PathBuf,
    preload_library: PathBuf,
    enospc_control: PathBuf,
}

struct ScratchFixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    data_home: PathBuf,
    config_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    db_path: PathBuf,
    provider_script: PathBuf,
    preload_source: PathBuf,
    preload_library: PathBuf,
    enospc_control: PathBuf,
}

impl ScratchFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = scratch_paths(dir.path());
        reset_dir(&paths.root);
        let fixture = fixture_from_paths(dir, paths);
        fixture.write_config();
        fixture.compile_enospc_preload();
        fixture
    }

    fn write_config(&self) {
        fs::create_dir_all(&self.models_dir).unwrap();
        fs::write(
            self.models_dir.join(format!("{MODEL_NAME}.toml")),
            model_config_contents(),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            providers_config_contents(&self.provider_script),
        )
        .unwrap();
        fs::write(
            &self.provider_script,
            provider_script_contents(&self.enospc_control),
        )
        .unwrap();
        let mut permissions = fs::metadata(&self.provider_script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&self.provider_script, permissions).unwrap();
    }

    fn compile_enospc_preload(&self) {
        fs::write(&self.preload_source, ENOSPC_PRELOAD_SOURCE).unwrap();
        let output = compile_preload_library(&self.preload_library, &self.preload_source);
        assert_compile_succeeded(&output);
    }

    fn command(&self) -> Command {
        fixture_command(self)
    }

    fn assert_all_isolated(&self) {
        self.assert_isolated(&self.root);
        self.assert_isolated(&self.data_home);
        self.assert_isolated(&self.config_home);
        self.assert_isolated(&self.db_path);
    }

    fn assert_isolated(&self, path: &Path) {
        assert!(
            path.starts_with(&self.root),
            "AGE-247 repro path must stay under isolated scratch root: {}",
            path.display()
        );
    }
}

fn scratch_paths(base: &Path) -> ScratchPaths {
    let root = base.join("agents-session-load-exit144");
    let data_home = root.join("xdg-data");
    let config_home = root.join("xdg-config");
    let app_config_dir = config_home.join("oulipoly-agent-runner");
    let models_dir = app_config_dir.join("models");
    let db_path = data_home.join("oulipoly-agent-runner").join("state.db");
    let provider_script = root.join("age247-provider.sh");
    let preload_source = root.join("age247-enospc.c");
    let preload_library = root.join("age247-enospc.so");
    let enospc_control = root.join("enable-enospc");

    ScratchPaths {
        root,
        data_home,
        config_home,
        app_config_dir,
        models_dir,
        db_path,
        provider_script,
        preload_source,
        preload_library,
        enospc_control,
    }
}

fn fixture_from_paths(dir: tempfile::TempDir, paths: ScratchPaths) -> ScratchFixture {
    ScratchFixture {
        _dir: dir,
        root: paths.root,
        data_home: paths.data_home,
        config_home: paths.config_home,
        app_config_dir: paths.app_config_dir,
        models_dir: paths.models_dir,
        db_path: paths.db_path,
        provider_script: paths.provider_script,
        preload_source: paths.preload_source,
        preload_library: paths.preload_library,
        enospc_control: paths.enospc_control,
    }
}

fn model_config_contents() -> String {
    format!(
        r#"[[providers]]
name = "{PROVIDER_NAME}"
args = []
"#
    )
}

fn providers_config_contents(provider_script: &Path) -> String {
    format!(
        r#"[{PROVIDER_NAME}]
command = "{}"
args = []
prompt_mode = "arg"
"#,
        provider_script.display()
    )
}

fn provider_script_contents(enospc_control: &Path) -> String {
    format!(
        "#!/usr/bin/env bash\n\
         set -euo pipefail\n\
         printf 'age247 provider reached terminal path\\n'\n\
         : > '{}'\n",
        enospc_control.display()
    )
}

fn compile_preload_library(preload_library: &Path, preload_source: &Path) -> Output {
    Command::new("cc")
        .arg("-shared")
        .arg("-fPIC")
        .arg("-O2")
        .arg("-ldl")
        .arg("-o")
        .arg(preload_library)
        .arg(preload_source)
        .output()
        .unwrap()
}

fn assert_compile_succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        compile_failure_message(output)
    );
}

fn compile_failure_message(output: &Output) -> String {
    format!(
        "failed to compile ENOSPC preload shim\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn fixture_command(fixture: &ScratchFixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    command
        .env("XDG_CONFIG_HOME", &fixture.config_home)
        .env("XDG_DATA_HOME", &fixture.data_home)
        .env("OULIPOLY_DATA_HOME", &fixture.data_home)
        .env("HOME", &fixture.root)
        .env("LD_PRELOAD", &fixture.preload_library)
        .env("AGE247_ENOSPC_CONTROL", &fixture.enospc_control)
        .env_remove("OULIPOLY_PARENT_INVOCATION")
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg(MODEL_NAME)
        .arg("exercise isolated session load under deterministic ENOSPC");
    command
}

#[derive(Debug, PartialEq, Eq)]
struct InvocationRow {
    status: String,
    finished_at: Option<String>,
    success: Option<i64>,
    exit_code: Option<i64>,
    terminal_reason: Option<String>,
}

fn reset_dir(path: &Path) {
    if path.exists() {
        fs::remove_dir_all(path).unwrap();
    }
    fs::create_dir_all(path).unwrap();
}

fn run_agents(command: &mut Command) -> Output {
    command.output().unwrap()
}

fn parse_invocation_uuid(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        let raw = line.trim().strip_prefix("OULIPOLY_INVOCATION=")?;
        let value: Value = serde_json::from_str(raw).ok()?;
        value.get("id")?.as_str().map(str::to_string)
    })
}

fn invocation_row(db_path: &Path, invocation_uuid: &str) -> Option<InvocationRow> {
    Connection::open(db_path)
        .unwrap()
        .query_row(
            "SELECT status, finished_at, success, exit_code, terminal_reason
             FROM invocations
             WHERE invocation_uuid = ?1",
            params![invocation_uuid],
            map_invocation_row,
        )
        .ok()
}

fn map_invocation_row(row: &Row<'_>) -> rusqlite::Result<InvocationRow> {
    Ok(InvocationRow {
        status: row.get(0)?,
        finished_at: row.get(1)?,
        success: row.get(2)?,
        exit_code: row.get(3)?,
        terminal_reason: row.get(4)?,
    })
}

struct ScenarioObservation {
    fixture: ScratchFixture,
    output: Output,
    stdout: String,
    stderr: String,
    invocation_uuid: Option<String>,
    terminal_row: Option<InvocationRow>,
}

#[test]
fn agents_terminal_state_enospc_after_session_load_returns_controlled_failure() {
    let observation = run_terminal_persistence_scenario();
    assert_controlled_terminal_persistence_failure(&observation);
}

fn run_terminal_persistence_scenario() -> ScenarioObservation {
    let fixture = ScratchFixture::new();
    fixture.assert_all_isolated();
    let output = run_agents(&mut fixture.command());
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let invocation_uuid = parse_invocation_uuid(&stderr);
    let terminal_row = invocation_uuid
        .as_deref()
        .and_then(|uuid| invocation_row(&fixture.db_path, uuid));

    scenario_observation(
        fixture,
        output,
        stdout,
        stderr,
        invocation_uuid,
        terminal_row,
    )
}

fn scenario_observation(
    fixture: ScratchFixture,
    output: Output,
    stdout: String,
    stderr: String,
    invocation_uuid: Option<String>,
    terminal_row: Option<InvocationRow>,
) -> ScenarioObservation {
    ScenarioObservation {
        fixture,
        output,
        stdout,
        stderr,
        invocation_uuid,
        terminal_row,
    }
}

fn assert_controlled_terminal_persistence_failure(observation: &ScenarioObservation) {
    assert_ne!(
        observation.output.status.signal(),
        Some(16),
        "AGE-247 reproduction: runner must not hard-kill with SIGSTKFLT / exit 144.\n{}",
        observation_failure_context(observation)
    );
    assert!(
        observation.output.status.signal().is_none(),
        "AGE-247 reproduction: storage-full handling must be controlled, not signal-killed.\n{}",
        observation_failure_context(observation)
    );
    assert_eq!(
        observation.output.status.code(),
        Some(1),
        "terminal persistence ENOSPC must return a controlled nonzero exit.\n{}",
        observation_failure_context(observation)
    );
    assert!(
        observation.stdout.contains("OULIPOLY_RESULT="),
        "controlled terminal persistence failure must emit a result envelope.\n{}",
        observation_failure_context(observation)
    );
    assert!(
        observation.stdout.contains(r#""success":false"#)
            && observation
                .stdout
                .contains(r#""error_category":"terminal_persistence"#)
            && observation
                .stdout
                .contains(r#""terminal_reason":"terminal_persistence_failed"#),
        "terminal persistence ENOSPC must not emit a successful result envelope.\n{}",
        observation_failure_context(observation)
    );
    assert!(
        !observation.stdout.contains(r#""success":true"#),
        "terminal persistence ENOSPC must suppress provider-success output.\n{}",
        observation_failure_context(observation)
    );
    assert!(
        observation
            .stderr
            .contains("[age247-enospc] injecting ENOSPC")
            && observation
                .stderr
                .contains("Warning: Failed to finalize invocation:")
            && observation
                .stderr
                .contains("Warning: Failed to finalize invocation in guard:"),
        "terminal persistence ENOSPC must keep guard recovery active after explicit finalize failure.\n{}",
        observation_failure_context(observation)
    );

    let Some(invocation_uuid) = observation.invocation_uuid.as_deref() else {
        panic!(
            "runner must either emit a controlled pre-invocation error or an invocation marker.\n{}",
            observation_failure_context(observation)
        );
    };
    let Some(row) = observation.terminal_row.as_ref() else {
        panic!(
            "invocation {invocation_uuid} must have a start DB row after isolated ENOSPC.\n{}",
            observation_failure_context(observation)
        );
    };

    assert!(
        row.status == "running" && row.finished_at.is_none() && row.success.is_none(),
        "while storage remains full, the DB row may be unfinalized but the process must not claim success.\n{}",
        observation_failure_context(observation)
    );
}

fn observation_failure_context(observation: &ScenarioObservation) -> String {
    failure_context(
        &observation.fixture,
        &observation.output,
        observation.invocation_uuid.as_deref(),
        observation.terminal_row.as_ref(),
    )
}

fn failure_context(
    fixture: &ScratchFixture,
    output: &Output,
    invocation_uuid: Option<&str>,
    terminal_row: Option<&InvocationRow>,
) -> String {
    format!(
        "scratch_root={}\nxdg_data_home={}\nxdg_config_home={}\ndb_path={}\nwal_path={}-wal\npreload_library={}\nenospc_control={}\ninvocation_uuid={invocation_uuid:?}\nterminal_row={terminal_row:?}\nstatus={:?}\nsignal={:?}\nstdout:\n{}\nstderr:\n{}",
        fixture.root.display(),
        fixture.data_home.display(),
        fixture.config_home.display(),
        fixture.db_path.display(),
        fixture.db_path.display(),
        fixture.preload_library.display(),
        fixture.enospc_control.display(),
        output.status,
        output.status.signal(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

const ENOSPC_PRELOAD_SOURCE: &str = r#"
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <unistd.h>

static const char *control_path(void) {
    return getenv("AGE247_ENOSPC_CONTROL");
}

static int control_file_exists(const char *path) {
    return path != NULL && access(path, F_OK) == 0;
}

static int fault_enabled(void) {
    return control_file_exists(control_path());
}

static int claim_log_once(void) {
    static int logged = 0;
    if (logged) {
        return 0;
    }
    logged = 1;
    return 1;
}

static const char *enospc_message(void) {
    return "[age247-enospc] injecting ENOSPC\n";
}

static void emit_stderr_message(const char *message) {
    typedef ssize_t (*write_fn)(int, const void *, size_t);
    static write_fn real_write = NULL;
    if (real_write == NULL) {
        real_write = (write_fn)dlsym(RTLD_NEXT, "write");
    }
    if (real_write != NULL) {
        real_write(STDERR_FILENO, message, strlen(message));
    }
}

static void log_once(void) {
    if (claim_log_once()) {
        emit_stderr_message(enospc_message());
    }
}

static ssize_t fail_with_enospc(int fd) {
    (void)fd;
    log_once();
    errno = ENOSPC;
    return -1;
}

ssize_t pwrite(int fd, const void *buf, size_t count, off_t offset) {
    typedef ssize_t (*real_fn)(int, const void *, size_t, off_t);
    static real_fn real = NULL;
    if (real == NULL) {
        real = (real_fn)dlsym(RTLD_NEXT, "pwrite");
    }
    if (fault_enabled()) {
        return fail_with_enospc(fd);
    }
    return real(fd, buf, count, offset);
}

ssize_t pwrite64(int fd, const void *buf, size_t count, off64_t offset) {
    typedef ssize_t (*real_fn)(int, const void *, size_t, off64_t);
    static real_fn real = NULL;
    if (real == NULL) {
        real = (real_fn)dlsym(RTLD_NEXT, "pwrite64");
    }
    if (fault_enabled()) {
        return fail_with_enospc(fd);
    }
    return real(fd, buf, count, offset);
}

ssize_t writev(int fd, const struct iovec *iov, int iovcnt) {
    typedef ssize_t (*real_fn)(int, const struct iovec *, int);
    static real_fn real = NULL;
    if (real == NULL) {
        real = (real_fn)dlsym(RTLD_NEXT, "writev");
    }
    if (fault_enabled()) {
        return fail_with_enospc(fd);
    }
    return real(fd, iov, iovcnt);
}
"#;
