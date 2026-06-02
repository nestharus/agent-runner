//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/providers_accounts/orchestration.rs
//!     role: adapter
//!     Translates:
//!       - Tauri IPC provider/account command contract
//!       - account validation and creation lifecycle contract
//!       - provider sync detection delegation contract
//! ```
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/providers_accounts/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: provider-account command lifecycle: repository reads, provider existence checks, account validation, and account mutation are one IPC lifecycle.
//!     Owns:
//!       - src-tauri/src/commands/providers_accounts/accessor.rs
//!       - src-tauri/src/commands/providers_accounts/validator.rs
//!       - src-tauri/src/commands/providers_accounts/mapper.rs
//!       - src-tauri/src/commands/providers_accounts/formatter.rs
//!       - src-tauri/src/commands/accessor.rs
//!   - component: src-tauri/src/commands/providers_accounts/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: provider-sync mapping lifecycle: CLI detection, residual display-name mapping, record projection, and repository upsert are one IPC lifecycle.
//!     Owns:
//!       - src-tauri/src/commands/providers_accounts/accessor.rs
//!       - src-tauri/src/commands/providers_accounts/mapper.rs
//!       - src-tauri/src/commands/providers_accounts/display_name.rs
//! ```

use super::{AddAccountInput, accessor, formatter, mapper, validator};
use crate::AppState;
use oulipoly_setup as setup_core;
use oulipoly_state::{AccountRecord, CliProviderRecord};

#[tauri::command]
pub(crate) fn list_cli_providers(
    state: tauri::State<AppState>,
) -> Result<Vec<CliProviderRecord>, String> {
    list_cli_providers_inner(&state)
}

pub fn list_cli_providers_inner(state: &AppState) -> Result<Vec<CliProviderRecord>, String> {
    accessor::list_cli_providers_inner(state)
}

#[tauri::command]
pub(crate) fn get_cli_provider(
    state: tauri::State<AppState>,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    get_cli_provider_inner(&state, cli_name)
}

pub fn get_cli_provider_inner(
    state: &AppState,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    accessor::get_cli_provider_inner(state, &cli_name)?
        .ok_or_else(|| formatter::provider_not_found_error(&cli_name))
}

#[tauri::command]
pub(crate) fn list_accounts(
    state: tauri::State<AppState>,
    provider: Option<String>,
) -> Result<Vec<AccountRecord>, String> {
    list_accounts_inner(&state, provider)
}

pub fn list_accounts_inner(
    state: &AppState,
    provider: Option<String>,
) -> Result<Vec<AccountRecord>, String> {
    accessor::list_accounts_inner(state, provider.as_deref())
}

#[tauri::command]
pub(crate) fn add_account(
    state: tauri::State<AppState>,
    account: AddAccountInput,
) -> Result<AccountRecord, String> {
    add_account_inner(&state, account)
}

pub fn add_account_inner(
    state: &AppState,
    account: AddAccountInput,
) -> Result<AccountRecord, String> {
    validator::validate_add_account_input(&account)?;
    let provider = account.provider.clone();
    accessor::get_cli_provider_inner(state, &provider)?
        .ok_or_else(|| formatter::provider_not_found_error(&provider))?;
    let record = mapper::account_record_from_input(account);
    accessor::insert_account_inner(state, &record)?;
    Ok(record)
}

#[tauri::command]
pub(crate) fn remove_account(
    state: tauri::State<AppState>,
    id: String,
    provider: String,
) -> Result<bool, String> {
    remove_account_inner(&state, id, provider)
}

pub fn remove_account_inner(
    state: &AppState,
    id: String,
    provider: String,
) -> Result<bool, String> {
    accessor::remove_account_inner(state, &id, &provider)
}

#[tauri::command]
pub(crate) fn sync_provider(
    state: tauri::State<AppState>,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    let cli_info = accessor::detect_single_cli(&cli_name);
    let record = mapper::sync_provider_record_from_cli_info(&cli_name, cli_info);

    sync_provider_persist_record(&state, &record)?;
    Ok(record)
}

pub fn sync_provider_record_from_cli_info(
    cli_name: &str,
    cli_info: setup_core::detection::CliInfo,
) -> CliProviderRecord {
    mapper::sync_provider_record_from_cli_info(cli_name, cli_info)
}

pub fn sync_provider_display_name(cli_name: &str) -> &str {
    super::display_name::sync_provider_display_name(cli_name)
}

pub fn sync_provider_persist_record(
    state: &AppState,
    record: &CliProviderRecord,
) -> Result<(), String> {
    accessor::sync_provider_persist_record(state, record)
}
