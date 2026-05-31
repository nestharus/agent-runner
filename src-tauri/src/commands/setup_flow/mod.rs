//! Setup-flow Tauri command facade.
//!
//! `check_setup_needed` intentionally preserves the existing provider-specific
//! Claude availability probe as an L6/S10/S11 residual. AGE-238 only relocates
//! that command and does not generalize the probe.
//!
//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/setup_flow/mod.rs
//!     role: adapter
//!     Translates:
//!       - Tauri IPC setup-flow command contract
//!       - setup session id string contract
//!       - setup event channel contract
//!       - setup memory snapshot command contract
//! ```

mod accessor;
mod formatter;
pub mod orchestration;

pub(crate) use orchestration::{
    __cmd__cancel_setup, __cmd__check_setup_needed, __cmd__detect_clis, __cmd__get_memory_graph,
    __cmd__setup_respond, __cmd__start_cli_setup, __cmd__start_setup, cancel_setup,
    check_setup_needed, detect_clis, get_memory_graph, setup_respond, start_cli_setup, start_setup,
};
