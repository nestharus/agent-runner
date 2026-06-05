# Touched surfaces — OULIPOLY_DATA_DIR pin (c49bf80)

Incremental gate over `e8a8e1c..0c8f706` (two commits: c49bf80 fix + 0c8f706 proof-gap tests). Prior surfaces gated LOW at e8a8e1c; this gate covers the delta only.

## Production files touched
- `crates/oulipoly-state/src/paths.rs` — NEW: canonical data-dir resolution (OULIPOLY_DATA_DIR > XDG-derived fallback).
- `crates/oulipoly-state/src/{db.rs,pid_identity.rs,lib.rs}` — default-path resolution rerouted through paths.rs.
- `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` — spawn-env pin (set OULIPOLY_DATA_DIR for provider children unless already present).
- `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` — control-socket dir resolution via the shared helper.
- `crates/oulipoly-runtime/src/quota/{lock_paths.rs,auth_refresh_lock.rs,marker_verification/lock.rs,mod.rs}` — data-home resolution via shared helper.
- `crates/oulipoly-runtime/src/{services/lock.rs,session_metadata/locator.rs,session_replace/mod.rs,sessions/mod.rs}` — same reroute.
- `src-tauri/src/{usage/fetcher.rs,wiring.rs}` — same reroute.

## Tests added/changed
- `crates/oulipoly-state/tests/data_dir_precedence.rs` — NEW: precedence + fallback proofs.
- `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs` — spawn-env pin assertions.
- `src-tauri/tests/wu_d_proactive_wake_integration.rs` — shadow-XDG child notify resolves the spawning runner's sidecar (the live-bug reproduction).
- `src-tauri/tests/{wu_b_mailbox_integration.rs,wu_e_pty_delivery_integration.rs}` — harness env isolation for OULIPOLY_DATA_DIR.
- `crates/oulipoly-runtime/src/quota/marker_verification/tests.rs` — path-resolution tests updated.
