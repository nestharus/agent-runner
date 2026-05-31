//! ## Declared roles
//!
//! `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/providers_accounts/formatter.rs
//!     role: adapter
//!     Translates:
//!       - provider-not-found error string contract
//! ```

pub fn provider_not_found_error(name: &str) -> String {
    format!("Provider '{name}' not found")
}
