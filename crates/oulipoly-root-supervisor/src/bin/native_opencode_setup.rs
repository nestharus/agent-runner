//! Provisions one native OpenCode host's launch directory for a root.
//!
//! Stdin: one JSON [`OpenCodeSetup`] line. Stdout: one JSON line, the
//! harness launch (`argv`, `endpoint`, the `env` its argv sets,
//! `removed_env`, `config_dir`), to use as a harness of the root's intent.
//! Exit 0 means provisioning completed and stdout was written and flushed;
//! it does not acknowledge that the caller consumed the receipt.
//! Exit 64 with `{"refused": reason, "effects": "none"}` means input-invalid
//! refusal before setup writes. Exit 1 with `{"failed": reason, "effects":
//! "possible", "retry": "do-not-replay"}` means construction failed and may
//! leave partial effects. Exit 74 means provisioning completed but receipt
//! delivery failed: the launch directory remains; do not replay setup.
//! The tree stays the caller's: this entry hands nothing to a work
//! identity (the Runner's `native-root` entry does, for `host-root`).

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use oulipoly_root_supervisor::native::{OpenCodeSetup, OpenCodeSetupError, provision_opencode};
use serde_json::{Value, json};

fn publish(out: &mut impl Write, receipt: &Value) -> io::Result<()> {
    writeln!(out, "{receipt}")?;
    out.flush()
}

fn main() -> ExitCode {
    let mut line = String::new();
    let result = io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("stdin: {error}"))
        .and_then(|_| {
            serde_json::from_str::<OpenCodeSetup>(&line).map_err(|error| format!("setup: {error}"))
        })
        .map_err(OpenCodeSetupError::InputInvalid)
        .and_then(|setup| provision_opencode(&setup, None));
    let mut out = io::stdout().lock();
    match result {
        Ok(launch) => match publish(&mut out, &launch.to_json()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(
                    io::stderr().lock(),
                    "provisioning completed; receipt delivery failed: {error}; \
                         launch directory retained; do not replay setup"
                );
                ExitCode::from(74)
            }
        },
        Err(error) => {
            let (receipt, code, meaning) = match error {
                OpenCodeSetupError::InputInvalid(reason) => (
                    json!({ "refused": reason, "effects": "none" }),
                    64,
                    "input-invalid refusal; no setup writes",
                ),
                OpenCodeSetupError::ConstructionFailed(reason) => (
                    json!({ "failed": reason, "effects": "possible", "retry": "do-not-replay" }),
                    1,
                    "construction failed; possible setup effects; do not replay setup",
                ),
            };
            if let Err(error) = publish(&mut out, &receipt) {
                let _ = writeln!(
                    io::stderr().lock(),
                    "{meaning}; receipt delivery failed: {error}"
                );
            }
            ExitCode::from(code)
        }
    }
}
