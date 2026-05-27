#![cfg(unix)]
//! Declared roles: validator, mapper, orchestration, formatter, accessor, parser

#[path = "../src/commands/schema_probe/formatter.rs"]
mod formatter;
#[path = "../src/json_error.rs"]
mod json_error;
#[path = "../src/commands/schema_probe/mapper.rs"]
mod mapper;

use formatter::*;
use mapper::*;
use oulipoly_state::ReadOnlyOpenError;
use oulipoly_state::schema_probe::{ProbeError, SchemaProbeReport, StateDbReport};
use std::collections::BTreeMap;
use std::io;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::ExitStatus;

const CAPTURE_CHILD_ENV: &str = "AGE_192_CAPTURE_CHILD";

#[test]
fn schema_probe_report_is_incompatible_returns_true_iff_existing_and_not_compatible() {
    for (exists, compatible, expected) in [
        (false, false, false),
        (false, true, false),
        (true, false, true),
        (true, true, false),
    ] {
        let report =
            schema_probe_report(PathBuf::from("/tmp/age-192-state.db"), exists, compatible);

        assert_eq!(
            schema_probe_report_is_incompatible(&report),
            expected,
            "{}",
            expected_report_incompatibility_case_label(exists, compatible)
        );
    }
}

#[test]
fn format_schema_incompatible_message_preserves_state_db_display_path() {
    let path = PathBuf::from("/tmp/age-192/schema probe/state.db");
    let report = schema_probe_report(path.clone(), true, false);

    assert_eq!(
        format_schema_incompatible_message(&report),
        expected_incompatible_message(&path)
    );
}

#[test]
fn probe_error_message_passes_state_path_and_inspect_messages_verbatim() {
    assert_eq!(
        probe_error_message(ProbeError::StatePath {
            message: "deployment primary failed".to_string(),
        }),
        "deployment primary failed"
    );
    assert_eq!(
        probe_error_message(ProbeError::Inspect {
            message: "sqlite_schema query failed".to_string(),
        }),
        "sqlite_schema query failed"
    );
}

#[test]
fn probe_error_message_maps_read_only_open_error_variants_exactly() {
    let missing = PathBuf::from("/tmp/age-192/missing-state.db");
    let not_a_database = PathBuf::from("/tmp/age-192/not-a-db");
    let permission_denied = PathBuf::from("/tmp/age-192/denied-state.db");
    let wal_sidecar = PathBuf::from("/tmp/age-192/sidecar-state.db");

    let cases = [
        (
            ProbeError::Open {
                error: ReadOnlyOpenError::Missing {
                    path: missing.clone(),
                },
            },
            expected_read_only_open_message_missing(&missing),
        ),
        (
            ProbeError::Open {
                error: ReadOnlyOpenError::NotADatabase {
                    path: not_a_database.clone(),
                    message: "file is not a database".to_string(),
                },
            },
            expected_read_only_open_message_not_a_database(
                &not_a_database,
                "file is not a database",
            ),
        ),
        (
            ProbeError::Open {
                error: ReadOnlyOpenError::PermissionDenied {
                    path: permission_denied.clone(),
                },
            },
            expected_read_only_open_message_permission_denied(&permission_denied),
        ),
        (
            ProbeError::Open {
                error: ReadOnlyOpenError::WalSidecarError {
                    path: wal_sidecar.clone(),
                    message: "SQLite sidecar is not readable".to_string(),
                },
            },
            expected_read_only_open_message_wal_sidecar(
                &wal_sidecar,
                "SQLite sidecar is not readable",
            ),
        ),
        (
            ProbeError::Open {
                error: ReadOnlyOpenError::Operational {
                    message: "disk I/O failed".to_string(),
                },
            },
            "disk I/O failed".to_string(),
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(probe_error_message(error), expected);
    }
}

#[test]
fn render_schema_probe_report_incompatible_returns_14_and_emits_schema_incompatible_json() {
    run_capture_child("age_192_child_render_incompatible_report");
}

#[test]
fn render_schema_probe_report_compatible_returns_0_and_emits_compact_report_json_line() {
    run_capture_child("age_192_child_render_compatible_report");
}

#[test]
fn render_schema_probe_error_returns_1_and_emits_operational_error_json() {
    run_capture_child("age_192_child_render_probe_error");
}

#[test]
#[ignore]
fn age_192_child_render_incompatible_report() {
    if std::env::var_os(CAPTURE_CHILD_ENV).is_none() {
        return;
    }

    let path = PathBuf::from("/tmp/age-192/incompatible-state.db");
    let report = schema_probe_report(path.clone(), true, false);

    let (result, stderr) = capture_stderr(|| render_schema_probe_report(&report));

    assert_eq!(result, Ok(14));
    assert_eq!(stderr, expected_schema_incompatible_envelope(&path));
}

#[test]
#[ignore]
fn age_192_child_render_compatible_report() {
    if std::env::var_os(CAPTURE_CHILD_ENV).is_none() {
        return;
    }

    let report = schema_probe_report(
        PathBuf::from("/tmp/age-192/compatible-state.db"),
        true,
        true,
    );
    let (result, stdout) = capture_stdout(|| render_schema_probe_report(&report));

    assert_eq!(result, Ok(0));
    assert_eq!(stdout, expected_compatible_report_stdout(&report));
}

#[test]
#[ignore]
fn age_192_child_render_probe_error() {
    if std::env::var_os(CAPTURE_CHILD_ENV).is_none() {
        return;
    }

    let error = ProbeError::Open {
        error: ReadOnlyOpenError::Operational {
            message: "database connection failed".to_string(),
        },
    };

    let (result, stderr) = capture_stderr(|| render_schema_probe_error(error));

    assert_eq!(result, Ok(1));
    assert_eq!(
        stderr,
        expected_operational_envelope("database connection failed")
    );
}

fn schema_probe_report(path: PathBuf, exists: bool, compatible: bool) -> SchemaProbeReport {
    oulipoly_state::schema_probe::report_from_state_db(StateDbReport {
        path,
        exists,
        schema_version: 7,
        user_version: 7,
        current_schema_version: 7,
        minimum_supported_schema_version: 2,
        compatible,
        migratable: false,
        tables: BTreeMap::new(),
        required_columns: BTreeMap::new(),
        required_indexes: BTreeMap::new(),
    })
}

fn run_capture_child(test_name: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(test_name)
        .arg("--ignored")
        .arg("--nocapture")
        .arg("--quiet")
        .env(CAPTURE_CHILD_ENV, "1")
        .output()
        .unwrap();

    let stdout = parse_child_output(&output.stdout);
    let stderr = parse_child_output(&output.stderr);
    assert_child_success(&output.status, &stdout, &stderr);
}

fn assert_child_success(status: &ExitStatus, stdout: &str, stderr: &str) {
    assert!(
        status.success(),
        "{}",
        format_child_failure_diagnostic(status, stdout, stderr)
    );
}

fn format_child_failure_diagnostic(status: &ExitStatus, stdout: &str, stderr: &str) -> String {
    format!(
        "child test failed with status {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        status.code()
    )
}

fn parse_child_output(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}

fn capture_stdout<T>(action: impl FnOnce() -> T) -> (T, String) {
    capture_fd(STDOUT_FILENO, action)
}

fn capture_stderr<T>(action: impl FnOnce() -> T) -> (T, String) {
    capture_fd(STDERR_FILENO, action)
}

#[cfg(unix)]
fn capture_fd<T>(fd: RawFd, action: impl FnOnce() -> T) -> (T, String) {
    use std::io::Write as _;
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

    let mut pipe_fds = [0; 2];
    assert_syscall_ok(
        unsafe { pipe(pipe_fds.as_mut_ptr()) },
        syscall_failure_diagnostic("pipe"),
    );
    let read_fd = pipe_fds[0];
    let write_fd = pipe_fds[1];
    let saved_fd = unsafe { dup(fd) };
    assert_syscall_ok(saved_fd, syscall_failure_diagnostic("dup"));
    assert_syscall_matches(
        unsafe { dup2(write_fd, fd) },
        fd,
        syscall_failure_diagnostic("dup2 redirect"),
    );
    assert_syscall_ok(
        unsafe { close(write_fd) },
        syscall_failure_diagnostic("close write fd"),
    );

    let result = catch_unwind(AssertUnwindSafe(action));
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    let _ = unsafe { fflush(std::ptr::null_mut()) };
    assert_syscall_matches(
        unsafe { dup2(saved_fd, fd) },
        fd,
        syscall_failure_diagnostic("dup2 restore"),
    );
    assert_syscall_ok(
        unsafe { close(saved_fd) },
        syscall_failure_diagnostic("close saved fd"),
    );

    let captured = read_captured_output(read_fd);

    match result {
        Ok(value) => (value, captured),
        Err(payload) => resume_unwind(payload),
    }
}

#[cfg(unix)]
fn assert_syscall_ok(result: i32, diagnostic: String) {
    assert!(result >= 0, "{diagnostic}");
}

#[cfg(unix)]
fn assert_syscall_matches(result: i32, expected: i32, diagnostic: String) {
    assert_eq!(result, expected, "{diagnostic}");
}

#[cfg(unix)]
fn syscall_failure_diagnostic(label: &str) -> String {
    format_syscall_failure(label, last_os_error())
}

#[cfg(unix)]
fn last_os_error() -> io::Error {
    io::Error::last_os_error()
}

#[cfg(unix)]
fn format_syscall_failure(label: &str, error: io::Error) -> String {
    format!("{label} failed: {error}")
}

#[cfg(unix)]
fn read_captured_output(fd: RawFd) -> String {
    use std::fs::File;
    use std::os::fd::FromRawFd;

    let mut reader = unsafe { File::from_raw_fd(fd) };
    let mut captured = String::new();
    reader.read_to_string(&mut captured).unwrap();
    captured
}

fn expected_report_incompatibility_case_label(exists: bool, compatible: bool) -> String {
    format!("exists={exists}, compatible={compatible}")
}

fn expected_incompatible_message(path: &Path) -> String {
    format!("state database schema is incompatible: {}", path.display())
}

fn expected_schema_incompatible_envelope(path: &Path) -> String {
    format!(
        "{{\"error\":{{\"code\":\"schema-incompatible\",\"message\":\"{}\"}}}}\n",
        expected_incompatible_message(path)
    )
}

fn expected_operational_envelope(message: &str) -> String {
    format!("{{\"error\":{{\"code\":\"operational-error\",\"message\":\"{message}\"}}}}\n")
}

fn expected_read_only_open_message_missing(path: &Path) -> String {
    format!("state database is missing: {}", path.display())
}

fn expected_read_only_open_message_not_a_database(path: &Path, message: &str) -> String {
    format!(
        "state database is not a SQLite database at {}: {message}",
        path.display()
    )
}

fn expected_read_only_open_message_permission_denied(path: &Path) -> String {
    format!(
        "permission denied reading state database at {}",
        path.display()
    )
}

fn expected_read_only_open_message_wal_sidecar(path: &Path, message: &str) -> String {
    format!(
        "failed to read SQLite WAL sidecar for state database at {}: {message}",
        path.display()
    )
}

fn expected_compatible_report_stdout(report: &SchemaProbeReport) -> String {
    format!("{}\n", serde_json::to_string(report).unwrap())
}

#[cfg(unix)]
type RawFd = std::os::fd::RawFd;

#[cfg(unix)]
const STDOUT_FILENO: RawFd = 1;
#[cfg(unix)]
const STDERR_FILENO: RawFd = 2;

#[cfg(unix)]
unsafe extern "C" {
    fn close(fd: std::os::raw::c_int) -> std::os::raw::c_int;
    fn dup(fd: std::os::raw::c_int) -> std::os::raw::c_int;
    fn dup2(fd: std::os::raw::c_int, fd2: std::os::raw::c_int) -> std::os::raw::c_int;
    fn fflush(stream: *mut std::os::raw::c_void) -> std::os::raw::c_int;
    fn pipe(fds: *mut std::os::raw::c_int) -> std::os::raw::c_int;
}
