//! ## Declared roles
//!
//! Roles: validator.
//!
//! - validator: validates command parts and preserves canonical empty-command
//!   failure text.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/command_validate.rs
//!     role: adapter
//!     Translates:
//!       - std-process-command-contract
//!       - provider-config-launch-contract
//! ```

pub(super) fn validate_command_parts(parts: &[String]) -> Result<(), String> {
    if parts.is_empty() {
        return Err("Empty command".to_string());
    }
    Ok(())
}
