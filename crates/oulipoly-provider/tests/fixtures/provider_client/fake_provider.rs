//! ## Declared roles
//!
//! Roles: orchestration, accessor, mapper, formatter, parser, predicate, validator.
//!
//! - orchestration: `main`, `dispatch_fake_provider_mode`, launch fixture
//!   modes, S7C/S5 fixture flows, sleep/hang modes, and probe child/grandchild
//!   modes drive fake provider subprocess behavior for tests.
//! - accessor: `fake_provider_mode`, `current_subcommand`, `s7c_env_or`,
//!   `read_request_id`, `request_id_from_stdin`, and invocation/probe helpers
//!   read environment, stdin, argv, and sidecar state.
//! - mapper: mode dispatch, `response_json_for_kind`,
//!   `s5_result_json_for_subcommand`, `provider_error_fields`,
//!   `terminal_signal_for_exit_code`, and S7C value builders map fixture inputs
//!   onto provider protocol payloads.
//! - formatter: JSON envelope/event builders, invocation-record formatters,
//!   S7C host-state/artifact formatters, and `json_escape` materialize stable
//!   fake provider stdout/stderr/file payloads.
//! - parser: `json_string_field`, `request_id_from_stdin`, and count helpers
//!   parse minimal JSON/stdin and sidecar count state used by fixtures.
//! - predicate: provider retryability, launch/describe/subcommand selectors,
//!   stdin/probe env checks, and SIGTERM-ignore checks choose fixture branches.
//! - validator: unknown-mode handling rejects unsupported fixture modes with a
//!   stable diagnostic and exit code.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs
//!     role: adapter
//!     Translates:
//!       - fake-provider-fixture-contract
//!       - provider-cli-subprocess-contract
//!       - launch-jsonl-stream-contract
//!       - oulipoly-provider-generated-dto-contract
//!       - process-supervision-liveness-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs
//!     role: intrinsic-surface
//!     Domain: fake provider executable fixture modes and protocol payloads
//!     Owns:
//!       - fake-provider mode vocabulary and environment dispatch
//!       - describe, settings, rotation, migration, and launch fixture payloads
//!       - invocation record, count, artifact, and probe sidecar file behavior
//!       - process-tree, hang, pipe-pressure, and signal-resistance scenarios
//!       - request-id correlation fallback and JSON escaping helpers
//! ```

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const CONTRACT: &str = "oulipoly.provider/v1";
const REQUEST_ID: &str = "request-example-001";

fn main() {
    exit_fake_provider(dispatch_fake_provider_mode(&fake_provider_mode()));
}

fn fake_provider_mode() -> String {
    env::var("FAKE_PROVIDER_MODE").unwrap_or_else(|_| "success".to_owned())
}

fn exit_fake_provider(code: i32) {
    std::process::exit(code);
}

fn dispatch_fake_provider_mode(mode: &str) -> i32 {
    match mode {
        "record-argv-stdin" => record_argv_stdin(),
        "s5-record-argv-stdin" => record_argv_stdin_with_response_kind(ResponseKind::S5Success),
        "stdin-eof" => stdin_eof(),
        "success" => success(),
        "s5-success" => s5_success(),
        "success-stderr" => success_stderr(),
        "provider-error" => provider_error_after_describe("failed", "example_failed", 0),
        "provider-timeout-error" => provider_error("timeout", "example_timeout", 0),
        "provider-error-nonzero" => provider_error("failed", "example_failed", 7),
        "exit-nonzero-no-envelope" => exit_nonzero_after_describe(),
        "describe-failed" => provider_error("failed", "describe_failed", 0),
        "describe-rotation-disabled" => describe_with_capabilities(false, true),
        "describe-migration-disabled" => describe_with_capabilities(true, false),
        "s7c-rotation-assess-success" => s7c_rotation_assess_success(),
        "s7c-rotation-assess-denied" => s7c_rotation_assess_denied(),
        "s7c-rotation-materialize-success" => s7c_rotation_materialize_success(true),
        "s7c-rotation-materialize-missing-source" => {
            s7c_rotation_materialize_provider_error("source_missing")
        }
        "s7c-rotation-materialize-dry-run" => s7c_rotation_materialize_no_change(),
        "s7c-rotation-materialize-no-change" => s7c_rotation_materialize_no_change(),
        "s7c-rotation-materialize-no-change-wrong-chain" => {
            s7c_rotation_materialize_no_change_wrong_chain()
        }
        "s7c-rotation-materialize-compaction-boundary" => s7c_rotation_materialize_success(true),
        "s7c-rotation-materialize-artifact-hash-mismatch" => {
            s7c_rotation_materialize_hash_mismatch()
        }
        "s7c-rotation-materialize-protocol-invalid" => s7c_protocol_invalid_after_describe(),
        "s7c-rotation-materialize-crash-after-artifact" => {
            s7c_rotation_materialize_crash_after_artifact()
        }
        "s7c-rotation-materialize-crash-during-apply" => materialize_crash_during_apply(),
        "s7c-migration-plan-success" => s7c_migration_plan_success(),
        "s7c-migration-plan-protocol-invalid" => s7c_protocol_invalid_after_describe(),
        "s7c-migration-apply-success" => s7c_migration_apply_success(),
        "s7c-migration-apply-protocol-invalid" => s7c_protocol_invalid_after_describe(),
        "success-then-nonzero" => success_then_nonzero(),
        "schema-invalid-success" => schema_invalid_success_after_describe(),
        "invalid-utf8" => invalid_utf8(),
        "non-object-array" => read_then_write_stdout("[]\n"),
        "non-object-string" => read_then_write_stdout("\"x\"\n"),
        "non-object-number" => read_then_write_stdout("5\n"),
        "missing-ok" => read_then_write_stdout(&missing_ok_json()),
        "invalid-json" => read_then_write_stdout("not json\n"),
        "empty-stdout" => empty_stdout(),
        "multiple-json" => write_stdout("{}\n{}\n"),
        "leading-log" => write_stdout("log line\n{}"),
        "trailing-junk" => write_stdout(&trailing_junk_json()),
        "stderr-envelope-only" => write_stderr(&success_json()),
        "mismatched-contract" => write_stdout(&mismatched_contract_json()),
        "mismatched-request-id" => write_stdout(&mismatched_request_id_json()),
        "large-stdout-stderr" => large_stdout_stderr(),
        "pipe-pressure" => pipe_pressure(),
        "sleep" => sleep_forever(),
        "child-grandchild" => child_grandchild(),
        "sigterm-resistant-child-grandchild" => sigterm_resistant_child_grandchild(),
        "exit-with-pipe-holding-descendant" => exit_with_pipe_holding_descendant(),
        "pipe-holding-descendant" => pipe_holding_descendant(),
        "probe-child" => probe_child(),
        "probe-grandchild" => probe_grandchild(),
        "early-stdin-success" => early_stdin_success(),
        "early-stdin-error" => early_stdin_error(),
        "early-stdin-empty" => early_stdin_empty(),
        "launch-valid" => launch_valid(0),
        "launch-provider-error" => provider_error("conflict", "launch_conflict", 2),
        "launch-model-nonzero" => launch_valid(9),
        "launch-provider-nonzero-after-final" => launch_provider_nonzero_after_final(),
        "launch-provider-nonzero-no-final" => launch_provider_nonzero_no_final(),
        "launch-cancelled-final-event" => launch_cancelled_final_event(),
        "launch-long-valid-stream" => launch_long_valid_stream(),
        "launch-malformed-line" => write_stdout("{not-json}\n"),
        "launch-malformed-line-nonzero" => launch_malformed_line_nonzero(),
        "launch-malformed-line-stderr" => launch_malformed_line_stderr(),
        "launch-blank-line" => launch_blank_line(),
        "launch-exit-then-large-stdout" => launch_exit_then_large_stdout(),
        "launch-invalid-base64" => launch_invalid_base64(),
        "launch-duplicate-exit" => launch_duplicate_exit(),
        "launch-event-after-exit" => launch_event_after_exit(),
        "launch-partial-hang" => launch_partial_hang(),
        "launch-heartbeats-then-exit" => launch_heartbeats_then_exit(),
        "launch-heartbeat-then-child-grandchild-hang" => {
            launch_heartbeat_then_child_grandchild_hang()
        }
        other => unknown_fake_provider_mode(other),
    }
}

fn unknown_fake_provider_mode(mode: &str) -> i32 {
    eprintln!("{}", unknown_fake_provider_mode_message(mode));
    64
}

fn unknown_fake_provider_mode_message(mode: &str) -> String {
    format!("unknown fake-provider mode: {mode}")
}

fn read_stdin_to_string() -> String {
    let mut stdin = String::new();
    let _ = io::stdin().read_to_string(&mut stdin);
    stdin
}

fn write_stdout(text: &str) -> i32 {
    print!("{text}");
    let _ = io::stdout().flush();
    0
}

fn read_then_write_stdout(text: &str) -> i32 {
    let _ = read_stdin_to_string();
    write_stdout(text)
}

fn write_bytes(bytes: &[u8]) -> i32 {
    let _ = io::stdout().write_all(bytes);
    let _ = io::stdout().flush();
    0
}

fn write_stderr(text: &str) -> i32 {
    let _ = read_stdin_to_string();
    eprint!("{text}");
    let _ = io::stderr().flush();
    0
}

fn materialize_crash_during_apply() -> i32 {
    let _ = s7c_rotation_materialize_success(true);
    write_stderr("crash_during_apply\n")
}

fn success_then_nonzero() -> i32 {
    success();
    7
}

fn invalid_utf8() -> i32 {
    let _ = read_stdin_to_string();
    write_bytes(&[0xff, 0xfe, 0xfd])
}

fn empty_stdout() -> i32 {
    let _ = read_stdin_to_string();
    0
}

fn missing_ok_json() -> String {
    format!("{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"result\":{{}}}}\n")
}

fn trailing_junk_json() -> String {
    format!("{} junk\n", success_json())
}

fn mismatched_contract_json() -> String {
    success_json().replace(CONTRACT, "example.contract/v0")
}

fn mismatched_request_id_json() -> String {
    success_json().replace(REQUEST_ID, "request-example-other")
}

fn record_argv_stdin() -> i32 {
    record_argv_stdin_with_response_kind(ResponseKind::DescribeSuccess)
}

enum ResponseKind {
    DescribeSuccess,
    S5Success,
}

struct InvocationRecord {
    path: String,
    argv: Vec<String>,
    stdin: String,
}

struct ProviderErrorFields<'a> {
    category: &'a str,
    code: &'a str,
    retryable: bool,
    message: String,
}

fn record_argv_stdin_with_response_kind(kind: ResponseKind) -> i32 {
    let record = record_current_invocation();
    write_response_for_kind(kind, Some(&record.stdin))
}

fn record_current_invocation() -> InvocationRecord {
    let record = current_invocation_record();
    let text = format_invocation_record(&record);
    write_invocation_record(&record.path, &text);
    record
}

fn current_invocation_record() -> InvocationRecord {
    invocation_record_from_parts(record_path_env(), current_argv(), read_stdin_to_string())
}

fn record_path_env() -> String {
    env::var("FAKE_PROVIDER_RECORD_PATH").expect("record path should be set")
}

fn current_argv() -> Vec<String> {
    env::args().collect()
}

fn invocation_record_from_parts(path: String, argv: Vec<String>, stdin: String) -> InvocationRecord {
    InvocationRecord { path, argv, stdin }
}

fn format_invocation_record(record: &InvocationRecord) -> String {
    format!(
        "argv:\n{}\nstdin:\n{}",
        record.argv.join("\n"),
        record.stdin
    )
}

fn write_invocation_record(path: &str, text: &str) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        fs::create_dir_all(parent).expect("record directory should be writable");
    }
    fs::write(path, text).expect("record file should be writable");
}

fn record_invocation_if_requested(stdin: &str) {
    increment_count_if_requested();
    if let Some(record) = invocation_record_if_requested(stdin) {
        write_invocation_record(&record.path, &format_invocation_record(&record));
    }
}

fn invocation_record_if_requested(stdin: &str) -> Option<InvocationRecord> {
    let path = env::var("FAKE_PROVIDER_RECORD_PATH").ok()?;
    Some(invocation_record(path, stdin))
}

fn invocation_record(path: String, stdin: &str) -> InvocationRecord {
    invocation_record_from_parts(path, current_argv(), stdin.to_string())
}

fn increment_count_if_requested() {
    let Some(path) = count_path_if_requested() else {
        return;
    };
    write_count(&path, incremented_count(read_count(&path)));
}

fn count_path_if_requested() -> Option<String> {
    env::var("FAKE_PROVIDER_COUNT_PATH").ok()
}

fn read_count(path: &str) -> u64 {
    parse_count_text(read_count_text(path)).unwrap_or(0)
}

fn read_count_text(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn parse_count_text(text: Option<String>) -> Option<u64> {
    text.and_then(|text| parse_count(&text))
}

fn parse_count(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

fn incremented_count(current: u64) -> u64 {
    current + 1
}

fn write_count(path: &str, count: u64) {
    fs::write(path, count.to_string()).expect("count file should be writable");
}

fn write_response_for_kind(kind: ResponseKind, stdin: Option<&str>) -> i32 {
    if launch_subcommand_requested() {
        write_launch_success_events(stdin.unwrap_or_default())
    } else {
        write_stdout(&response_json_for_kind(kind))
    }
}

fn launch_subcommand_requested() -> bool {
    env::args().any(|arg| arg == "launch")
}

fn write_launch_success_events(stdin: &str) -> i32 {
    let request_id = request_id_from_stdin(stdin);
    write_jsonl(&stdout_event(&request_id, 1, "YQ=="));
    write_jsonl(&exit_event(&request_id, 2, 0));
    0
}

fn response_json_for_kind(kind: ResponseKind) -> String {
    match kind {
        ResponseKind::DescribeSuccess => success_json(),
        ResponseKind::S5Success => s5_success_json(),
    }
}

fn provider_error_fields<'a>(category: &'a str, code: &'a str) -> ProviderErrorFields<'a> {
    ProviderErrorFields {
        category,
        code,
        retryable: provider_error_retryable(category),
        message: provider_error_message(category),
    }
}

fn provider_error_retryable(category: &str) -> bool {
    category == "timeout"
}

fn provider_error_message(category: &str) -> String {
    format!("{category} from fake-provider")
}

fn provider_error_json(request_id: &str, fields: &ProviderErrorFields<'_>) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":false,\"error\":{{\"category\":\"{}\",\"code\":\"{}\",\"retryable\":{},\"message\":\"{}\",\"details\":{{\"source\":\"example\"}}}}}}\n",
        fields.category, fields.code, fields.retryable, fields.message
    )
}

fn current_subcommand() -> String {
    env::args().nth(1).unwrap_or_default()
}

fn s5_result_json_for_subcommand(subcommand: &str) -> &'static str {
    match subcommand {
        "schema" => {
            r#"{"schema_id":"example.settings/v1","schema":{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["profile_id"],"properties":{"profile_id":{"type":"string","title":"Profile"}},"additionalProperties":false},"ui":{"sections":[{"id":"account","title":"Account","fields":["profile_id"]}]}}"#
        }
        "settings.list" => {
            r#"{"records":[{"id":"example-settings","display_name":"Example Settings","version":"7","summary":{"status":"ready"}}]}"#
        }
        _ => {
            r#"{"provider_id":"fake-provider","display_name":"Fake Provider","contract_versions":["oulipoly.provider/v1"],"preferred_contract":"oulipoly.provider/v1","capabilities":{"launch":true,"policy":false,"quota":false,"session":false,"terminal":false,"rotation":false,"discovery":false,"settings":true,"setup_brain":false,"setup":false,"migration":false},"settings_schema_id":"example.settings/v1","concurrency":{"safe_for_parallel_invocation":true,"state_locking":"none"}}"#
        }
    }
}

fn s5_success_envelope(result: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"ok\":true,\"result\":{result}}}\n"
    )
}

fn terminal_signal_for_exit_code(code: i32) -> &'static str {
    if code == 0 {
        "clean_exit"
    } else {
        "nonzero_exit"
    }
}

fn launch_exit_event_json(request_id: &str, seq: u64, code: i32, signal: &str) -> String {
    let request_id = json_escape(request_id);
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"exit\",\"status\":{{\"kind\":\"exited\",\"code\":{code}}},\"terminal_signal\":{{\"kind\":\"{signal}\",\"evidence\":\"fake-provider exit event\",\"observed_at_unix_ms\":{}}},\"session\":{{\"provider_session_id\":\"example-session\"}}}}",
        1000 + seq,
        1000 + seq
    )
}

fn stdin_eof() -> i32 {
    let stdin = read_stdin_to_string();
    write_stdin_eof_result(&stdin)
}

fn write_stdin_eof_result(stdin: &str) -> i32 {
    if stdin_empty(stdin) {
        return write_empty_stdin_eof();
    }
    write_observed_stdin_eof()
}

fn stdin_empty(stdin: &str) -> bool {
    stdin.is_empty()
}

fn write_empty_stdin_eof() -> i32 {
    eprintln!("stdin was empty before eof");
    65
}

fn write_observed_stdin_eof() -> i32 {
    write_observed_stdin_eof_diagnostic();
    success()
}

fn write_observed_stdin_eof_diagnostic() {
    eprintln!("observed stdin eof");
}

fn success() -> i32 {
    let _ = read_stdin_to_string();
    write_stdout(&success_json())
}

fn s5_success() -> i32 {
    let _ = read_stdin_to_string();
    write_stdout(&s5_success_json())
}

fn success_stderr() -> i32 {
    write_success_stderr_diagnostic();
    success()
}

fn write_success_stderr_diagnostic() {
    eprintln!("fake-provider diagnostic on stderr");
}

fn provider_error(category: &str, code: &str, exit_code: i32) -> i32 {
    let stdin = read_stdin_to_string();
    record_invocation_if_requested(&stdin);
    let fields = provider_error_fields(category, code);
    write_stdout(&provider_error_json(
        &request_id_from_stdin(&stdin),
        &fields,
    ));
    exit_code
}

fn provider_error_after_describe(category: &str, code: &str, exit_code: i32) -> i32 {
    if s7c_describe_requested() {
        return describe_with_capabilities(true, true);
    }
    provider_error(category, code, exit_code)
}

fn exit_nonzero_after_describe() -> i32 {
    if s7c_describe_requested() {
        return describe_with_capabilities(true, true);
    }
    let stdin = read_stdin_to_string();
    record_invocation_if_requested(&stdin);
    7
}

fn s7c_describe_requested() -> bool {
    current_subcommand() == "describe" && s7c_runtime_fixture_requested()
}

fn schema_invalid_success_after_describe() -> i32 {
    s7c_runtime_describe_or_else(|| write_recorded_stdout(schema_invalid_success_json))
}

fn s7c_runtime_describe_or_else(run: impl FnOnce() -> i32) -> i32 {
    if s7c_describe_requested() {
        return describe_with_capabilities(true, true);
    }
    run()
}

fn schema_invalid_success_json(request_id: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{}}}}\n"
    )
}

fn success_json() -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"ok\":true,\"result\":{{\"provider_id\":\"fake-provider\",\"display_name\":\"Fake Provider\",\"contract_versions\":[\"{CONTRACT}\"],\"preferred_contract\":\"{CONTRACT}\",\"capabilities\":{{\"launch\":true,\"policy\":false,\"quota\":false,\"session\":false,\"terminal\":false,\"rotation\":false,\"discovery\":false,\"settings\":false,\"setup_brain\":false,\"setup\":false,\"migration\":false}},\"concurrency\":{{\"safe_for_parallel_invocation\":true,\"state_locking\":\"none\"}}}}}}\n"
    )
}

fn describe_with_capabilities(rotation: bool, migration: bool) -> i32 {
    let stdin = read_stdin_to_string();
    record_invocation_if_requested(&stdin);
    write_stdout(&describe_json(
        &request_id_from_stdin(&stdin),
        rotation,
        migration,
    ))
}

fn describe_or_recorded_stdout(format_response: fn(&str) -> String) -> i32 {
    describe_or_else(|| write_recorded_stdout(format_response))
}

fn describe_or_materialize_stdout(format_response: impl FnOnce(&str) -> String) -> i32 {
    describe_or_else(|| write_recorded_materialize_stdout(format_response))
}

fn describe_or_else(run: impl FnOnce() -> i32) -> i32 {
    if s7c_describe_subcommand_requested() {
        return describe_with_capabilities(true, true);
    }
    run()
}

fn s7c_describe_subcommand_requested() -> bool {
    current_subcommand() == "describe"
}

fn write_recorded_stdout(format_response: fn(&str) -> String) -> i32 {
    let stdin = read_stdin_to_string();
    record_invocation_if_requested(&stdin);
    write_stdout(&format_response(&request_id_from_stdin(&stdin)))
}

fn write_recorded_materialize_stdout(format_response: impl FnOnce(&str) -> String) -> i32 {
    let stdin = read_stdin_to_string();
    record_invocation_if_requested(&stdin);
    let request_id = request_id_from_stdin(&stdin);
    write_empty_materialize_artifact();
    write_stdout(&format_response(&request_id))
}

fn s7c_rotation_assess_success_json(request_id: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{\"allowed\":true,\"score\":80,\"reason\":\"target-provider accepted\",\"requirements\":[]}}}}\n"
    )
}

fn s7c_rotation_assess_denied_json(request_id: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{\"allowed\":false,\"score\":0,\"reason\":\"target-provider denied\",\"requirements\":[{{\"kind\":\"quota\"}}]}}}}\n"
    )
}

fn s7c_rotation_materialize_success_json(request_id: &str, changed: bool) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{\"changed\":{changed},\"target_provider_session_id\":\"{}\",\"artifacts\":[{}],\"host_state_plan\":{}}}}}\n",
        s7c_target_session_id(),
        s7c_artifact_json(),
        s7c_host_state_plan_json()
    )
}

fn s7c_rotation_materialize_no_change_json(request_id: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{\"changed\":false,\"artifacts\":[{}],\"host_state_plan\":{}}}}}\n",
        s7c_artifact_json(),
        s7c_host_state_plan_json()
    )
}

fn s7c_rotation_materialize_wrong_chain_json(request_id: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{\"changed\":false,\"artifacts\":[{}],\"host_state_plan\":{}}}}}\n",
        s7c_artifact_json(),
        s7c_host_state_plan_json_with_chain("wrong-chain")
    )
}

fn s7c_rotation_materialize_hash_mismatch_json(request_id: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{\"changed\":true,\"target_provider_session_id\":\"{}\",\"artifacts\":[{{\"kind\":\"file\",\"path\":\"{}\",\"sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\"}}],\"host_state_plan\":{}}}}}\n",
        s7c_target_session_id(),
        json_escape(&s7c_artifact_path()),
        s7c_host_state_plan_json()
    )
}

fn s7c_migration_plan_success_json(request_id: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{\"actions\":[{{\"kind\":\"noop\"}}],\"warnings\":[],\"requires_backup\":false}}}}\n"
    )
}

fn s7c_migration_apply_success_json(request_id: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{\"applied_actions\":[{{\"kind\":\"noop\"}}],\"artifacts\":[],\"warnings\":[],\"outcome\":{{\"changed\":false}}}}}}\n"
    )
}

fn describe_json(request_id: &str, rotation: bool, migration: bool) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"ok\":true,\"result\":{{\"provider_id\":\"fake-provider\",\"display_name\":\"Fake Provider\",\"contract_versions\":[\"{CONTRACT}\"],\"preferred_contract\":\"{CONTRACT}\",\"capabilities\":{{\"launch\":true,\"policy\":false,\"quota\":false,\"session\":false,\"terminal\":false,\"rotation\":{rotation},\"discovery\":false,\"settings\":false,\"setup_brain\":false,\"setup\":false,\"migration\":{migration}}},\"settings_schema_id\":\"fake-settings\",\"concurrency\":{{\"safe_for_parallel_invocation\":true,\"state_locking\":\"host\"}}}}}}\n"
    )
}

fn s7c_rotation_assess_success() -> i32 {
    describe_or_recorded_stdout(s7c_rotation_assess_success_json)
}

fn s7c_rotation_assess_denied() -> i32 {
    describe_or_recorded_stdout(s7c_rotation_assess_denied_json)
}

fn s7c_rotation_materialize_success(changed: bool) -> i32 {
    describe_or_materialize_stdout(|request_id| {
        s7c_rotation_materialize_success_json(request_id, changed)
    })
}

fn s7c_rotation_materialize_no_change() -> i32 {
    describe_or_recorded_stdout(s7c_rotation_materialize_no_change_json)
}

fn s7c_rotation_materialize_no_change_wrong_chain() -> i32 {
    describe_or_recorded_stdout(s7c_rotation_materialize_wrong_chain_json)
}

fn s7c_rotation_materialize_hash_mismatch() -> i32 {
    describe_or_materialize_stdout(s7c_rotation_materialize_hash_mismatch_json)
}

fn s7c_rotation_materialize_crash_after_artifact() -> i32 {
    describe_or_else(s7c_rotation_materialize_hash_mismatch)
}

fn s7c_rotation_materialize_provider_error(code: &str) -> i32 {
    describe_or_else(|| provider_error("failed", code, 0))
}

fn s7c_migration_plan_success() -> i32 {
    describe_or_recorded_stdout(s7c_migration_plan_success_json)
}

fn s7c_migration_apply_success() -> i32 {
    describe_or_recorded_stdout(s7c_migration_apply_success_json)
}

fn s7c_protocol_invalid_after_describe() -> i32 {
    describe_or_else(write_recorded_invalid_protocol)
}

fn write_recorded_invalid_protocol() -> i32 {
    let stdin = read_stdin_to_string();
    record_invocation_if_requested(&stdin);
    write_stdout("not json\n")
}

fn s7c_artifact_json() -> String {
    format!(
        "{{\"kind\":\"file\",\"path\":\"{}\",\"sha256\":\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"}}",
        json_escape(&s7c_artifact_path())
    )
}

fn s7c_host_state_plan_json() -> String {
    let chain_id = s7c_env_or("S7C_CHAIN_ID", "chain-alpha");
    s7c_host_state_plan_json_with_chain(&chain_id)
}

fn s7c_host_state_plan_json_with_chain(chain_id: &str) -> String {
    let values = s7c_host_state_values(chain_id);
    format_s7c_host_state_plan(&values)
}

struct S7cHostStateValues {
    chain_id: String,
    source_provider: String,
    target_provider: String,
    source_session: String,
    target_session: String,
}

fn s7c_host_state_values(chain_id: &str) -> S7cHostStateValues {
    S7cHostStateValues {
        chain_id: chain_id.to_string(),
        source_provider: s7c_env_or("S7C_SOURCE_PROVIDER", "source-provider"),
        target_provider: s7c_env_or("S7C_TARGET_PROVIDER", "target-provider"),
        source_session: s7c_env_or("S7C_SOURCE_SESSION_ID", "session-source"),
        target_session: s7c_target_session_id(),
    }
}

fn format_s7c_host_state_plan(values: &S7cHostStateValues) -> String {
    format!(
        "{{\"schema_version\":1,\"operation\":\"rotation.materialize\",\"chain_id\":\"{}\",\"source_provider\":\"{}\",\"target_provider\":\"{}\",\"source_session_id\":\"{}\",\"target_session_id\":\"{}\",\"transition_reason\":\"quota_threshold\",\"segments\":[{{\"provider\":\"{}\",\"session_id\":\"{}\",\"ended_at\":\"2026-05-01T00:00:00Z\"}},{{\"provider\":\"{}\",\"session_id\":\"{}\",\"started_at\":\"2026-05-01T00:00:00Z\"}}],\"artifacts\":[{}]}}",
        json_escape(&values.chain_id),
        json_escape(&values.source_provider),
        json_escape(&values.target_provider),
        json_escape(&values.source_session),
        json_escape(&values.target_session),
        json_escape(&values.source_provider),
        json_escape(&values.source_session),
        json_escape(&values.target_provider),
        json_escape(&values.target_session),
        s7c_artifact_json()
    )
}

fn write_empty_materialize_artifact() {
    write_empty_file(&materialize_artifact_path());
}

fn materialize_artifact_path() -> std::path::PathBuf {
    std::path::PathBuf::from(s7c_artifact_path())
}

fn write_empty_file(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, []);
}

fn s7c_artifact_path() -> String {
    s7c_env_or("S7C_ARTIFACT_PATH", "/tmp/oulipoly/session-target.jsonl")
}

fn s7c_target_session_id() -> String {
    s7c_env_or("S7C_TARGET_SESSION_ID", "session-target")
}

fn s7c_env_or(key: &str, fallback: &str) -> String {
    env::var(key).unwrap_or_else(|_| fallback.to_string())
}

fn s7c_runtime_fixture_requested() -> bool {
    env::var_os("S7C_CHAIN_ID").is_some()
}

fn s5_success_json() -> String {
    let subcommand = current_subcommand();
    let result = s5_result_json_for_subcommand(&subcommand);
    s5_success_envelope(result)
}

fn large_stdout_stderr() -> i32 {
    let _ = read_stdin_to_string();
    let block = "x".repeat(128 * 1024);
    print!("{block}");
    eprint!("{block}");
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
    0
}

fn pipe_pressure() -> i32 {
    let stdout_thread = thread::spawn(write_stdout_pressure_blocks);
    let stderr_thread = thread::spawn(write_stderr_pressure_blocks);
    let _ = read_stdin_to_string();
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    0
}

fn write_stdout_pressure_blocks() {
    for _ in 0..128 {
        print!("{}", stdout_pressure_block());
        let _ = io::stdout().flush();
    }
}

fn write_stderr_pressure_blocks() {
    for _ in 0..128 {
        eprint!("{}", stderr_pressure_block());
        let _ = io::stderr().flush();
    }
}

fn stdout_pressure_block() -> String {
    "o".repeat(8192)
}

fn stderr_pressure_block() -> String {
    "e".repeat(8192)
}

fn sleep_forever() -> i32 {
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn child_grandchild() -> i32 {
    spawn_probe_child(false)
}

fn sigterm_resistant_child_grandchild() -> i32 {
    ignore_sigterm();
    spawn_probe_child(true)
}

fn exit_with_pipe_holding_descendant() -> i32 {
    let _ = read_stdin_to_string();
    let mut command = pipe_holding_descendant_command();
    let _descendant = command.spawn().expect("pipe-holding descendant should spawn");
    write_stdout(&success_json())
}

fn pipe_holding_descendant() -> i32 {
    write_probe_pid("pipe-holder");
    thread::sleep(Duration::from_secs(2));
    0
}

fn pipe_holding_descendant_command() -> Command {
    let mut command = Command::new(current_executable_path());
    command
        .env("FAKE_PROVIDER_MODE", "pipe-holding-descendant")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    apply_probe_dir_env(&mut command, probe_dir_env());
    command
}

fn spawn_probe_child(ignore_sigterm: bool) -> i32 {
    let mut child_command = probe_child_command(ignore_sigterm);
    let mut child = child_command.spawn().expect("child should spawn");
    let _ = child.wait();
    sleep_forever()
}

fn probe_child() -> i32 {
    maybe_ignore_sigterm();
    write_probe_pid("child");
    let mut command = probe_grandchild_command();
    let _grandchild = command.spawn().expect("grandchild should spawn");
    sleep_forever()
}

fn probe_grandchild() -> i32 {
    maybe_ignore_sigterm();
    write_probe_pid("grandchild");
    sleep_forever()
}

fn maybe_ignore_sigterm() {
    if ignore_sigterm_requested() {
        ignore_sigterm();
    }
}

fn probe_child_command(ignore_sigterm: bool) -> Command {
    let mut command = probe_process_command("probe-child");
    apply_ignore_sigterm_env(&mut command, ignore_sigterm);
    apply_probe_dir_env(&mut command, probe_dir_env());
    command
}

fn probe_grandchild_command() -> Command {
    let mut command = probe_process_command("probe-grandchild");
    apply_ignore_sigterm_env(&mut command, ignore_sigterm_requested());
    apply_probe_dir_env(&mut command, probe_dir_env());
    command
}

fn probe_process_command(mode: &str) -> Command {
    let mut command = Command::new(current_executable_path());
    command
        .env("FAKE_PROVIDER_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn current_executable_path() -> std::path::PathBuf {
    env::current_exe().expect("current executable should be known")
}

fn apply_ignore_sigterm_env(command: &mut Command, enabled: bool) {
    if enabled {
        command.env("FAKE_PROVIDER_IGNORE_SIGTERM", "1");
    }
}

fn ignore_sigterm_requested() -> bool {
    env::var_os("FAKE_PROVIDER_IGNORE_SIGTERM").is_some()
}

fn apply_probe_dir_env(command: &mut Command, probe_dir: Option<String>) {
    if let Some(probe_dir) = probe_dir {
        command.env("FAKE_PROVIDER_PROBE_DIR", probe_dir);
    }
}

fn probe_dir_env() -> Option<String> {
    env::var("FAKE_PROVIDER_PROBE_DIR").ok()
}

#[cfg(unix)]
fn ignore_sigterm() {
    const SIGTERM: i32 = 15;
    const SIG_IGN: usize = 1;
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    unsafe {
        let _ = signal(SIGTERM, SIG_IGN);
    }
}

#[cfg(not(unix))]
fn ignore_sigterm() {}

fn write_probe_pid(label: &str) {
    let Some(root) = probe_root() else {
        return;
    };
    write_pid_file(&root, label, std::process::id());
}

fn probe_root() -> Option<std::path::PathBuf> {
    env::var("FAKE_PROVIDER_PROBE_DIR")
        .ok()
        .map(std::path::PathBuf::from)
}

fn write_pid_file(root: &std::path::Path, label: &str, pid: u32) {
    let _ = fs::create_dir_all(root);
    let _ = fs::write(probe_pid_path(root, label, pid), pid.to_string());
}

fn probe_pid_path(root: &std::path::Path, label: &str, pid: u32) -> std::path::PathBuf {
    root.join(format!("{label}-{pid}.pid"))
}

fn early_stdin_success() -> i32 {
    read_one_stdin_byte();
    write_stdout(&success_json())
}

fn early_stdin_error() -> i32 {
    read_one_stdin_byte();
    provider_error("failed", "example_early_stdin", 0)
}

fn early_stdin_empty() -> i32 {
    read_one_stdin_byte();
    0
}

fn read_one_stdin_byte() {
    let mut buffer = [0_u8; 1];
    let _ = io::stdin().read(&mut buffer);
}

fn launch_valid(exit_code: i32) -> i32 {
    let request_id = read_request_id();
    write_jsonl(&stdout_event(&request_id, 1, "AAH/"));
    write_jsonl(&stderr_event(&request_id, 2, "ZXJy"));
    write_jsonl(&marker_event(&request_id, 3));
    write_jsonl(&heartbeat_event(&request_id, 4));
    write_jsonl(&exit_event(&request_id, 5, exit_code));
    0
}

fn launch_provider_nonzero_after_final() -> i32 {
    launch_valid(0);
    6
}

fn launch_provider_nonzero_no_final() -> i32 {
    let request_id = read_request_id();
    write_jsonl(&stdout_event(&request_id, 1, "YQ=="));
    8
}

fn launch_cancelled_final_event() -> i32 {
    let request_id = read_request_id();
    write_jsonl(&stdout_event(&request_id, 1, "YQ=="));
    thread::sleep(Duration::from_millis(150));
    write_jsonl(&cancelled_exit_event(&request_id, 2));
    0
}

fn launch_long_valid_stream() -> i32 {
    let request_id = read_request_id();
    write_long_valid_launch_stream(&request_id);
    0
}

fn write_long_valid_launch_stream(request_id: &str) {
    write_launch_event_lines(&long_valid_launch_stream_events(request_id));
}

fn long_valid_launch_stream_events(request_id: &str) -> Vec<String> {
    let detail = long_launch_heartbeat_detail();
    let mut events = long_launch_heartbeat_events(request_id, &detail);
    events.push(exit_event(request_id, 701, 0));
    events
}

fn long_launch_heartbeat_events(request_id: &str, detail: &str) -> Vec<String> {
    (1..=700)
        .map(|seq| heartbeat_event_with_detail(request_id, seq, detail))
        .collect()
}

fn long_launch_heartbeat_detail() -> String {
    "h".repeat(4096)
}

fn write_launch_event_lines(events: &[String]) {
    let mut stdout = io::stdout().lock();
    for event in events {
        let _ = writeln!(stdout, "{event}");
    }
    let _ = stdout.flush();
}

fn launch_malformed_line_nonzero() -> i32 {
    write_stdout("{not-json}\n");
    8
}

fn launch_malformed_line_stderr() -> i32 {
    write_launch_malformed_line_diagnostic();
    write_stdout("{not-json}\n")
}

fn write_launch_malformed_line_diagnostic() {
    eprintln!("fake-provider launch diagnostic on stderr");
}

fn launch_blank_line() -> i32 {
    let request_id = read_request_id();
    write_launch_blank_line_events(&request_id);
    0
}

fn write_launch_blank_line_events(request_id: &str) {
    write_jsonl(&stdout_event(request_id, 1, "YQ=="));
    write_blank_launch_line();
    write_jsonl(&exit_event(request_id, 2, 0));
}

fn write_blank_launch_line() {
    println!("   ");
}

fn launch_exit_then_large_stdout() -> i32 {
    let request_id = read_request_id();
    write_exit_then_large_stdout(&request_id);
    0
}

fn write_exit_then_large_stdout(request_id: &str) {
    write_jsonl(&exit_event(&request_id, 1, 0));
    write_large_launch_stdout();
}

fn write_large_launch_stdout() {
    print!("{}", large_launch_stdout_block());
    let _ = io::stdout().flush();
}

fn large_launch_stdout_block() -> String {
    "x".repeat(1024 * 1024)
}

fn launch_invalid_base64() -> i32 {
    let request_id = read_request_id();
    write_jsonl(&stdout_event(&request_id, 1, "@@@"));
    write_jsonl(&exit_event(&request_id, 2, 0));
    0
}

fn launch_duplicate_exit() -> i32 {
    let request_id = read_request_id();
    write_jsonl(&exit_event(&request_id, 1, 0));
    write_jsonl(&exit_event(&request_id, 2, 0));
    0
}

fn launch_event_after_exit() -> i32 {
    let request_id = read_request_id();
    write_jsonl(&exit_event(&request_id, 1, 0));
    write_jsonl(&stdout_event(&request_id, 2, "Yg=="));
    0
}

fn launch_partial_hang() -> i32 {
    let request_id = read_request_id();
    write_jsonl(&stdout_event(&request_id, 1, "YQ=="));
    let _ = io::stdout().flush();
    sleep_forever()
}

fn launch_heartbeats_then_exit() -> i32 {
    let request_id = read_request_id();
    write_jsonl(&stdout_event(&request_id, 1, "YQ=="));
    for seq in 2..=5 {
        thread::sleep(Duration::from_millis(50));
        write_jsonl(&heartbeat_event(&request_id, seq));
    }
    thread::sleep(Duration::from_millis(50));
    write_jsonl(&exit_event(&request_id, 6, 0));
    0
}

fn launch_heartbeat_then_child_grandchild_hang() -> i32 {
    let request_id = read_request_id();
    write_jsonl(&stdout_event(&request_id, 1, "YQ=="));
    thread::sleep(Duration::from_millis(50));
    write_jsonl(&heartbeat_event(&request_id, 2));
    thread::sleep(Duration::from_millis(50));
    child_grandchild()
}

fn write_jsonl(line: &str) {
    println!("{line}");
    let _ = io::stdout().flush();
}

fn read_request_id() -> String {
    request_id_from_stdin(&read_stdin_to_string())
}

fn request_id_from_stdin(stdin: &str) -> String {
    json_string_field(stdin, "request_id").unwrap_or_else(|| REQUEST_ID.to_owned())
}

fn json_string_field(input: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let field_start = input.find(&needle)? + needle.len();
    let after_colon = input[field_start..].split_once(':')?.1.trim_start();
    let value = after_colon.strip_prefix('"')?;
    let end = value.find('"')?;
    Some(value[..end].to_owned())
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn stdout_event(request_id: &str, seq: u64, data_base64: &str) -> String {
    let request_id = json_escape(request_id);
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"stdout\",\"data_base64\":\"{data_base64}\"}}",
        1000 + seq
    )
}

fn stderr_event(request_id: &str, seq: u64, data_base64: &str) -> String {
    let request_id = json_escape(request_id);
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"stderr\",\"data_base64\":\"{data_base64}\"}}",
        1000 + seq
    )
}

fn marker_event(request_id: &str, seq: u64) -> String {
    let request_id = json_escape(request_id);
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"marker\",\"name\":\"example-marker\",\"value\":{{\"phase\":\"example\"}}}}",
        1000 + seq
    )
}

fn heartbeat_event(request_id: &str, seq: u64) -> String {
    heartbeat_event_with_detail(request_id, seq, "example heartbeat")
}

fn heartbeat_event_with_detail(request_id: &str, seq: u64, detail: &str) -> String {
    let request_id = json_escape(request_id);
    let detail = json_escape(detail);
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"heartbeat\",\"detail\":\"{detail}\"}}",
        1000 + seq
    )
}

fn exit_event(request_id: &str, seq: u64, code: i32) -> String {
    launch_exit_event_json(request_id, seq, code, terminal_signal_for_exit_code(code))
}

fn cancelled_exit_event(request_id: &str, seq: u64) -> String {
    let request_id = json_escape(request_id);
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{request_id}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"exit\",\"status\":{{\"kind\":\"cancelled\"}},\"terminal_signal\":{{\"kind\":\"cancelled\",\"evidence\":\"fake-provider cancellation\",\"observed_at_unix_ms\":{}}},\"session\":{{\"provider_session_id\":\"example-session\"}}}}",
        1000 + seq,
        1000 + seq
    )
}
