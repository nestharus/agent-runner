//! Production Linux protocol client for a root's registered children.
//! The packaged caller supplies its absolute path when a registered parent
//! offers child routes; the shared provider bridge executes that offered path.
//!
//! `oulipoly-root-child ROUTE QUESTION` asks the owner named by
//! `OULIPOLY_ROOT_BASH_V1` for one child on ROUTE, keeps the connection
//! open while the child lives, prints every reply line to stderr (prefixed
//! `oulipoly-root-child: `) and the final `result` (or `refused`) line to
//! stdout. Exit: 0 for a `result`, 65 for `refused`, 69 when there is no
//! ingress or the connection failed before sending, 75 when the stream
//! ended without a result (the child may have run). Nothing is retried.
//! Intermediate stages (including close-not-applied) are not terminal.
//! A result's transport success is separate from its child outcome, answer,
//! tagged turn end and process/namespace end; inspect those reported fields.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use oulipoly_root_supervisor::bash::{BASH_ENV, PROTOCOL};
use serde_json::{Value, json};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [route, question] = args.as_slice() else {
        eprintln!("oulipoly-root-child: usage: ROUTE QUESTION");
        return ExitCode::from(64);
    };
    let Some(path) = std::env::var_os(BASH_ENV) else {
        eprintln!("oulipoly-root-child: {BASH_ENV} is not set");
        return ExitCode::from(69);
    };
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("oulipoly-root-child: owner unreachable: {error}");
            return ExitCode::from(69);
        }
    };
    let request = json!({ "v": PROTOCOL, "op": "child", "route": route, "prompt": question });
    if writeln!(stream, "{request}").is_err() {
        eprintln!("oulipoly-root-child: request not written");
        return ExitCode::from(75);
    }
    for line in BufReader::new(&stream).lines() {
        let Ok(line) = line else { break };
        eprintln!("oulipoly-root-child: {line}");
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match value["event"].as_str() {
            Some("result") => {
                println!("{line}");
                return ExitCode::SUCCESS;
            }
            Some("refused") => {
                println!("{line}");
                return ExitCode::from(65);
            }
            _ => {}
        }
    }
    println!(
        "{}",
        json!({ "event": "lost", "meaning": "no result received; the child may have run" })
    );
    ExitCode::from(75)
}
