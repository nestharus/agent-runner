//! Provider-specific setup probe residual island for L6/S10/S11.
//!
//! ## Declared roles
//!
//! `predicate`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/setup_flow/provider_probe.rs
//!     role: adapter
//!     Translates:
//!       - std::process::Command host-command probe contract
//!       - which executable lookup contract
//!       - claude provider availability invocation contract
//!       - probe status-fold contract
//!       - setup-needed boolean contract
//! ```

pub(crate) fn claude_probe_requires_setup() -> bool {
    // Provider-specific Claude probe residual for L6/S10/S11; preserve exactly.
    let output = std::process::Command::new("which").arg("claude").output();
    !matches!(output, Ok(o) if o.status.success())
}
