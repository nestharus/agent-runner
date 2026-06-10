//! ## Declared roles
//!
//! `mapper`, `orchestration`

use oulipoly_state::mailbox::{MailboxDb, WakeClaimAcquireResult, WakeClaimRequest};

use super::StartWakeInput;
use crate::wake_coordinator::constants::WAKE_CLAIM_STALE_AFTER_SECONDS;

pub(super) fn acquire_wake_claim(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    claim_token: &str,
) -> Result<WakeClaimAcquireResult, String> {
    db.try_acquire_or_renew_wake_claim(wake_claim_request(input, claim_token), input.renew_token)
}

fn wake_claim_request<'a>(input: StartWakeInput<'a>, claim_token: &'a str) -> WakeClaimRequest<'a> {
    WakeClaimRequest {
        session_id: input.session_id,
        claim_token,
        reason: input.reason,
        auto_wake_count: input.auto_wake_count,
        wake_invocation_uuid: None,
        stale_after_seconds: WAKE_CLAIM_STALE_AFTER_SECONDS,
    }
}
