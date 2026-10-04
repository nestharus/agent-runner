//! Provisions one native OpenCode host's launch directory for a root.
//!
//! Stdin: one JSON [`OpenCodeSetup`] line. Stdout: one JSON line, the
//! harness launch (`argv`, `endpoint`, the `env` its argv sets,
//! `removed_env`, `config_dir`), to use as a harness of the root's intent.
//! Exit 0, or 64 with `{"refused": reason}` (an invalid setup writes
//! nothing; a failure while writing leaves what was written).

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use oulipoly_root_supervisor::native::{OpenCodeSetup, provision_opencode};
use serde_json::json;

fn main() -> ExitCode {
    let mut line = String::new();
    let result = io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("stdin: {error}"))
        .and_then(|_| {
            serde_json::from_str::<OpenCodeSetup>(&line).map_err(|error| format!("setup: {error}"))
        })
        .and_then(|setup| provision_opencode(&setup));
    let mut out = io::stdout().lock();
    match result {
        Ok(launch) => {
            let _ = writeln!(out, "{}", launch.to_json());
            ExitCode::SUCCESS
        }
        Err(reason) => {
            let _ = writeln!(out, "{}", json!({ "refused": reason }));
            ExitCode::from(64)
        }
    }
}
