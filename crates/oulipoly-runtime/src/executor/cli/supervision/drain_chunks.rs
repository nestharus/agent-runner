//! ## Declared roles
//!
//! Roles: filter, predicate, mapper.
//!
//! - filter: appends non-empty output chunks to raw stdout/stderr byte
//!   buffers while preserving stream order within received events.
//! - predicate: answers whether an output chunk is empty.
//! - mapper: maps streams to output buffers and updates output timestamps.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/supervision/drain_chunks.rs
//!     role: adapter
//!     Translates:
//!       - std-io-pipe-drain-contract
//!       - raw-output-byte-contract
//! ```

use super::DrainStream;
use std::time::Instant;

pub(super) fn append_output_chunk(
    stream: DrainStream,
    chunk: Vec<u8>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    last_output_seen: &mut Instant,
) {
    if output_chunk_is_empty(&chunk) {
        return;
    }
    append_non_empty_output_chunk(stream, &chunk, stdout, stderr);
    record_output_seen(last_output_seen);
}

fn output_chunk_is_empty(chunk: &[u8]) -> bool {
    chunk.is_empty()
}

fn append_non_empty_output_chunk(
    stream: DrainStream,
    chunk: &[u8],
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) {
    match stream {
        DrainStream::Stdout => stdout.extend_from_slice(chunk),
        DrainStream::Stderr => stderr.extend_from_slice(chunk),
    }
}

fn record_output_seen(last_output_seen: &mut Instant) {
    *last_output_seen = Instant::now();
}
