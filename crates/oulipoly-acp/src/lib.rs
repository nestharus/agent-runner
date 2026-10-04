//! ACP v2 **draft** client core.
//!
//! This crate speaks the client side of the Agent Client Protocol (ACP) v2
//! draft for one purpose: delivering a communication to a harness and
//! learning, with honest labels, what happened to it.
//!
//! # Pinned schema
//!
//! This implements a consumed subset of the published draft schema pinned in
//! [`pin`]. ACP v2 is a Draft; later `alpha` releases may change any shape
//! used here. Nothing in this crate has been run against a real harness. Its
//! end-to-end behaviour is exercised only against the deterministic
//! reference peer in this crate's tests.
//!
//! # What an outcome means
//!
//! * **Insertion acknowledgement.** A v2 `session/prompt` response carries a
//!   `messageId`. Per the pinned schema it means the agent inserted the user
//!   message into its ACP conversation. It is **not** turn completion,
//!   **not** physical drain of any queue, and **not** dedup.
//! * **Session readiness.** The next unconsumed idle `state_update` since the
//!   session's **first tracked attempt**, separately with
//!   [`AcpClient::await_session_idle`]. Later attempts do not reset this history;
//!   idle may precede the latest message. The draft provides no completion
//!   correlation with that message.
//!   Another foreground task may have caused idle; this is not physical drain.
//! * **Message identity.** [`OutboundMessage::fresh`] mints a Linux random
//!   identity for one communication and carries it in the prompt's `_meta` under
//!   [`MESSAGE_KEY_META`]. Retries reuse it; [`OutboundMessage`] has no way
//!   to change it.
//!
//! # The local dedup contract
//!
//! The schema's `messageId` is an acknowledgement, not an idempotency key: a
//! retry may be accepted as a distinct submission. Dedup is therefore a
//! receiver-side contract defined here, carried entirely in sanctioned
//! namespaced `_meta` keys:
//!
//! 1. A complying agent advertises [`DEDUP_CONTRACT_META`] with
//!    `{"version": 1}` in its `initialize` response `_meta`.
//! 2. On a `session/prompt` whose `_meta` carries a message key it has already
//!    inserted **in that session**, it inserts nothing and returns the original
//!    `messageId`. This promise lasts for the entire resumable session lifetime.
//!    A receiver unable to retain that memory across restart/resume must not
//!    advertise the contract for that session.
//! 3. Every prompt response echoes the key under [`MESSAGE_KEY_META`] and
//!    sets [`DUPLICATE_META`] to `true` when it returned an earlier
//!    insertion.
//!
//! [`Acceptance::basis`] identifies either a single counted attempt for a
//! fresh unforked identity, or a complete same-session history in which every
//! attempt advertised the contract and the current answer echoes the key.
//! Supplied/recreated/recovered/cloned keys have unknown history. Calling
//! [`OutboundMessage::key`] permits forks and abandons its complete-history
//! claim. Capturing a key from outgoing wire bytes or a custom transport bypasses
//! that downgrade; forks made from it are outside the original tracked history.
//! These labels therefore trust the caller/transport not to re-supply captured
//! keys. A current advertisement cannot repair earlier non-contract or unknown
//! attempts.
//! An insertion ACK without either basis is [`DeliveryOutcome::DuplicateUnknown`].
//! A valid error stays distinct but counts as an insertion-uncertain attempt.
//!
//! These are contractual evidence labels, not measured receiver compliance.
//! An advertisement says nothing about another receiver, session or store;
//! same-session resume relies on the receiver's lifetime promise, not a
//! client-invented continuity proof. No arbitrary provider-effects claim or
//! durable history is supplied: [`OutboundMessage::fresh_recorded`] only
//! lets an origin owner store a fresh key, and a message rebuilt from that
//! store is a supplied key with unknown history. Messages are bound to
//! their first session-id string, which is trusted contract scope, not proof
//! of receiver/store identity or cross-receiver continuity.
//!
//! Empty `messageId` is conservatively refused (stricter than the schema).
//! Unknown optional schema fields are ignored, not claimed fully validated.
//! Consumed response results, notification params and mandatory nested
//! initialize info require wire objects. Invalid capability shapes cannot
//! advertise session support. Session updates require `jsonrpc: "2.0"` and
//! object params/update with the consumed discriminators and fields; malformed
//! notifications are ignored without retaining events or readiness. Ignored
//! optional fields and unconsumed update variants are not fully validated.
//!
//! Transport limits accepted for this unused primitive: synchronous blocking
//! reads, no deadline or line-size bound, read/UTF-8 errors reported as closed,
//! and an unbounded event Vec. Reopen these at the first real-harness process
//! or supervisor-loop slice. Readiness evidence is retained per session.
//!
//! # Not implemented
//!
//! * ACP v1 is not accepted as v2 consumption. A peer that negotiates v1 gets
//!   [`NegotiationFailure::UnsupportedVersion`], and no prompt is sent.
//! * No v1 fallback, no busy-queue semantics (the draft leaves them
//!   unspecified), no authentication, no client-side tool methods, no real
//!   harness spawning.

mod client;
mod transport;
pub mod wire;

pub use client::{
    Acceptance, AcpClient, AtMostOnceBasis, ClientInfo, DeliveryOutcome, IdleWaitFailure,
    MessageKey, NegotiatedPeer, NegotiationFailure, NoAckCause, OutboundMessage, RequestFailure,
    SessionEvent, SessionIdle,
};
pub use transport::{Incoming, LineTransport, PeerClosed, Transport};

/// Provenance of the ACP v2 draft schema subset implemented here.
pub mod pin {
    /// Release tag in `agentclientprotocol/agent-client-protocol`.
    pub const SCHEMA_TAG: &str = "schema-v2.0.0-alpha.7";
    /// Commit that the tag points to.
    pub const SCHEMA_COMMIT: &str = "1761180eeddf0828d4ecc367106a632c61be06d9";
    /// The schema file at that commit.
    pub const SCHEMA_URL: &str = "https://github.com/agentclientprotocol/agent-client-protocol/blob/1761180eeddf0828d4ecc367106a632c61be06d9/schema/v2/schema.json";
    /// Git blob id of `schema/v2/schema.json` at [`SCHEMA_COMMIT`].
    pub const SCHEMA_BLOB: &str = "bc085103d0c2c5daf856447ed1b10bd8944250a2";
}

/// The only protocol version this client accepts.
pub const PROTOCOL_VERSION: u16 = 2;

/// `_meta` key carrying the sender's stable message key on a prompt request,
/// and its echo on a complying prompt response.
pub const MESSAGE_KEY_META: &str = "oulipoly.ai/messageKey";

/// `_meta` key a complying agent uses in its `initialize` response to
/// advertise the dedup contract. The client sends it in its `initialize`
/// request to say that its prompts carry message keys.
pub const DEDUP_CONTRACT_META: &str = "oulipoly.ai/messageKeyDedup";

/// `_meta` key on a prompt response: `true` when the agent returned an
/// earlier insertion of the same message key instead of inserting again.
pub const DUPLICATE_META: &str = "oulipoly.ai/duplicate";

/// Version of the dedup contract described in the crate documentation.
pub const DEDUP_CONTRACT_VERSION: u64 = 1;
