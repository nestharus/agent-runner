//! ## Declared roles
//!
//! `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/providers_accounts/validator.rs
//!     role: adapter
//!     Translates:
//!       - AddAccountInput emptiness validation contract
//!       - account validation error string contract
//! ```

use super::AddAccountInput;

pub fn validate_add_account_input(account: &AddAccountInput) -> Result<(), String> {
    if account.id.is_empty() {
        return Err("Account id cannot be empty".to_string());
    }
    if account.provider.is_empty() {
        return Err("Account provider cannot be empty".to_string());
    }
    if account.profile_name.is_empty() {
        return Err("Account profile_name cannot be empty".to_string());
    }

    Ok(())
}
