# AGE-319 terminal marker and auth integration

Base: `4573d508`. Integrated source commits: terminal marker `cfc4717d` and auth coalescence `b20635ac`. This is a closed private Runner/broker candidate. It does not open the selector or establish installed host or product behavior.

## Resolution

- Fresh route candidates now carry a version 2 config-bound terminal recognizer. Older private candidate JSON without the recognizer fails closed on readback. The broker records typed terminal evidence only from the selected provider's exact K and physical Q. Typed quota, auth, and availability markers survive restarts; a stale healthy quota Q, generic nonzero status, or explicit pin cannot clear a confirmed marker.
- The J01 lineage's explicit unknown artifact remains authoritative. Historical missing/changed K and unknown Q refuse selection. An unresolved applicable quota effect also refuses an unpinned selection even if another account appears healthy. The current route's prepared grant is excluded only during its first pre-K marker check; previous consumed K remains checked exactly. No unknown state authorizes another provider K or automatic Runner replay.
- Cross-root auth followers hold a durable reference to the exact source effect ID. They wait for the source's physical Q; only a drained, successful auth result authorizes their own quota retry. Failure, pending, unknown, and lost reply do not create a second auth K or imply success.
- A typed provider auth rejection after a healthy quota Q now authorizes one post-provider-Q auth refresh and a quota retry on the same root. The Runner diagnoses the original completed provider stderr, and the broker independently requires the matching typed provider Q before accepting the auth K. The Runner never retries provider K. A later root may use the successful physical auth Q across roots only for the same account, config, index, and effect environment, and only when that Q is newer than the marker.
- The old v29/State/WAL paths and the exact E/h/f/c protocol remain unchanged by this integration. No real provider or authentication call was made.

## Verification

- `cargo fmt --all -- --check`, `git diff --check`, `CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo check --workspace -j2`, and `cargo check -p oulipoly-agent-runner --features age319-private-broker-fixture -j2`: passed.
- `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --bin oulipoly-kernel-broker -j2`: 31/31 passed. This includes typed terminal classification, old/new physical Q ordering, pin/fallback, generic nonzero/cancel, auth follower source ID/provenance, failed auth, unknown historical K/Q, unknown quota blocking alternate K, and J01 frame checks.
- Private root loop: 61/62 listed modes passed using the built Runner, broker, and synthetic provider fixture. The new `normal_model_provider_auth_after_healthy` mode checks healthy first quota Q, typed provider auth Q, exactly one provider K, one auth K/Q, one quota retry K/Q, a mapped runtime result, and unchanged old State/WAL. Auth success, reply-loss, restart, quota, no-pin, legacy, native, and recipient modes also passed. The only failed mode, `normal_handoff_bash_child`, was supplied the installed `agent-bash` image, which lacks the private `__age319-private-admit-child-v1` fixture subcommand; this worktree contains no matching Bash fixture image.
- An additional `cargo check --workspace --all-targets --features age319-private-broker-fixture -j2` failed while compiling the unchanged `acr329_wake_start_characterization` test target because its test crate cannot resolve `private_provider_probe`, `require_legacy_recipient_effect_route`, and `require_unqualified_legacy_session`. The normal workspace and private Runner checks passed.

## Remaining limits

The private selector remains CLOSED. Source-only user-namespace fixtures do not prove installed host-root/sudo operation, source W/recipient ACK publication, or real provider/auth behavior. The full unfiltered root loop still needs a compatible Bash fixture image. Failed or unknown auth remains blocked with artifact-based caller-owned retry; this candidate does not turn those outcomes into success.
