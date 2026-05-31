//! ## Declared roles
//!
//! `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/setup_flow/accessor.rs
//!     role: adapter
//!     Translates:
//!       - AppState model-cache mutex contract
//!       - setup input sender storage contract
//!       - setup memory DB path contract
//!       - MemoryGraph open/snapshot contract
//! ```

use super::formatter;
use crate::AppState;
use oulipoly_setup::actions::UserResponse;
use oulipoly_setup::memory::{MemoryGraph, MemorySnapshot};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

pub fn create_user_response_channel() -> (mpsc::Sender<UserResponse>, mpsc::Receiver<UserResponse>)
{
    mpsc::channel::<UserResponse>(16)
}

pub fn models_cache_is_empty(state: &AppState) -> Result<bool, String> {
    let models = state.models.lock().map_err(formatter::lock_error)?;
    Ok(models.is_empty())
}

pub fn store_setup_sender(state: &AppState, tx: mpsc::Sender<UserResponse>) -> Result<(), String> {
    let mut guard = state.setup_input_tx.lock().map_err(formatter::lock_error)?;
    *guard = Some(tx);
    Ok(())
}

pub fn current_setup_sender(
    state: &AppState,
) -> Result<Option<mpsc::Sender<UserResponse>>, String> {
    let guard = state.setup_input_tx.lock().map_err(formatter::lock_error)?;
    Ok(guard.clone())
}

pub fn send_user_response(
    tx: &mpsc::Sender<UserResponse>,
    response: UserResponse,
) -> Result<(), mpsc::error::SendError<UserResponse>> {
    tx.blocking_send(response)
}

pub fn clear_setup_sender(state: &AppState) -> Result<(), String> {
    let mut guard = state.setup_input_tx.lock().map_err(formatter::lock_error)?;
    *guard = None;
    Ok(())
}

pub fn setup_memory_db_path(state: &AppState) -> PathBuf {
    state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("state.db")
}

pub fn open_memory_graph(db_path: &Path) -> Result<MemoryGraph, String> {
    MemoryGraph::open(db_path)
}

pub fn memory_snapshot(db_path: &Path) -> Result<MemorySnapshot, String> {
    let graph = MemoryGraph::open(db_path)?;
    graph.snapshot()
}
