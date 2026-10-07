//! Launch entry for one per-root supervisor process. See the library docs.
//!
//! `--describe STORE --requester R --describer D` instead prints one
//! `session_control/v2` `root_entry` for the root stored at STORE, read
//! without claiming or locking it (exit 0), or a refusal (exit 65).

use std::io::{self, BufReader};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
