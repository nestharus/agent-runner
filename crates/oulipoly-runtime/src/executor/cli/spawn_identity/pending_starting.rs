//! Retained creator-owned finalization, not orphan recovery. Provider cleanup
//! keeps its own child handles; only exact generation Q enables this mutation.
use super::*;
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct Pending {
    contexts: Vec<SpawnIdentityContext>,
    running: bool,
}
fn pending() -> &'static Mutex<Pending> {
    static PENDING: OnceLock<Mutex<Pending>> = OnceLock::new();
    PENDING.get_or_init(Mutex::default)
}

pub(super) fn retain(context: &SpawnIdentityContext) -> Result<(), String> {
    let mut state = pending().lock().unwrap_or_else(|e| e.into_inner());
    if !state.contexts.contains(context) {
        state.contexts.push(context.clone());
    }
    if !state.running {
        state.running = true;
        if std::thread::Builder::new()
            .name("starting-finalizer".into())
            .spawn(drain)
            .is_err()
        {
            state.running = false;
            // Retain the obligation; a later enqueue can retry worker startup.
            return Err("Starting finalizer retained but worker unavailable".into());
        }
    }
    Ok(())
}

fn finish(context: &SpawnIdentityContext) -> bool {
    // Do not spend a two-second seal deadline on every pending generation.
    // A lost monitor remains unknown and cannot hold up independently ready Q.
    if !context.launch_custody.get().is_some_and(|c| c.quiescent()) {
        return false;
    }
    match exit_runtime_generation_outcome(Some(context), RuntimeTerminalReason::StartupFailed, None)
    {
        Ok(_) => true,
        Err(GenerationOperationError::Unknown | GenerationOperationError::StorageFailure) => false,
        Err(error) => {
            // Exact fence rejection cannot authorize a successor mutation.
            tracing::warn!(%error, "Retained Starting finalization rejected");
            true
        }
    }
}

fn drain() {
    loop {
        let contexts = {
            let mut state = pending().lock().unwrap_or_else(|e| e.into_inner());
            if state.contexts.is_empty() {
                state.running = false;
                return;
            }
            std::mem::take(&mut state.contexts)
        };
        let remaining = contexts
            .into_iter()
            .filter(|c| !finish(c))
            .collect::<Vec<_>>();
        pending()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contexts
            .extend(remaining);
        std::thread::sleep(Duration::from_millis(50));
    }
}
