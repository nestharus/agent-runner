#![cfg(unix)]

use oulipoly_state::diagnostic_recorder::{
    DiagnosticId, DiagnosticPhase, FlightRecorder, PhaseObservation, RecorderConfig, SpanStart,
};
use oulipoly_state::event_store::{
    Digest32, EventCorrelations, EventEnvelopeV1, EventFamily, EventId, EventKind,
    EventWriterConfig, NativeProcessIdentity, NewEventV1, PayloadNormalizationPolicy,
    ProcessEventWriter, ProcessInstanceId, ProducerIdentity, SpanId, TraceId, WriterInstanceId,
};
use oulipoly_state::{StateDb, mailbox::MailboxDb};
use rusqlite::Connection;
use serde_json::Value;
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

const DIAGNOSTIC_ID: &str = "11111111-1111-4111-8111-111111111111";

struct Fixture {
    _root: tempfile::TempDir,
    config_home: PathBuf,
    data_dir: PathBuf,
    recorder_root: PathBuf,
    state_path: PathBuf,
    mailbox_path: PathBuf,
}

impl Fixture {
    fn isolated() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config_home = root.path().join("config");
        let data_dir = root.path().join("data");
        let recorder_root = data_dir.join("diagnostics/flight-recorder-v1");
        let state_path = data_dir.join("state.db");
        let mailbox_path = data_dir.join("pid-identity.db");
        fs::create_dir_all(&config_home).unwrap();
        fs::create_dir_all(&recorder_root).unwrap();
        Self {
            _root: root,
            config_home,
            data_dir,
            recorder_root,
            state_path,
            mailbox_path,
        }
    }

    fn with_unavailable_stores() -> Self {
        let fixture = Self::isolated();
        // Directories at the two database leaf paths are deterministically
        // unavailable SQLite targets even with elevated filesystem privileges.
        fs::create_dir(&fixture.state_path).unwrap();
        fs::create_dir(&fixture.mailbox_path).unwrap();
        fixture
    }

    fn with_valid_stores() -> Self {
        let fixture = Self::isolated();
        drop(StateDb::open(&fixture.state_path).unwrap());
        drop(MailboxDb::open(&fixture.mailbox_path).unwrap());
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        command.env("OULIPOLY_DATA_DIR", &self.data_dir);
        command.env("OULIPOLY_CONFIG_HOME", &self.config_home);
        command.env("XDG_CONFIG_HOME", &self.config_home);
        command
    }

    fn assert_store_paths_remained_unavailable(&self) {
        assert!(self.state_path.is_dir());
        assert!(self.mailbox_path.is_dir());
        for suffix in ["-journal", "-shm", "-wal"] {
            assert!(!PathBuf::from(format!("{}{suffix}", self.state_path.display())).exists());
            assert!(!PathBuf::from(format!("{}{suffix}", self.mailbox_path.display())).exists());
        }
    }

    fn seed_failed_trace_with_truncated_tail(&self) {
        let diagnostic_id = DIAGNOSTIC_ID.parse::<DiagnosticId>().unwrap();
        let recorder =
            FlightRecorder::open(&self.recorder_root, RecorderConfig::default()).unwrap();
        recorder.with_requested_span(
            SpanStart::new("fixture.transaction", "state").with_diagnostic_id(diagnostic_id),
            |span| {
                span.record(
                    DiagnosticPhase::Failed,
                    PhaseObservation::started_unknown().with_cause("fixture failure"),
                );
            },
        );
        drop(recorder);

        let shard = fs::read_dir(&self.recorder_root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("flight-") && name.ends_with(".jsonl"))
            })
            .expect("recorder must create one active shard");
        // Deferred event append may still be draining after the last handle
        // drops; append the deliberate torn tail only after both full lines.
        for _ in 0..100 {
            if fs::read(&shard)
                .unwrap()
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count()
                >= 2
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            fs::read(&shard)
                .unwrap()
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count()
                >= 2
        );
        OpenOptions::new()
            .append(true)
            .open(shard)
            .unwrap()
            .write_all(b"{\"truncated\":")
            .unwrap();
    }
}

struct HeldWriteTransaction {
    connection: Connection,
    path: PathBuf,
    before_command: Vec<(String, Option<Vec<u8>>)>,
}

impl HeldWriteTransaction {
    fn begin(path: &Path) -> Self {
        let connection = Connection::open(path).unwrap();
        connection.busy_timeout(Duration::ZERO).unwrap();
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode, "delete");
        connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE age319_offline_boundary_probe (
                     id INTEGER PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT INTO age319_offline_boundary_probe (id, value)
                 VALUES (1, 'held-write-transaction');",
            )
            .unwrap();
        assert!(!connection.is_autocommit());
        let journal_path = sqlite_companion(path, "-journal");
        assert!(journal_path.is_file(), "{}", journal_path.display());
        let before_command = sqlite_artifacts(path);
        Self {
            connection,
            path: path.to_path_buf(),
            before_command,
        }
    }

    fn assert_held_and_unmodified(&self) {
        assert!(!self.connection.is_autocommit());
        let value: String = self
            .connection
            .query_row(
                "SELECT value FROM age319_offline_boundary_probe WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "held-write-transaction");
        assert_eq!(sqlite_artifacts(&self.path), self.before_command);

        let contender = Connection::open(&self.path).unwrap();
        contender.busy_timeout(Duration::ZERO).unwrap();
        let error = contender.execute_batch("BEGIN IMMEDIATE").unwrap_err();
        assert!(
            matches!(
                error.sqlite_error_code(),
                Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
                    | Some(rusqlite::ffi::ErrorCode::DatabaseLocked)
            ),
            "unexpected contender result for {}: {error}",
            self.path.display()
        );
    }

    fn rollback(self) {
        self.connection.execute_batch("ROLLBACK").unwrap();
        drop(self.connection);
        let probe = Connection::open(&self.path).unwrap();
        let table_count: i64 = probe
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type='table' AND name='age319_offline_boundary_probe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 0);
    }
}

fn sqlite_artifacts(path: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    ["", "-journal", "-wal", "-shm"]
        .into_iter()
        .map(|suffix| {
            let artifact = sqlite_companion(path, suffix);
            (suffix.to_string(), fs::read(artifact).ok())
        })
        .collect()
}

fn sqlite_companion(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

#[test]
fn recent_product_route_is_independent_of_unavailable_store_paths() {
    let fixture = Fixture::with_unavailable_stores();
    fixture.seed_failed_trace_with_truncated_tail();
    let output = fixture
        .command()
        .args(["diagnostics", "recent", "--limit", "10", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let json = stdout_json(&output);
    assert_eq!(json["command"], "diagnostics recent");
    assert_eq!(json["limit"], 10);
    assert!(json.get("recent").is_some(), "{json:#}");
    assert!(json.get("coalesced").is_some(), "{json:#}");
    assert_eq!(json["recent"]["coverage"]["files_seen"], 1);
    assert_eq!(
        json["recent"]["coverage"]["records_seen"], 2,
        "coverage counts all readable records before the failure filter: {json:#}"
    );
    assert_eq!(json["recent"]["events"].as_array().unwrap().len(), 1);
    assert_eq!(json["recent"]["coverage"]["skipped_records"], 1);
    assert_eq!(json["recent"]["issues"][0]["kind"], "truncated_tail");
    assert_eq!(json["recent"]["issues"][0]["source"]["line"], 3);
    assert_eq!(
        json["recent"]["events"][0]["event"]["diagnostic_id"],
        DIAGNOSTIC_ID
    );
    assert_eq!(json["recent"]["events"][0]["source"]["line"], 2);
    assert_eq!(json["coalesced"]["failures"][0]["count"], 1);
    assert_eq!(json["coalesced"]["failures"][0]["sources"][0]["line"], 2);
    assert_eq!(json["coalesced"]["issues"][0]["kind"], "truncated_tail");
    assert_eq!(json["recent"]["coverage"], json["coalesced"]["coverage"]);
    assert_eq!(json["recent"]["issues"], json["coalesced"]["issues"]);
    assert!(fixture.recorder_root.is_dir());
    fixture.assert_store_paths_remained_unavailable();
}

#[test]
fn trace_product_route_reports_coverage_with_unavailable_store_paths() {
    let fixture = Fixture::with_unavailable_stores();
    fixture.seed_failed_trace_with_truncated_tail();
    let output = fixture
        .command()
        .args(["diagnostics", "trace", DIAGNOSTIC_ID, "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let json = stdout_json(&output);
    assert_eq!(json["command"], "diagnostics trace");
    assert_eq!(json["diagnostic_id"], DIAGNOSTIC_ID);
    assert!(json.get("trace").is_some(), "{json:#}");
    assert_eq!(json["trace"]["coverage"]["files_seen"], 1);
    assert_eq!(json["trace"]["coverage"]["records_seen"], 2);
    assert_eq!(json["trace"]["coverage"]["skipped_records"], 1);
    assert_eq!(json["trace"]["events"][0]["event"]["phase"], "requested");
    assert_eq!(json["trace"]["events"][1]["event"]["phase"], "failed");
    assert_eq!(json["trace"]["issues"][0]["kind"], "truncated_tail");
    fixture.assert_store_paths_remained_unavailable();
}

#[test]
fn recent_product_route_succeeds_while_valid_stores_have_live_write_transactions() {
    let fixture = Fixture::with_valid_stores();
    fixture.seed_failed_trace_with_truncated_tail();
    let state_writer = HeldWriteTransaction::begin(&fixture.state_path);
    let mailbox_writer = HeldWriteTransaction::begin(&fixture.mailbox_path);

    let output = fixture
        .command()
        .args(["diagnostics", "recent", "--limit", "10", "--json"])
        .output()
        .unwrap();

    assert_success(&output);
    let json = stdout_json(&output);
    assert_eq!(json["command"], "diagnostics recent");
    assert_eq!(json["recent"]["events"].as_array().unwrap().len(), 1);
    assert_eq!(json["coalesced"]["failures"][0]["count"], 1);
    state_writer.assert_held_and_unmodified();
    mailbox_writer.assert_held_and_unmodified();
    state_writer.rollback();
    mailbox_writer.rollback();
}

#[test]
fn human_recent_output_keeps_coverage_and_coalesced_sections() {
    let fixture = Fixture::with_unavailable_stores();
    let output = fixture
        .command()
        .args(["diagnostics", "recent", "--limit", "3"])
        .output()
        .unwrap();

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("diagnostics recent limit=3\n"),
        "{stdout}"
    );
    assert!(stdout.contains("recent failures:\n"), "{stdout}");
    assert!(stdout.contains("coalesced failures:\n"), "{stdout}");
    assert!(stdout.contains("\"coverage\""), "{stdout}");
    fixture.assert_store_paths_remained_unavailable();
}

fn normal_diagnostic_envelope(writer: WriterInstanceId, event: EventId) -> EventEnvelopeV1 {
    let process = ProcessInstanceId::from_bytes([21; 16]);
    let now = chrono::Utc::now().timestamp_micros();
    EventEnvelopeV1::normalize(
        NewEventV1 {
            event_id: event,
            family: EventFamily::Diagnostic,
            kind: EventKind::registered("diagnostic.observation").unwrap(),
            recorded_at_unix_micros: now,
            producer_sequence: i64::from(event.as_bytes()[0]),
            producer: ProducerIdentity {
                writer_instance_id: writer,
                process_instance_id: process,
                process_root_id: process,
                parent_process_instance_id: None,
                supervisor_authority_id: None,
                native_process: Some(NativeProcessIdentity {
                    os_pid: 1234,
                    os_boot_id_sha256: Digest32::from_bytes([8; 32]),
                    os_pid_starttime_ticks: 100,
                }),
            },
            correlations: EventCorrelations {
                trace_id: Some(TraceId::from(uuid::Uuid::parse_str(DIAGNOSTIC_ID).unwrap())),
                span_id: Some(SpanId::from_bytes([31; 16])),
                ..EventCorrelations::default()
            },
            payload: json!({
                "operation": "fixture.normal_failure",
                "resource": "state",
                "lifecycle_phase": null,
                "phase": "failed",
                "elapsed_micros": 50,
                "observation": serde_json::to_value(PhaseObservation::started_unknown()).unwrap(),
                "diagnostic_correlations": {},
                "database_identity": null,
            }),
            legacy_provenance: None,
            retry_of_generation_id: None,
        },
        &PayloadNormalizationPolicy::registered(&[
            "operation",
            "resource",
            "lifecycle_phase",
            "phase",
            "elapsed_micros",
            "observation",
            "diagnostic_correlations",
            "database_identity",
        ])
        .unwrap(),
        now,
    )
    .unwrap()
}

#[test]
fn normal_generation_and_shadow_jsonl_deduplicate_during_live_writer_read() {
    let fixture = Fixture::with_unavailable_stores();
    let event_root = fixture.data_dir.join("diagnostics/event-store-v1");
    let writer_id = WriterInstanceId::from_bytes([19; 16]);
    let event_id = EventId::from_bytes([29; 16]);
    let envelope = normal_diagnostic_envelope(writer_id, event_id);
    let writer = ProcessEventWriter::start(EventWriterConfig::native(
        &event_root,
        envelope.producer.clone(),
    ))
    .unwrap();
    writer.append(envelope.clone()).unwrap();
    fs::write(
        fixture.recorder_root.join("flight-shadow.jsonl"),
        format!("{}\n", serde_json::to_string(&envelope).unwrap()),
    )
    .unwrap();

    let output = fixture
        .command()
        .args(["diagnostics", "recent", "--limit", "10", "--json"])
        .output()
        .unwrap();
    assert_success(&output);
    let json = stdout_json(&output);
    let records = json["recent"]["events"].as_array().unwrap();
    assert_eq!(records.len(), 1, "{json:#}");
    assert_eq!(records[0]["event"]["event_id"], event_id.to_string());
    assert_eq!(records[0]["origins"].as_array().unwrap().len(), 2);
    assert_eq!(json["coalesced"]["failures"][0]["count"], 1);
    assert!(
        json["recent"]["coverage"]["coverage_complete"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(
        json["recent"]["coverage"]["normal"]["partitions_examined"],
        1
    );
    // The independent producer remains writable after the offline snapshot.
    let second = normal_diagnostic_envelope(writer_id, EventId::from_bytes([30; 16]));
    writer.append(second).unwrap();
    writer.shutdown().unwrap();
    fixture.assert_store_paths_remained_unavailable();
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn cli_continuation_finds_known_failure_beyond_jsonl_directory_prefix() {
    let fixture = Fixture::with_unavailable_stores();
    fixture.seed_failed_trace_with_truncated_tail();
    let original = fs::read_dir(&fixture.recorder_root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".jsonl")
        })
        .unwrap();
    let bytes = fs::read(&original).unwrap();
    fs::remove_file(&original).unwrap();
    for index in 0..1030 {
        fs::write(
            fixture
                .recorder_root
                .join(format!("flight-decoy-{index:04}.jsonl")),
            b"",
        )
        .unwrap();
    }
    let entries = fs::read_dir(&fixture.recorder_root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect::<Vec<_>>();
    assert!(entries.len() > 1024);
    let target = fixture.recorder_root.join(&entries[1024]);
    assert!(
        target
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("flight-decoy-")
    );
    fs::write(&target, bytes).unwrap();

    for command_name in ["recent", "trace"] {
        let mut cursor: Option<String> = None;
        let mut found = false;
        for page_number in 0..16 {
            let mut command = fixture.command();
            command.args(["diagnostics", command_name]);
            if command_name == "recent" {
                command.args(["--limit", "10"]);
            } else {
                command.arg(DIAGNOSTIC_ID);
            }
            command.arg("--json");
            if let Some(value) = &cursor {
                command.args(["--cursor", value]);
            }
            let output = command.output().unwrap();
            assert_success(&output);
            let json = stdout_json(&output);
            let section = if command_name == "recent" {
                "recent"
            } else {
                "trace"
            };
            let report = &json[section];
            assert!(
                report["coverage"]["directory_entries_examined"]
                    .as_u64()
                    .unwrap()
                    <= 1024
            );
            assert!(report["coverage"]["files_seen"].as_u64().unwrap() <= 256);
            if page_number == 0 {
                assert_eq!(report["coverage"]["coverage_complete"], false);
                assert_eq!(report["coverage"]["page_local_results"], true);
                assert!(
                    report["issues"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|issue| issue["kind"] == "incomplete_search_no_match")
                );
            }
            found |= report["events"].as_array().unwrap().iter().any(|record| {
                record["event"]["diagnostic_id"] == DIAGNOSTIC_ID
                    && record["event"]["phase"] == "failed"
            });
            cursor = report["coverage"]["next_cursor"]
                .as_str()
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        assert!(found, "{command_name} never reached known failure");
    }
    fixture.assert_store_paths_remained_unavailable();
}

#[test]
fn unequal_duplicate_event_identity_is_reported_as_incomplete() {
    let fixture = Fixture::with_unavailable_stores();
    fixture.seed_failed_trace_with_truncated_tail();
    let shard = fs::read_dir(&fixture.recorder_root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".jsonl")
        })
        .unwrap();
    let text = fs::read_to_string(&shard).unwrap();
    let lines = text.lines().take(2).collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    let mut changed: Value = serde_json::from_str(lines[1]).unwrap();
    changed["operation"] = Value::String("fixture.conflicting_operation".to_string());
    fs::write(&shard, format!("{}\n{}\n{}\n", lines[0], lines[1], changed)).unwrap();
    let output = fixture
        .command()
        .args(["diagnostics", "recent", "--limit", "10", "--json"])
        .output()
        .unwrap();
    assert_success(&output);
    let json = stdout_json(&output);
    assert_eq!(json["recent"]["events"].as_array().unwrap().len(), 2);
    assert_eq!(json["recent"]["coverage"]["coverage_complete"], false);
    assert!(
        json["recent"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["kind"] == "identity_conflict")
    );
}

#[test]
fn conflicting_normal_generations_keep_both_physical_origins() {
    let fixture = Fixture::with_unavailable_stores();
    let event_root = fixture.data_dir.join("diagnostics/event-store-v1");
    let id = EventId::from_bytes([44; 16]);
    let mut writers = Vec::new();
    for writer_byte in [41_u8, 42_u8] {
        let envelope =
            normal_diagnostic_envelope(WriterInstanceId::from_bytes([writer_byte; 16]), id);
        let writer = ProcessEventWriter::start(EventWriterConfig::native(
            &event_root,
            envelope.producer.clone(),
        ))
        .unwrap();
        writer.append(envelope).unwrap();
        writers.push(writer);
    }
    let output = fixture
        .command()
        .args(["diagnostics", "recent", "--limit", "10", "--json"])
        .output()
        .unwrap();
    assert_success(&output);
    let json = stdout_json(&output);
    let records = json["recent"]["events"].as_array().unwrap();
    assert_eq!(records.len(), 2, "{json:#}");
    assert!(
        records
            .iter()
            .all(|record| record["origins"].as_array().unwrap().len() == 1)
    );
    assert_ne!(
        records[0]["origins"][0]["writer_instance_id"],
        records[1]["origins"][0]["writer_instance_id"]
    );
    assert_eq!(json["recent"]["coverage"]["coverage_complete"], false);
    assert!(
        json["recent"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["kind"] == "identity_conflict")
    );
    for writer in writers {
        writer.shutdown().unwrap();
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "{output:?}");
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not JSON: {error}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}
