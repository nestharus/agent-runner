//! ## Declared roles
//!
//! `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/accessor.rs
//!     role: adapter
//!     Translates:
//!       - AppState setup repository access contract
//!       - test SetupRepository injection preference contract
//!       - GUI state.db opener fallback contract
//! ```

use crate::AppState;
use oulipoly_state::StateDb;
use oulipoly_state::repositories::SetupRepository;

pub fn open_state_db(state: &AppState) -> Result<StateDb, String> {
    state.state_db_opener.open_at(&state.db_path())
}

pub fn with_setup_repository<T>(
    state: &AppState,
    f: impl FnOnce(&dyn SetupRepository) -> Result<T, String>,
) -> Result<T, String> {
    #[cfg(test)]
    if let Some(repo) = state.setup_repository.as_ref() {
        return f(repo.as_ref());
    }

    let db = open_state_db(state)?;
    f(&db)
}
