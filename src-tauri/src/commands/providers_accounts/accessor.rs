//! ## Declared roles
//!
//! `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/providers_accounts/accessor.rs
//!     role: adapter
//!     Translates:
//!       - SetupRepository provider/account read contract
//!       - SetupRepository account mutation contract
//!       - SetupRepository provider sync persistence contract
//! ```

use crate::AppState;
use crate::commands::accessor as command_accessor;
use oulipoly_setup as setup_core;
use oulipoly_state::{AccountRecord, CliProviderRecord};

pub fn list_cli_providers_inner(state: &AppState) -> Result<Vec<CliProviderRecord>, String> {
    command_accessor::with_setup_repository(state, |repo| repo.list_cli_providers())
}

pub fn get_cli_provider_inner(
    state: &AppState,
    cli_name: &str,
) -> Result<Option<CliProviderRecord>, String> {
    command_accessor::with_setup_repository(state, |repo| repo.get_cli_provider(cli_name))
}

pub fn list_accounts_inner(
    state: &AppState,
    provider: Option<&str>,
) -> Result<Vec<AccountRecord>, String> {
    command_accessor::with_setup_repository(state, |repo| repo.list_accounts(provider))
}

pub fn insert_account_inner(state: &AppState, record: &AccountRecord) -> Result<(), String> {
    command_accessor::with_setup_repository(state, |repo| repo.insert_account(record))
}

pub fn remove_account_inner(state: &AppState, id: &str, provider: &str) -> Result<bool, String> {
    command_accessor::with_setup_repository(state, |repo| repo.delete_account(id, provider))
}

pub fn sync_provider_persist_record(
    state: &AppState,
    record: &CliProviderRecord,
) -> Result<(), String> {
    command_accessor::with_setup_repository(state, |repo| repo.upsert_cli_provider(record))
}

pub fn detect_single_cli(cli_name: &str) -> setup_core::detection::CliInfo {
    setup_core::detection::detect_single_cli(cli_name)
}
