//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/providers_accounts/mod.rs
//!     role: adapter
//!     Translates:
//!       - AddAccountInput deserialization contract
//!       - AddAccountInput field-name wire contract
//!       - Tauri IPC provider/account command contract
//! ```

mod accessor;
mod display_name;
mod formatter;
mod mapper;
pub mod orchestration;
mod validator;

use oulipoly_state::AuthMethod;
use serde::Deserialize;

/// Input payload for adding a new account.
#[derive(Deserialize)]
pub struct AddAccountInput {
    pub id: String,
    pub provider: String,
    pub profile_name: String,
    pub auth_method: AuthMethod,
}

pub(crate) use orchestration::{
    __cmd__add_account, __cmd__get_cli_provider, __cmd__list_accounts, __cmd__list_cli_providers,
    __cmd__remove_account, __cmd__sync_provider, add_account, get_cli_provider, list_accounts,
    list_cli_providers, remove_account, sync_provider,
};
