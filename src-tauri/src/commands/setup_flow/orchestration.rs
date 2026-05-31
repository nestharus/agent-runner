//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/setup_flow/orchestration.rs
//!     role: adapter
//!     Translates:
//!       - Tauri IPC setup-flow command contract
//!       - setup input channel lifecycle contract
//!       - setup memory graph lifecycle contract
//!       - setup detection delegation contract
//! ```
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/commands/setup_flow/orchestration.rs
//!     role: intrinsic-surface
//!     Domain: setup-flow command lifecycle: session id creation, response channel storage, memory opening, and setup flow launch are one ordered IPC lifecycle.
//!     Owns:
//!       - src-tauri/src/commands/setup_flow/accessor.rs
//!       - src-tauri/src/commands/setup_flow/formatter.rs
//!       - src-tauri/src/setup/flow.rs
//! ```
//!
//! Provider-specific availability probe residual is flagged for L6/S10/S11.

use super::{accessor, formatter, provider_probe};
use crate::{AppState, setup};
use oulipoly_setup as setup_core;
use oulipoly_setup::actions::{SetupEvent, UserResponse};
use std::path::PathBuf;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

#[tauri::command]
pub(crate) fn check_setup_needed(state: tauri::State<AppState>) -> Result<bool, String> {
    let models_empty = accessor::models_cache_is_empty(&state)?;
    if models_empty {
        return Ok(true);
    }
    Ok(provider_probe::claude_probe_requires_setup())
}

#[tauri::command]
pub(crate) async fn start_setup(
    state: tauri::State<'_, AppState>,
    on_event: Channel<SetupEvent>,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = accessor::create_user_response_channel();
    accessor::store_setup_sender(&state, tx)?;

    let sid = session_id.clone();
    let db_path = accessor::setup_memory_db_path(&state);

    spawn_setup_flow(on_event, rx, db_path, sid);

    Ok(session_id)
}

#[tauri::command]
pub(crate) fn setup_respond(
    state: tauri::State<AppState>,
    response: UserResponse,
) -> Result<(), String> {
    let sender = accessor::current_setup_sender(&state)?;
    if let Some(tx) = sender {
        return accessor::send_user_response(&tx, response).map_err(formatter::setup_send_error);
    }

    Err(formatter::no_active_setup_session_error())
}

#[tauri::command]
pub(crate) fn cancel_setup(state: tauri::State<AppState>) -> Result<(), String> {
    accessor::clear_setup_sender(&state)
}

#[tauri::command]
pub(crate) async fn start_cli_setup(
    state: tauri::State<'_, AppState>,
    cli_name: String,
    on_event: Channel<SetupEvent>,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = accessor::create_user_response_channel();
    accessor::store_setup_sender(&state, tx)?;

    let sid = session_id.clone();
    let db_path = accessor::setup_memory_db_path(&state);
    let cli = cli_name.clone();

    spawn_cli_setup_flow(on_event, rx, db_path, sid, cli);

    Ok(session_id)
}

#[tauri::command]
pub(crate) fn detect_clis() -> Result<setup_core::detection::DetectionReport, String> {
    Ok(setup_core::detection::detect_all())
}

#[tauri::command]
pub(crate) fn get_memory_graph(
    state: tauri::State<AppState>,
) -> Result<setup_core::memory::MemorySnapshot, String> {
    let db_path = accessor::setup_memory_db_path(&state);
    accessor::memory_snapshot(&db_path)
}

fn spawn_setup_flow(
    on_event: Channel<SetupEvent>,
    rx: mpsc::Receiver<UserResponse>,
    db_path: PathBuf,
    sid: String,
) {
    tauri::async_runtime::spawn(async move {
        let memory = match accessor::open_memory_graph(&db_path) {
            Ok(m) => m,
            Err(e) => {
                let _ = on_event.send(formatter::memory_open_error_event(e));
                return;
            }
        };

        let flow = setup::flow::SetupFlow::new(on_event, rx, memory, sid);
        flow.run().await;
    });
}

fn spawn_cli_setup_flow(
    on_event: Channel<SetupEvent>,
    rx: mpsc::Receiver<UserResponse>,
    db_path: PathBuf,
    sid: String,
    cli: String,
) {
    tauri::async_runtime::spawn(async move {
        let memory = match accessor::open_memory_graph(&db_path) {
            Ok(m) => m,
            Err(e) => {
                let _ = on_event.send(formatter::memory_open_error_event(e));
                return;
            }
        };

        let flow = setup::flow::SetupFlow::new(on_event, rx, memory, sid);
        flow.run_for_cli(&cli).await;
    });
}
