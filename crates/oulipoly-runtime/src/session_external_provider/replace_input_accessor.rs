//! Role: accessor.

use crate::session_replace::{ReplaceError, ReplaceSource};
use std::fs;
use std::io::Read;

pub(crate) fn read_replace_source(source: &ReplaceSource) -> Result<Vec<u8>, ReplaceError> {
    match source {
        ReplaceSource::File(path) => {
            fs::read(path).map_err(|err| ReplaceError::InvalidInputTranscript {
                reason: format!("failed to read input file: {err}"),
                line: None,
            })
        }
        ReplaceSource::Stdin => read_stdin_bytes(),
    }
}

fn read_stdin_bytes() -> Result<Vec<u8>, ReplaceError> {
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).map_err(|err| {
        ReplaceError::InvalidInputTranscript {
            reason: format!("failed to read stdin: {err}"),
            line: None,
        }
    })?;
    Ok(bytes)
}
