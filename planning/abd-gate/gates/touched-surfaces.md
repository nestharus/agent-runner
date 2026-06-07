# A+B+D async-bash agent-runner touched production surfaces (origin/main..HEAD)

## New files (100% additions — fully WU-owned):
- crates/oulipoly-runtime/src/executor/cli/headless.rs
- crates/oulipoly-runtime/src/executor/cli/interactive.rs
- crates/oulipoly-runtime/src/executor/cli/resume_execution.rs
- crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs
- crates/oulipoly-state/src/lib.rs
- crates/oulipoly-state/src/mailbox.rs
- crates/oulipoly-state/src/pid_identity.rs
- src-tauri/src/commands/mailbox.rs
- src-tauri/src/commands/mod.rs
- src-tauri/src/commands/notify.rs
- src-tauri/src/commands/pid_session.rs
- src-tauri/src/mailbox_delivery.rs
- src-tauri/src/main.rs
- src-tauri/src/run/balancing/finalization.rs
- src-tauri/src/run/balancing/orchestration.rs
- src-tauri/src/usage/cli.rs
- src-tauri/src/wake_coordinator.rs

## Existing files with additive hooks (whole-file in scope per touched-file ownership):
- crates/oulipoly-runtime/src/executor/cli.rs
- crates/oulipoly-runtime/src/executor/cli/provider_execution.rs
- crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs
- src-tauri/src/dispatch.rs
- src-tauri/src/migration_providers.rs
- src-tauri/src/run/repl/orchestration.rs
- src-tauri/src/run/resume/orchestration.rs
