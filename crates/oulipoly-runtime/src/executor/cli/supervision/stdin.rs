//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! - orchestration: starts, writes, and joins the child stdin writer thread.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/stdin.rs
//!     role: adapter
//!     Translates:
//!       - std-io-pipe-drain-contract
//!       - prompt-stdin-contract
//! ```

use super::SupervisorConfig;
use super::errors::{stdin_writer_panic_error, write_stdin_error};
use super::stdin_access::{take_child_stdin, take_supervised_stdin_payload};
use super::stdin_predicates::supervised_stdin_write_needed;
use std::io::Write;
use std::process::Child;
use std::thread;

use crate::executor::cli::spawn_identity::child_custody_test_fault;

const STDIN_WRITE_CHUNK_SIZE: usize = 16 * 1024;

pub(super) struct StdinWriter {
    handle: thread::JoinHandle<Result<(), String>>,
}

pub(super) fn start_child_stdin_writer(
    child: &mut Child,
    config: &mut SupervisorConfig,
) -> Result<Option<StdinWriter>, String> {
    if !supervised_stdin_write_needed(config) {
        return Ok(None);
    }
    let Some(payload) = take_supervised_stdin_payload(config) else {
        return Ok(None);
    };
    child_custody_test_fault("headless_stdin")?;
    let mut stdin = take_child_stdin(child)?;
    let handle = thread::spawn(move || {
        write_prompt_payload(&mut stdin, &payload).map_err(|err| write_stdin_error(&err))
    });
    Ok(Some(StdinWriter { handle }))
}

fn write_prompt_payload<W: Write>(stdin: &mut W, payload: &[u8]) -> std::io::Result<()> {
    for chunk in payload.chunks(STDIN_WRITE_CHUNK_SIZE) {
        stdin.write_all(chunk)?;
    }
    stdin.flush()
}

pub(super) fn finish_stdin_writer(stdin_writer: Option<StdinWriter>) -> Option<String> {
    let writer = stdin_writer?;
    match writer.handle.join() {
        Ok(Ok(())) => None,
        Ok(Err(err)) => Some(err),
        Err(_) => Some(stdin_writer_panic_error()),
    }
}
