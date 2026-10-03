//! ACP v2 **draft** client core.
//!
//! This crate speaks the client side of the Agent Client Protocol (ACP) v2
//! draft for one purpose: delivering a communication to a harness and
//! learning, with honest labels, what happened to it.
//!
//! # Pinned schema
//!
//! Conformance is claimed only against the published draft schema pinned in
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
//! * **Turn completion.** An idle `state_update` session update, observed
//!   separately with [`AcpClient::await_turn_end`]. It says foreground work
//!   stopped; it is not physical drain either.
//! * **Message identity.** The sender mints one [`MessageKey`] per
//!   communication and carries it in the prompt's `_meta` under
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
//!    inserted, it inserts nothing and returns the original `messageId`.
//! 3. Every prompt response echoes the key under [`MESSAGE_KEY_META`] and
//!    sets [`DUPLICATE_META`] to `true` when it returned an earlier
//!    insertion.
//!
//! A complying receiver therefore inserts a given message identity **at most
//! once**. That is the only at-most-once claim this crate makes. It makes no
//! claim about arbitrary provider execution or effects. A retry after a lost
//! acknowledgement to a receiver that does not demonstrably comply is labelled
//! [`DeliveryOutcome::DuplicateUnknown`] and never treated as at-most-once.
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
    Acceptance, AcpClient, ClientInfo, DeliveryOutcome, MessageKey, NegotiatedPeer,
    NegotiationFailure, NoAckCause, OutboundMessage, RequestFailure, SessionEvent, TurnEnd,
    TurnWaitFailure,
};
pub use transport::{Incoming, LineTransport, PeerClosed, Transport};

/// Provenance of the ACP v2 draft schema this crate conforms to.
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
