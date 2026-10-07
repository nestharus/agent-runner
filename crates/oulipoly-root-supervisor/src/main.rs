//! Launch entry for one per-root supervisor process. See the library docs.
//!
//! `--describe STORE --requester R --describer D` instead prints one
//! `session_control/v3` `root_entry` for the root stored at STORE, read
//! without claiming or locking it (exit 0), or a refusal (exit 65).
//! `--read-control` reads one bounded JSON caller encounter on stdin and
//! returns SDK selection, trace steps and retained rejection diagnostics.
//! It is read-only: no store, process authority, native launch or effect.

use std::io::{self, BufReader};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--read-control"] {
        use std::io::Read;
        let mut text = String::new();
        let result = io::stdin()
            .take(4 * 1024 * 1024 + 1)
            .read_to_string(&mut text);
        if result.is_err() || text.len() > 4 * 1024 * 1024 {
            return ExitCode::from(65);
        }
        return match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => {
                println!("{}", oulipoly_root_supervisor::control_reader::read(&value));
                ExitCode::SUCCESS
            }
            Err(_) => ExitCode::from(65),
        };
    }
    if let [
        flag,
        store,
        requester_flag,
        requester,
        describer_flag,
        describer,
    ] = args.as_slice()
        && flag == "--describe"
        && requester_flag == "--requester"
        && describer_flag == "--describer"
    {
        return match oulipoly_root_supervisor::describe_entry(
            Path::new(store),
            requester,
            describer,
        ) {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(reason) => {
                println!(
                    "{}",
                    serde_json::json!({ "event": "describe-refused", "reason": reason })
                );
                ExitCode::from(oulipoly_root_supervisor::EXIT_STORE_REFUSED)
            }
        };
    }
    if !args.is_empty() {
        println!(
            "{}",
            serde_json::json!({ "event": "terminal", "status": "spec-refused", "reason": "usage: (request on stdin) | --describe STORE --requester R --describer D" })
        );
        return ExitCode::from(oulipoly_root_supervisor::EXIT_SPEC_REFUSED);
    }
    let input = BufReader::new(io::stdin());
    let code = oulipoly_root_supervisor::run(input, io::stdout().lock());
    ExitCode::from(code)
}
