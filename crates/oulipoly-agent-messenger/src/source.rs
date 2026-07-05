//! ## Declared roles
//!
//! - mapper
//! - orchestration
//!
//! Return-source materialization behind the scratchpad repository seam.

use crate::MessengerError;
use crate::mapper::{inline_return_payload, scratchpad_read_request, scratchpad_return_payload};
use crate::model::{ReturnPayload, ReturnRequest, ReturnSource};
use crate::repository::ScratchpadRepository;
use oulipoly_agent_scratchpad::ScratchpadName;

pub(crate) fn materialize_return_source<R: ScratchpadRepository>(
    scratchpads: &R,
    req: &ReturnRequest,
) -> Result<ReturnPayload, MessengerError> {
    match &req.source {
        ReturnSource::Scratchpad { name, version } => {
            materialize_scratchpad_return_source(scratchpads, req, name, *version)
        }
        ReturnSource::InlineBytes(bytes) => Ok(inline_return_payload(
            bytes.clone(),
            req.format_hint.clone(),
            req.verdict_line.clone(),
        )),
    }
}

fn materialize_scratchpad_return_source<R: ScratchpadRepository>(
    scratchpads: &R,
    req: &ReturnRequest,
    name: &ScratchpadName,
    version: Option<u64>,
) -> Result<ReturnPayload, MessengerError> {
    let read_request = scratchpad_read_request(req.invocation_uuid, name.clone(), version);
    let record = scratchpads.read(&req.db_path, read_request)?;
    Ok(scratchpad_return_payload(
        name,
        req.format_hint.clone(),
        req.verdict_line.clone(),
        record,
    ))
}
