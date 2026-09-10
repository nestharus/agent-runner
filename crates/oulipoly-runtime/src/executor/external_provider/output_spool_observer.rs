//! Launch-event delivery into complete disk-backed output custody.

use crate::executor::ExecutionOutputSpool;
use oulipoly_provider::stream::DecodedLaunchEvent;

pub(super) fn observe_output(
    spool: &ExecutionOutputSpool,
    event: &DecodedLaunchEvent,
) -> Result<(), String> {
    spool.observe(event)
}
