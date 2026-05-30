//! ## Declared roles
//!
//! Roles: orchestration, accessor, mapper.
//!
//! - orchestration: starts stdout/stderr drain threads, drains queued output
//!   events, and joins drain threads at process completion.
//! - accessor: reads drain chunks from child pipe streams.
//! - mapper: maps read byte counts into owned drain chunks.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/drain.rs
//!     role: adapter
//!     Translates:
//!       - std-io-pipe-drain-contract
//!       - std-process-child-lifecycle-contract
//! ```

use super::DrainStream;
use super::drain_access::{take_child_stderr, take_child_stdout};
use super::drain_chunks::append_output_chunk;
use std::io::Read;
use std::process::Child;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

pub(super) struct ChildDrains {
    pub(super) rx: mpsc::Receiver<(DrainStream, Vec<u8>)>,
    stdout_handle: thread::JoinHandle<()>,
    stderr_handle: thread::JoinHandle<()>,
}

pub(super) fn start_child_drains(child: &mut Child) -> Result<ChildDrains, String> {
    let stdout = take_child_stdout(child)?;
    let stderr = take_child_stderr(child)?;
    let (tx, rx) = mpsc::channel();
    let stdout_handle = spawn_drain_thread(stdout, DrainStream::Stdout, tx.clone());
    let stderr_handle = spawn_drain_thread(stderr, DrainStream::Stderr, tx.clone());
    drop(tx);
    Ok(ChildDrains {
        rx,
        stdout_handle,
        stderr_handle,
    })
}

pub(super) fn finish_child_drains(
    drains: ChildDrains,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    last_output_seen: &mut Instant,
) {
    let _ = drains.stdout_handle.join();
    let _ = drains.stderr_handle.join();
    drain_output_events(&drains.rx, stdout, stderr, last_output_seen);
}

fn spawn_drain_thread<R>(
    mut reader: R,
    stream: DrainStream,
    sender: mpsc::Sender<(DrainStream, Vec<u8>)>,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        while let Some(chunk) = read_drain_chunk(&mut reader, &mut buffer) {
            if send_drain_chunk(&sender, stream, chunk).is_err() {
                break;
            }
        }
    })
}

fn read_drain_chunk<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Option<Vec<u8>> {
    let count = read_drain_count(reader, buffer)?;
    Some(drain_chunk_from_count(buffer, count))
}

fn read_drain_count<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Option<usize> {
    match reader.read(buffer) {
        Ok(0) | Err(_) => None,
        Ok(n) => Some(n),
    }
}

fn drain_chunk_from_count(buffer: &[u8], count: usize) -> Vec<u8> {
    buffer[..count].to_vec()
}

fn send_drain_chunk(
    sender: &mpsc::Sender<(DrainStream, Vec<u8>)>,
    stream: DrainStream,
    chunk: Vec<u8>,
) -> Result<(), mpsc::SendError<(DrainStream, Vec<u8>)>> {
    sender.send((stream, chunk))
}

pub(super) fn drain_output_events(
    rx: &mpsc::Receiver<(DrainStream, Vec<u8>)>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    last_output_seen: &mut Instant,
) {
    while let Ok((stream, chunk)) = rx.try_recv() {
        append_output_chunk(stream, chunk, stdout, stderr, last_output_seen);
    }
}
