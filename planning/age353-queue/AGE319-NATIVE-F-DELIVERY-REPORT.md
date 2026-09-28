# AGE-319 second selected-K native turn for fresh F

2026-09-27, private paired Runner/Bash source. No installation, selector cutover, live service/config/DB mutation, handoff update, descendant agent, PR, merge, or cleanup.

## Result

The real-Codex paired `normal_model_provider_native_codex_f_turn` mode passed in 104.31 seconds. It retained the exact selected-K app-server control after the original v2 native assistant ACK, accepted the distinct Bash H/W/physical Q and fresh F grant, reserved one F-specific attempt before sending, and submitted one second `turn/start` on that same K thread. A fresh provider-owned rollout anchor followed the completed first turn. The F receipt required an exact post-anchor user input envelope and one assistant-authored `AGE319_F_ACK <F token>` response in the completed second turn. The test checked a different turn ID from the original, selected K, F grant/source/session/row/payload, new nonce/envelope digest, and the durable attempt's exact payload bytes. The F submit reply was deliberately dropped; the Runner obtained the receipt by readback. A duplicate second submit was refused without replay.

The original v2 row stayed explicitly ACKed with one delivery attempt. **Fresh F remains `submitted` with zero delivery attempts and terminal `f_submitted_native_pending`.** This increment does not claim an F ACK or root close. Its broker receipt is native Codex evidence, but it is a private immutable artifact, not yet the fresh lane's transactional ACK evidence. No F socket submission was counted as a native receipt, and the resident PTY preparation/adapter and `manual_ack` paths were not used.

## Source

- Runner `fresh_provider.rs` now returns the selected-K control from the successful first ACK worker. Its second-turn path reattests `FreshV30Lane::attest_native_k_f_candidate`, the original ACK receipt and current K, anchors the same native rollout after the first completed turn, and creates one synced F attempt containing the exact grant, token, request, source, row, payload, nonce and envelope before `turn/start`. The worker reattests before send and after native proof. Failure or restart has no resend path.
- Runner broker/client use distinct private `>` submission and `_` readback operations, including a larger native response bound. The F request ID comes from the original Runner's fresh F report and is checked against accepted W in State. The F worker reports an exact native second-turn receipt while leaving fresh F pending.
- Runner integration and unit tests cover the real selected-K second turn, dropped submission reply, duplicate submission, wrong F token/W request/K candidate, stale first anchor, wrong turn ID, wrong assistant token and duplicate assistant action.
- Bash checks the accepted W event's attempt, lane, generation, session and root against its child in both ordinary and private source paths. Its private fixture Runner SHA pin was updated to the exact stripped source image.

## Verification

Passed from the Runner worktree:

```text
CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo check -p oulipoly-kernel-broker -p oulipoly-agent-runner --features age319-private-broker-fixture -j2
CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo build -p oulipoly-agent-runner -p oulipoly-kernel-broker --features age319-private-broker-fixture --bin oulipoly-agent-runner --bin oulipoly-kernel-broker -j2
CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test private_root_join --no-run -j2
CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --bin oulipoly-kernel-broker fresh_f_action_requires_new_anchor_second_turn_and_own_token -- --nocapture
AGE319_TEST_BIN=src-tauri/target/debug/deps/private_root_join-9ce3a407f5473e5f
AGE319_RUNNER_IMAGE=/home/nes/projects/agent-runner/worktrees/AGE-319-native-f-delivery/src-tauri/target/debug/oulipoly-agent-runner
AGE319_BASH_IMAGE=/home/nes/projects/agent-bash-tool/worktrees/AGE-319-native-f-delivery/target/debug/agent-bash
AGE319_FIXTURE_IMAGE=/home/nes/projects/agent-runner/worktrees/AGE-319-native-f-delivery/src-tauri/target/debug/age319-fresh-provider-fixture
AGE319_CODEX_ELF=/home/nes/.npm-global/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex
AGE319_PRIVATE_JOIN_ONLY_MODE=normal_model_provider_native_codex_f_turn AGE319_TEST_CODEX_ELF="$AGE319_CODEX_ELF" AGE319_TEST_CODEX_AUTH=/home/nes/.codex2/auth.json OULIPOLY_AGE319_RUNNER_IMAGE="$AGE319_RUNNER_IMAGE" OULIPOLY_AGE319_BASH_IMAGE="$AGE319_BASH_IMAGE" "$AGE319_TEST_BIN" --exact original_runner_joins_once_behind_persistent_root_pid1 --nocapture --test-threads=1
AGE319_PRIVATE_JOIN_ONLY_MODE=normal_model_provider_bash_causal OULIPOLY_AGE319_PROVIDER_IMAGE="$AGE319_FIXTURE_IMAGE" OULIPOLY_AGE319_RUNNER_IMAGE="$AGE319_RUNNER_IMAGE" OULIPOLY_AGE319_BASH_IMAGE="$AGE319_BASH_IMAGE" "$AGE319_TEST_BIN" --exact original_runner_joins_once_behind_persistent_root_pid1 --nocapture --test-threads=1
AGE319_PRIVATE_JOIN_ONLY_MODE=normal_model_provider_bash_ordinary_sync_parent_output OULIPOLY_AGE319_PROVIDER_IMAGE="$AGE319_FIXTURE_IMAGE" OULIPOLY_AGE319_RUNNER_IMAGE="$AGE319_RUNNER_IMAGE" OULIPOLY_AGE319_BASH_IMAGE="$AGE319_BASH_IMAGE" "$AGE319_TEST_BIN" --exact original_runner_joins_once_behind_persistent_root_pid1 --nocapture --test-threads=1
cargo fmt --all --check
git diff --check
```

Passed from Bash: `CARGO_BUILD_JOBS=2 cargo build --features private-v30-admission --bin agent-bash -j2`, `cargo test --features private-v30-admission --test private_v30_source -- --nocapture` (4 tests), `cargo fmt --all --check`, and `git diff --check`.

The test sequence also found and corrected: the stale paired Runner pin; a transient Bash image pathname replacement while a build ran concurrently with the test; collisions with existing broker opcodes; an F request ID read from the wrong process environment; the first-turn-only rollout anchor rejecting a legitimate completed first turn; and the ordinary 256-byte response reader rejecting native F readback. The final paired run used stable images and passed.

| Image | SHA-256 |
|---|---|
| Stripped private Runner | `432154f3b04951ac2314d5864b69c915c3eead24de8d0fd362d90f25139697e0` |
| Private Broker | `9a873f782664f85d812295f79fd206e711e0a580d64c27259d2d4d3b132f3a70` |
| Paired Bash | `bfb950133841f8fe9912194d343f173229c76e34f253fe46e8fd1bc8ae461502` |
| Paired test executable | `a9d9f2661c8aef791a6a89156c58ee0e5507bc62e7faac3dd58aef9f516c975e` |
| Fixture provider | `7622d338f7f771d9fd8b9fee01d4af06eaca6c663062637e0a761ea7c58d7c1a` |
| Real Codex ELF | `1a822376d4634ac32dddc030e5117c63359f7f8cd4b1b64382c68190287d0258` |

## Remaining exact gap

Fresh State needs a separate durable headless-Codex F receipt/action record and one transactional ACK operation bound to this exact F attempt, second turn, assistant token, selected K and fresh grant/source/row/payload. That operation must ACK only F, read back the exact settlement after a lost reply, preserve the original v2 ACK, and let terminal F become `acked`. The existing `fresh_native_f_preparation`/transport/receipt tables describe resident PTY adapter evidence and cannot be reused as authority for this headless Codex turn. A restart after this increment's send or proof still leaves F pending/unknown with no resend.

## Source heads

Runner entry `0f973268ef73b229bc413caca595a95a0b0f0c83`; pushed source branch `age-319-native-f-delivery` at `99a33dacaf44ce9aba2beb5ec375f1a5f1e5cd4d`. Bash entry `3dd80edf46e176f8c0f79c3c671e126f90a04e19`; pushed paired source branch at `1b85f6953346ea64c06995611dbc4266ffcfb70b`. Both remote source heads matched local readback. The report itself is committed on the Runner branch after these source commits. Root owns PR, merge and cleanup.
