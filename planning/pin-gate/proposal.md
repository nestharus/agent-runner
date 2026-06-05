# OULIPOLY_DATA_DIR Pin Fix Proposal

The live bug is that a wrapper or descendant provider can override `XDG_DATA_HOME`, then descendant `agents` invocations resolve agent-runner state under that shadow data root instead of the spawning runner's sidecar. The fix is to pin the canonical app data directory into provider child process environments as `OULIPOLY_DATA_DIR`, and to make default state resolution prefer that pin before falling back to the XDG-derived platform data directory.

Precedence is intentionally narrow: an existing `OULIPOLY_DATA_DIR` wins, default resolution otherwise derives the app directory from `dirs::data_dir()`, and explicit user/config paths remain unchanged because the reroutes only affect default path helpers.

## Proof plan

Runtime claim: Default state path resolution prefers `OULIPOLY_DATA_DIR` over the XDG-derived fallback.
Proof method: `crates/oulipoly-state/tests/data_dir_precedence.rs::default_state_locations_prefer_oulipoly_data_dir_over_xdg_data_home`.
Evidence-class match: The test sets both env surfaces and calls the shipped default path APIs (`StateDb::default_path`, `PidIdentityDb::default_path`, `MailboxDb::default_path`), so it exercises the runtime resolution path rather than inspecting helper internals.

Runtime claim: Default state path resolution falls back to the XDG-derived app data directory when `OULIPOLY_DATA_DIR` is unset.
Proof method: `crates/oulipoly-state/tests/data_dir_precedence.rs::default_state_locations_fall_back_to_xdg_data_home_when_unpinned`.
Evidence-class match: The test removes the pin, sets `XDG_DATA_HOME`, and asserts the shipped default path APIs resolve under `XDG_DATA_HOME/oulipoly-agent-runner`, so it covers the actual unpinned runtime fallback.

Runtime claim: A spawned provider child receives an `OULIPOLY_DATA_DIR` pin when the spawning runner starts from an unpinned XDG data root.
Proof method: `src-tauri/tests/wu_d_proactive_wake_integration.rs::provider_shadow_xdg_notify_uses_pinned_data_dir_and_wakes`.
Evidence-class match: The provider script runs as the real spawned provider child and exits if `OULIPOLY_DATA_DIR` is absent, so the evidence is the child process environment produced by the runtime spawn path.

Runtime claim: Spawn-side sidecar capture still writes the verified sidecar row under the expected app data directory with the harness pin removed.
Proof method: `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs::spawn_capture_writes_verified_sidecar_row_without_state_schema_change`.
Evidence-class match: The test executes `RuntimeExecutorService::execute`, reads the actual PID identity sidecar DB under the XDG-derived app data directory, and verifies state schema stability; it is runtime sidecar evidence, but it does not directly assert the child env value or pre-existing-pin preservation.

Runtime claim: A pre-existing `OULIPOLY_DATA_DIR` in the parent environment is not overridden during provider spawn.
Proof method: `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs::spawn_preserves_preexisting_oulipoly_data_dir_in_provider_child`.
Evidence-class match: The test sets the spawning process env to `OULIPOLY_DATA_DIR=<custom dir>` with an isolated `XDG_DATA_HOME`, executes a real fixture provider through `RuntimeExecutorService::execute`, and asserts the child-recorded env equals the custom pin rather than the runner's XDG-derived default.

Runtime claim: A child whose environment changes `XDG_DATA_HOME` still notifies into the spawning runner's sidecar and the wake fires.
Proof method: `src-tauri/tests/wu_d_proactive_wake_integration.rs::provider_shadow_xdg_notify_uses_pinned_data_dir_and_wakes`.
Evidence-class match: The provider process exports a different `XDG_DATA_HOME`, runs a real descendant `notify`, waits for the resumed prompt containing `handle: h-shadow-xdg`, verifies the mailbox row is delivered in the original fixture sidecar, and asserts the shadow XDG app state directory was not created.

Runtime claim: Wake-resume env preserves the data-dir pin into the resumed provider child.
Proof method: `src-tauri/tests/wu_d_proactive_wake_integration.rs::provider_shadow_xdg_notify_uses_pinned_data_dir_and_wakes`.
Evidence-class match: The provider process shadows `XDG_DATA_HOME`, a real descendant `notify` triggers the wake-resume path, and the resumed provider child records `OULIPOLY_DATA_DIR`; the test asserts that value equals the spawning runner's pinned app data directory.
