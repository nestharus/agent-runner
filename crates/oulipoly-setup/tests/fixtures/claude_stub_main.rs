//! ## Declared roles
//!
//! Roles: `parser`, `validator`, `mapper`, `formatter`, `orchestration`.
//!
//! Justifications:
//! - parser: `parse_arg_tokens`, `parse_next_value`, `parse_mode_value`,
//!   `parse_usize_value`, `parse_i32_value` parse raw argv / values into
//!   structured tokens.
//! - validator: `validate_arg_values`, `validate_optional_flag_value`,
//!   `validate_mode_arg`, `validate_usize_arg`, `validate_i32_arg`
//!   validate parsed shapes.
//! - mapper: `validate_arg_values`, `build_defaulted_args`, `build_args`
//!   map between argument shapes.
//! - formatter: `format_required_value_message`,
//!   `format_unsupported_mode_message`, `format_usize_message`,
//!   `format_i32_message`, `format_usage_message`, `emit_usage`,
//!   `report_write_failure`, and the `run_*` writers format and emit
//!   protocol bytes / messages.
//! - orchestration: `main`, `parse_args` sequence named helpers; the
//!   error-handling closures at call sites sequence emit + exit through
//!   named operations.
//!
//! Both `emit_usage` and `report_write_failure` are pure formatters: each
//! writes one formatted message to stderr and does not call
//! `process::exit`. Process termination is sequenced at the call site,
//! keeping each helper single-classification per
//! `~/ai/conventions/code-quality.md` § Function categories per function.
//!
//! ## Adapter declarations
//!
//! Component: `crates/oulipoly-setup/tests/fixtures/claude_stub_main.rs`
//! Role: adapter
//! Translates:
//! - argv mode flag -> deterministic stdout/stderr write protocol.
//! - argv flags -> exit code (0 for normal/chatty/parse-error, configurable for fail, sleep-then-no-exit for hang).
//! - argv stdout filler size + content -> stdout bytes written.
//! - argv stderr filler size + Session: token -> stderr bytes written; the `Session: <session_id>` token shape is the producer side of the canonical schema declared in `conventions/claude-cli-output-format.md` § Schema § Session-token vocabulary (form 1).

use std::env;
use std::io::{self, Write};
use std::num::ParseIntError;
use std::process;
use std::thread;
use std::vec::IntoIter;

const DEFAULT_CHATTY_FILLER_BYTES: usize = 70 * 1024;
const NORMAL_SESSION_ID: &str = "test-session-normal";
const CHATTY_SESSION_ID: &str = "test-session-chatty";
const FAIL_MESSAGE: &str = "age125 fail-mode stderr fragment";
const INVALID_JSON: &str = "not-json-age125";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Normal,
    Chatty,
    Hang,
    Fail,
    ParseError,
}

#[derive(Debug)]
struct Args {
    mode: Mode,
    stdout_filler_bytes: usize,
    stderr_filler_bytes: usize,
    exit_code: i32,
    stderr_message: String,
}

#[derive(Debug, Default)]
struct RawArgs {
    mode: ArgValueToken,
    stdout_filler_bytes: ArgValueToken,
    stderr_filler_bytes: ArgValueToken,
    exit_code: ArgValueToken,
    stderr_message: ArgValueToken,
}

#[derive(Debug, Default)]
enum ArgValueToken {
    #[default]
    Absent,
    MissingValue,
    Value(String),
}

#[derive(Debug)]
struct ProvidedArgs {
    mode: Option<String>,
    stdout_filler_bytes: Option<String>,
    stderr_filler_bytes: Option<String>,
    exit_code: Option<String>,
    stderr_message: Option<String>,
}

#[derive(Debug)]
struct DefaultedArgs {
    mode: String,
    stdout_filler_bytes: String,
    stderr_filler_bytes: String,
    exit_code: String,
    stderr_message: String,
}

fn main() {
    let args = parse_args(env::args().skip(1).collect());

    let result = match args.mode {
        Mode::Normal => run_normal(&args),
        Mode::Chatty => run_chatty(&args),
        Mode::Hang => run_hang(&args),
        Mode::Fail => run_fail(&args),
        Mode::ParseError => run_parse_error(&args),
    };

    if let Err(err) = result {
        report_write_failure(err);
        process::exit(111);
    }

    if args.mode == Mode::Fail {
        process::exit(args.exit_code);
    }
}

fn report_write_failure(err: io::Error) {
    let _ = writeln!(io::stderr(), "claude_stub write failed: {err}");
}

fn parse_args(raw_args: Vec<String>) -> Args {
    let raw = parse_arg_tokens(raw_args);
    let provided = validate_arg_values(raw);
    let defaulted = build_defaulted_args(provided);
    let mode = validate_mode_arg(defaulted.mode);
    let stdout_filler_bytes =
        validate_usize_arg("--stdout-filler-bytes", defaulted.stdout_filler_bytes);
    let stderr_filler_bytes =
        validate_usize_arg("--stderr-filler-bytes", defaulted.stderr_filler_bytes);
    let exit_code = validate_i32_arg("--exit-code", defaulted.exit_code);

    build_args(
        mode,
        stdout_filler_bytes,
        stderr_filler_bytes,
        exit_code,
        defaulted.stderr_message,
    )
}

fn parse_arg_tokens(raw_args: Vec<String>) -> RawArgs {
    let mut args = RawArgs::default();
    let mut iter = raw_args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--mode" => {
                args.mode = parse_next_value(&mut iter);
            }
            "--stdout-filler-bytes" => {
                args.stdout_filler_bytes = parse_next_value(&mut iter);
            }
            "--stderr-filler-bytes" => {
                args.stderr_filler_bytes = parse_next_value(&mut iter);
            }
            "--exit-code" => {
                args.exit_code = parse_next_value(&mut iter);
            }
            "--stderr-message" => {
                args.stderr_message = parse_next_value(&mut iter);
            }
            _ => {}
        }
    }

    args
}

fn parse_next_value(iter: &mut IntoIter<String>) -> ArgValueToken {
    iter.next()
        .map(ArgValueToken::Value)
        .unwrap_or(ArgValueToken::MissingValue)
}

fn validate_arg_values(raw: RawArgs) -> ProvidedArgs {
    ProvidedArgs {
        mode: validate_optional_flag_value("--mode", raw.mode),
        stdout_filler_bytes: validate_optional_flag_value(
            "--stdout-filler-bytes",
            raw.stdout_filler_bytes,
        ),
        stderr_filler_bytes: validate_optional_flag_value(
            "--stderr-filler-bytes",
            raw.stderr_filler_bytes,
        ),
        exit_code: validate_optional_flag_value("--exit-code", raw.exit_code),
        stderr_message: validate_optional_flag_value("--stderr-message", raw.stderr_message),
    }
}

fn validate_optional_flag_value(flag: &str, token: ArgValueToken) -> Option<String> {
    match token {
        ArgValueToken::Absent => None,
        ArgValueToken::MissingValue => {
            emit_usage(&format_required_value_message(flag));
            process::exit(2)
        }
        ArgValueToken::Value(value) => Some(value),
    }
}

fn build_defaulted_args(provided: ProvidedArgs) -> DefaultedArgs {
    DefaultedArgs {
        mode: provided.mode.unwrap_or_else(|| "normal".to_string()),
        stdout_filler_bytes: provided
            .stdout_filler_bytes
            .unwrap_or_else(|| DEFAULT_CHATTY_FILLER_BYTES.to_string()),
        stderr_filler_bytes: provided
            .stderr_filler_bytes
            .unwrap_or_else(|| DEFAULT_CHATTY_FILLER_BYTES.to_string()),
        exit_code: provided.exit_code.unwrap_or_else(|| "1".to_string()),
        stderr_message: provided
            .stderr_message
            .unwrap_or_else(|| FAIL_MESSAGE.to_string()),
    }
}

fn build_args(
    mode: Mode,
    stdout_filler_bytes: usize,
    stderr_filler_bytes: usize,
    exit_code: i32,
    stderr_message: String,
) -> Args {
    Args {
        mode,
        stdout_filler_bytes,
        stderr_filler_bytes,
        exit_code,
        stderr_message,
    }
}

fn validate_mode_arg(value: String) -> Mode {
    parse_mode_value(&value).unwrap_or_else(|| {
        emit_usage(&format_unsupported_mode_message(&value));
        process::exit(2)
    })
}

fn parse_mode_value(value: &str) -> Option<Mode> {
    match value {
        "normal" | "happy" => Some(Mode::Normal),
        "chatty" => Some(Mode::Chatty),
        "hang" | "sleep" => Some(Mode::Hang),
        "fail" | "exit-nonzero" => Some(Mode::Fail),
        "parse-error" | "invalid-json" => Some(Mode::ParseError),
        _ => None,
    }
}

fn validate_usize_arg(flag: &str, value: String) -> usize {
    parse_usize_value(&value).unwrap_or_else(|_| {
        emit_usage(&format_usize_message(flag));
        process::exit(2)
    })
}

fn parse_usize_value(value: &str) -> Result<usize, ParseIntError> {
    value.parse::<usize>()
}

fn validate_i32_arg(flag: &str, value: String) -> i32 {
    parse_i32_value(&value).unwrap_or_else(|_| {
        emit_usage(&format_i32_message(flag));
        process::exit(2)
    })
}

fn parse_i32_value(value: &str) -> Result<i32, ParseIntError> {
    value.parse::<i32>()
}

fn format_required_value_message(flag: &str) -> String {
    format!("{flag} requires a value")
}

fn format_unsupported_mode_message(value: &str) -> String {
    format!("unsupported --mode value: {value}")
}

fn format_usize_message(flag: &str) -> String {
    format!("{flag} must be a usize")
}

fn format_i32_message(flag: &str) -> String {
    format!("{flag} must be an i32")
}

fn format_usage_message(message: &str) -> String {
    format!("{message}\n")
}

fn emit_usage(message: &str) {
    let _ = write!(io::stderr(), "{}", format_usage_message(message));
}

fn run_normal(_args: &Args) -> io::Result<()> {
    write_result("normal")?;
    writeln!(io::stderr(), "Session: {NORMAL_SESSION_ID}")?;
    Ok(())
}

fn run_chatty(args: &Args) -> io::Result<()> {
    write_stdout_filler(args.stdout_filler_bytes)?;
    write_result("chatty")?;

    write_stderr_filler(args.stderr_filler_bytes / 2)?;
    writeln!(io::stderr())?;
    writeln!(io::stderr(), "Session: {CHATTY_SESSION_ID}")?;
    write_stderr_filler(args.stderr_filler_bytes - (args.stderr_filler_bytes / 2))?;
    Ok(())
}

fn run_hang(_args: &Args) -> io::Result<()> {
    loop {
        thread::park();
    }
}

fn run_fail(args: &Args) -> io::Result<()> {
    writeln!(io::stderr(), "{}", args.stderr_message)
}

fn run_parse_error(_args: &Args) -> io::Result<()> {
    write!(io::stdout(), "{INVALID_JSON}")?;
    Ok(())
}

fn write_result(label: &str) -> io::Result<()> {
    write!(
        io::stdout(),
        r#"{{"actions":[{{"type":"status","message":"age125 {label} status"}}],"done":true}}"#
    )
}

fn write_stdout_filler(bytes: usize) -> io::Result<()> {
    write_repeated(io::stdout(), b' ', bytes)
}

fn write_stderr_filler(bytes: usize) -> io::Result<()> {
    write_repeated(io::stderr(), b'x', bytes)
}

fn write_repeated<W: Write>(mut writer: W, byte: u8, bytes: usize) -> io::Result<()> {
    const CHUNK_SIZE: usize = 8192;
    let chunk = [byte; CHUNK_SIZE];
    let mut remaining = bytes;

    while remaining > 0 {
        let amount = remaining.min(CHUNK_SIZE);
        writer.write_all(&chunk[..amount])?;
        remaining -= amount;
    }

    writer.flush()
}
