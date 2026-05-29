use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const CONTRACT: &str = "oulipoly.provider/v1";
const REQUEST_ID: &str = "request-example-001";

fn main() {
    let mode = env::var("FAKE_PROVIDER_MODE").unwrap_or_else(|_| "success".to_owned());
    let code = match mode.as_str() {
        "record-argv-stdin" => record_argv_stdin(),
        "s5-record-argv-stdin" => {
            record_argv_stdin_with_response_kind(ResponseKind::S5Success)
        }
        "stdin-eof" => stdin_eof(),
        "success" => success(),
        "s5-success" => s5_success(),
        "success-stderr" => success_stderr(),
        "provider-error" => provider_error("failed", "example_failed", 0),
        "provider-timeout-error" => provider_error("timeout", "example_timeout", 0),
        "provider-error-nonzero" => provider_error("failed", "example_failed", 7),
        "exit-nonzero-no-envelope" => 7,
        "success-then-nonzero" => {
            success();
            7
        }
        "schema-invalid-success" => write_stdout(&format!(
            "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"ok\":true,\"result\":{{}}}}\n"
        )),
        "invalid-utf8" => {
            let _ = read_stdin_to_string();
            write_bytes(&[0xff, 0xfe, 0xfd])
        }
        "non-object-array" => read_then_write_stdout("[]\n"),
        "non-object-string" => read_then_write_stdout("\"x\"\n"),
        "non-object-number" => read_then_write_stdout("5\n"),
        "missing-ok" => read_then_write_stdout(&format!(
            "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"result\":{{}}}}\n"
        )),
        "invalid-json" => read_then_write_stdout("not json\n"),
        "empty-stdout" => {
            let _ = read_stdin_to_string();
            0
        }
        "multiple-json" => write_stdout("{}\n{}\n"),
        "leading-log" => write_stdout("log line\n{}"),
        "trailing-junk" => write_stdout(&format!("{} junk\n", success_json())),
        "stderr-envelope-only" => write_stderr(&success_json()),
        "mismatched-contract" => {
            write_stdout(&success_json().replace(CONTRACT, "example.contract/v0"))
        }
        "mismatched-request-id" => {
            write_stdout(&success_json().replace(REQUEST_ID, "request-example-other"))
        }
        "large-stdout-stderr" => large_stdout_stderr(),
        "pipe-pressure" => pipe_pressure(),
        "sleep" => sleep_forever(),
        "child-grandchild" => child_grandchild(),
        "sigterm-resistant-child-grandchild" => sigterm_resistant_child_grandchild(),
        "probe-child" => probe_child(),
        "probe-grandchild" => probe_grandchild(),
        "early-stdin-success" => early_stdin_success(),
        "early-stdin-error" => early_stdin_error(),
        "early-stdin-empty" => early_stdin_empty(),
        "launch-valid" => launch_valid(0),
        "launch-model-nonzero" => launch_valid(9),
        "launch-provider-nonzero-after-final" => {
            launch_valid(0);
            6
        }
        "launch-provider-nonzero-no-final" => {
            write_jsonl(&stdout_event(1, "YQ=="));
            8
        }
        "launch-cancelled-final-event" => {
            write_jsonl(&stdout_event(1, "YQ=="));
            thread::sleep(Duration::from_millis(150));
            write_jsonl(&cancelled_exit_event(2));
            0
        }
        "launch-malformed-line" => write_stdout("{not-json}\n"),
        "launch-malformed-line-nonzero" => {
            write_stdout("{not-json}\n");
            8
        }
        "launch-malformed-line-stderr" => {
            eprintln!("fake-provider launch diagnostic on stderr");
            write_stdout("{not-json}\n")
        }
        "launch-blank-line" => {
            write_jsonl(&stdout_event(1, "YQ=="));
            println!("   ");
            write_jsonl(&exit_event(2, 0));
            0
        }
        "launch-exit-then-large-stdout" => {
            write_jsonl(&exit_event(1, 0));
            print!("{}", "x".repeat(1024 * 1024));
            let _ = io::stdout().flush();
            0
        }
        "launch-invalid-base64" => {
            write_jsonl(&stdout_event(1, "@@@"));
            write_jsonl(&exit_event(2, 0));
            0
        }
        "launch-duplicate-exit" => {
            write_jsonl(&exit_event(1, 0));
            write_jsonl(&exit_event(2, 0));
            0
        }
        "launch-event-after-exit" => {
            write_jsonl(&exit_event(1, 0));
            write_jsonl(&stdout_event(2, "Yg=="));
            0
        }
        "launch-partial-hang" => {
            write_jsonl(&stdout_event(1, "YQ=="));
            let _ = io::stdout().flush();
            sleep_forever()
        }
        other => {
            eprintln!("unknown fake-provider mode: {other}");
            64
        }
    };
    std::process::exit(code);
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
    record_current_invocation();
    write_response_for_kind(kind)
}

fn record_current_invocation() {
    let record = current_invocation_record();
    let text = format_invocation_record(&record);
    write_invocation_record(&record.path, &text);
}

fn current_invocation_record() -> InvocationRecord {
    InvocationRecord {
        path: env::var("FAKE_PROVIDER_RECORD_PATH").expect("record path should be set"),
        argv: env::args().collect(),
        stdin: read_stdin_to_string(),
    }
}

fn format_invocation_record(record: &InvocationRecord) -> String {
    format!("argv:\n{}\nstdin:\n{}", record.argv.join("\n"), record.stdin)
}

fn write_invocation_record(path: &str, text: &str) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        fs::create_dir_all(parent).expect("record directory should be writable");
    }
    fs::write(path, text).expect("record file should be writable");
}

fn write_response_for_kind(kind: ResponseKind) -> i32 {
    if launch_subcommand_requested() {
        write_launch_success_events()
    } else {
        write_stdout(&response_json_for_kind(kind))
    }
}

fn launch_subcommand_requested() -> bool {
    env::args().any(|arg| arg == "launch")
}

fn write_launch_success_events() -> i32 {
    write_jsonl(&stdout_event(1, "YQ=="));
    write_jsonl(&exit_event(2, 0));
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
        retryable: category == "timeout",
        message: format!("{category} from fake-provider"),
    }
}

fn provider_error_json(fields: &ProviderErrorFields<'_>) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"ok\":false,\"error\":{{\"category\":\"{}\",\"code\":\"{}\",\"retryable\":{},\"message\":\"{}\",\"details\":{{\"source\":\"example\"}}}}}}\n",
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

fn launch_exit_event_json(seq: u64, code: i32, signal: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"exit\",\"status\":{{\"kind\":\"exited\",\"code\":{code}}},\"terminal_signal\":{{\"kind\":\"{signal}\",\"evidence\":\"fake-provider exit event\",\"observed_at_unix_ms\":{}}},\"session\":{{\"provider_session_id\":\"example-session\"}}}}",
        1000 + seq,
        1000 + seq
    )
}

fn stdin_eof() -> i32 {
    let stdin = read_stdin_to_string();
    if stdin.is_empty() {
        eprintln!("stdin was empty before eof");
        65
    } else {
        eprintln!("observed stdin eof");
        success()
    }
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
    eprintln!("fake-provider diagnostic on stderr");
    success()
}

fn provider_error(category: &str, code: &str, exit_code: i32) -> i32 {
    let _ = read_stdin_to_string();
    let fields = provider_error_fields(category, code);
    write_stdout(&provider_error_json(&fields));
    exit_code
}

fn success_json() -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"ok\":true,\"result\":{{\"provider_id\":\"fake-provider\",\"display_name\":\"Fake Provider\",\"contract_versions\":[\"{CONTRACT}\"],\"preferred_contract\":\"{CONTRACT}\",\"capabilities\":{{\"launch\":true,\"policy\":false,\"quota\":false,\"session\":false,\"terminal\":false,\"rotation\":false,\"discovery\":false,\"settings\":false,\"setup_brain\":false,\"setup\":false,\"migration\":false}},\"concurrency\":{{\"safe_for_parallel_invocation\":true,\"state_locking\":\"none\"}}}}}}\n"
    )
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
    let stdout_thread = thread::spawn(|| {
        for _ in 0..128 {
            print!("{}", "o".repeat(8192));
            let _ = io::stdout().flush();
        }
    });
    let stderr_thread = thread::spawn(|| {
        for _ in 0..128 {
            eprint!("{}", "e".repeat(8192));
            let _ = io::stderr().flush();
        }
    });
    let _ = read_stdin_to_string();
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    0
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

fn spawn_probe_child(ignore_sigterm: bool) -> i32 {
    let current = env::current_exe().expect("current executable should be known");
    let mut child_command = Command::new(current);
    child_command
        .env("FAKE_PROVIDER_MODE", "probe-child")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if ignore_sigterm {
        child_command.env("FAKE_PROVIDER_IGNORE_SIGTERM", "1");
    }
    if let Ok(probe_dir) = env::var("FAKE_PROVIDER_PROBE_DIR") {
        child_command.env("FAKE_PROVIDER_PROBE_DIR", probe_dir);
    }
    let mut child = child_command.spawn().expect("child should spawn");
    let _ = child.wait();
    sleep_forever()
}

fn probe_child() -> i32 {
    maybe_ignore_sigterm();
    write_probe_pid("child");
    let current = env::current_exe().expect("current executable should be known");
    let mut command = Command::new(current);
    command
        .env("FAKE_PROVIDER_MODE", "probe-grandchild")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if env::var_os("FAKE_PROVIDER_IGNORE_SIGTERM").is_some() {
        command.env("FAKE_PROVIDER_IGNORE_SIGTERM", "1");
    }
    if let Ok(probe_dir) = env::var("FAKE_PROVIDER_PROBE_DIR") {
        command.env("FAKE_PROVIDER_PROBE_DIR", probe_dir);
    }
    let _grandchild = command.spawn().expect("grandchild should spawn");
    sleep_forever()
}

fn probe_grandchild() -> i32 {
    maybe_ignore_sigterm();
    write_probe_pid("grandchild");
    sleep_forever()
}

fn maybe_ignore_sigterm() {
    if env::var_os("FAKE_PROVIDER_IGNORE_SIGTERM").is_some() {
        ignore_sigterm();
    }
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
    let Ok(root) = env::var("FAKE_PROVIDER_PROBE_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let _ = fs::create_dir_all(&root);
    let pid = std::process::id();
    let _ = fs::write(root.join(format!("{label}-{pid}.pid")), pid.to_string());
}

fn early_stdin_success() -> i32 {
    let mut buffer = [0_u8; 1];
    let _ = io::stdin().read(&mut buffer);
    write_stdout(&success_json())
}

fn early_stdin_error() -> i32 {
    let mut buffer = [0_u8; 1];
    let _ = io::stdin().read(&mut buffer);
    provider_error("failed", "example_early_stdin", 0)
}

fn early_stdin_empty() -> i32 {
    let mut buffer = [0_u8; 1];
    let _ = io::stdin().read(&mut buffer);
    0
}

fn launch_valid(exit_code: i32) -> i32 {
    let _ = read_stdin_to_string();
    write_jsonl(&stdout_event(1, "AAH/"));
    write_jsonl(&stderr_event(2, "ZXJy"));
    write_jsonl(&marker_event(3));
    write_jsonl(&heartbeat_event(4));
    write_jsonl(&exit_event(5, exit_code));
    0
}

fn write_jsonl(line: &str) {
    println!("{line}");
    let _ = io::stdout().flush();
}

fn stdout_event(seq: u64, data_base64: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"stdout\",\"data_base64\":\"{data_base64}\"}}",
        1000 + seq
    )
}

fn stderr_event(seq: u64, data_base64: &str) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"stderr\",\"data_base64\":\"{data_base64}\"}}",
        1000 + seq
    )
}

fn marker_event(seq: u64) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"marker\",\"name\":\"example-marker\",\"value\":{{\"phase\":\"example\"}}}}",
        1000 + seq
    )
}

fn heartbeat_event(seq: u64) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"heartbeat\",\"detail\":\"example heartbeat\"}}",
        1000 + seq
    )
}

fn exit_event(seq: u64, code: i32) -> String {
    launch_exit_event_json(seq, code, terminal_signal_for_exit_code(code))
}

fn cancelled_exit_event(seq: u64) -> String {
    format!(
        "{{\"contract\":\"{CONTRACT}\",\"request_id\":\"{REQUEST_ID}\",\"seq\":{seq},\"time_unix_ms\":{},\"kind\":\"exit\",\"status\":{{\"kind\":\"cancelled\"}},\"terminal_signal\":{{\"kind\":\"cancelled\",\"evidence\":\"fake-provider cancellation\",\"observed_at_unix_ms\":{}}},\"session\":{{\"provider_session_id\":\"example-session\"}}}}",
        1000 + seq,
        1000 + seq
    )
}
