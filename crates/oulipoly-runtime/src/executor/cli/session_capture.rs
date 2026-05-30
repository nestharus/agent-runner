//! ## Declared roles
//!
//! Roles: none.
//!
//! Functionless module inventory; no A1 function-role claim.
//!
//! ## ACR-251 canonical-doc-as-schema declaration (PP-007 + PP-008)
//!
//! The runtime is a documented consumer of two implicit provider-output
//! schemas. Both schemas are pinned here.
//!
//! ### PP-007 - Forced-flag verified stdout JSONL
//!
//! - Input: provider stdout, decoded with `String::from_utf8_lossy`, then
//!   split by `lines()`.
//! - Each line is JSON-parsed. Non-JSON lines are skipped silently.
//! - Recognized event objects (success cases):
//!     - `{"type":"result","session_id":<string>}` returns `session_id`.
//!     - `{"type":"system","subtype":"init","session_id":<string>}`
//!       returns `session_id`.
//! - Recognized error cases (exact canonical strings):
//!     - When a `system.init` event is present but missing `session_id`,
//!       error is `"system.init event missing session_id"`.
//!     - When no recognized event is observed, error is
//!       `"stdout did not contain a result or system.init session_id event"`.
//!
//! ### PP-008 - Stdout JSON event session capture
//!
//! - Input: provider stdout, decoded with `String::from_utf8_lossy`, then
//!   split by `lines()`.
//! - Each line is JSON-parsed. Non-JSON lines are skipped silently.
//! - Match rule: object whose `type` field equals the configured
//!   `event_type`.
//! - Path lookup: dotted JSON path traversal over nested objects.
//! - Recognized error cases (exact canonical strings):
//!     - When the event is observed but the dotted id path returns nothing,
//!       error is `"event '<event_type>' missing id path '<event_id_path>'"`.
//!     - When no matching event is observed, error is
//!       `"stdout did not contain event '<event_type>'"`.
//!
//! `tests/age_164_c5_resume_capture.rs` (`acr251_pp007_*` and
//! `acr251_pp008_*`) pins these strings; the push-pull auditor accepts
//! this rustdoc as canonical-doc-as-schema proof for PP-007 and PP-008.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture.rs
//!     role: adapter
//!     Translates:
//!       - provider-session-capture-config-contract
//!       - executor-session-capture-component-set-contract
//! ```

mod args;
mod json_path;
mod messages;
mod parse_forced_flag;
mod parse_stdout_event;
mod paths;
mod plan;
mod start_known;

pub(super) use crate::executor::provider_specific::session_capture::remove_unsanctioned_money_fields;
pub(super) use parse_forced_flag::parse_forced_flag_verified_session_id;
pub(super) use parse_stdout_event::parse_stdout_json_event_session_id;
pub(super) use plan::{CapturePlan, build_capture_plan};
pub use start_known::start_known_provider_session_id;
