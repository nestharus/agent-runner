//! ## Declared roles
//!
//! `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/providers_accounts/mapper.rs
//!     role: adapter
//!     Translates:
//!       - AddAccountInput to AccountRecord mapping contract
//!       - CliInfo to CliProviderRecord mapping contract
//!       - RFC3339 timestamp field contract
//! ```

use super::{AddAccountInput, display_name};
use oulipoly_setup as setup_core;
use oulipoly_state::{AccountRecord, AuthStatus, CliProviderRecord};

pub fn account_record_from_input(account: AddAccountInput) -> AccountRecord {
    let now = chrono::Utc::now().to_rfc3339();
    AccountRecord {
        id: account.id,
        provider: account.provider,
        profile_name: account.profile_name,
        auth_method: account.auth_method,
        auth_status: AuthStatus::Unknown,
        created_at: now,
    }
}

pub fn sync_provider_record_from_cli_info(
    cli_name: &str,
    cli_info: setup_core::detection::CliInfo,
) -> CliProviderRecord {
    let now = chrono::Utc::now().to_rfc3339();
    CliProviderRecord {
        cli_name: cli_info.name,
        display_name: display_name::sync_provider_display_name(cli_name).to_string(),
        installed: cli_info.installed,
        version: cli_info.version,
        config_dir: cli_info.config_dir.map(|p| p.to_string_lossy().to_string()),
        last_synced: Some(now),
    }
}
