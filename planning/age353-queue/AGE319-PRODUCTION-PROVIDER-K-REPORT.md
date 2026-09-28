# AGE-319 production provider K: no-effect admission prerequisite

2026-09-28. Runner base `462f34f3ca1b8192bd6f63b2b96c1a78b347f4e1`, source head `5b0fd8c0a0d94c461608b5cb0a86229efffada6b`; Bash base `1aadfe99a5bf4016f29b1ed1198ff74d9173c7f8`, paired head `b7865647f47352eba156b802a2af36b851e97279`. The paired branches are `age319-production-provider-k-20260928`; the Runner branch's report commit follows its source head.

## Result

**No production K or physical Q was opened.** The featureless Runner now requests a durable, no-effect admission after its exact held selection and executable plan. Fresh State stores one immutable admission ID and plan SHA-256 per handoff. Broker opcodes `0x88`/`0x89` use the challenged, same-actor socket and the three existing config/cwd/sealed-environment descriptors. Admission reattests the root/owner/handoff/D/session/actor through the released handoff and held State rows, recomputes the config/account and entire plan, checks the live actor and root admission fence, then writes or reads back the same admission. Runner reconciles a lost admission reply by exact readback. Retry returns the original ID; changed plan, actor, session, root or config is refused. No inherited environment values are stored in State.

The normal root still exits before provider spawn with `normal provider route held`. The private provider K/Q implementation retains its fixture gate. Installed entry/selector, fresh Bash effects, host service/config/DB, and old-v29 debt were untouched. Bash changed only its private stripped Runner SHA pin.

## Verification

All commands ran in the named worktrees. Set `R` to `/home/nes/projects/agent-runner/worktrees/age319-production-provider-k-20260928`, `B` to `/home/nes/projects/agent-bash-tool/worktrees/age319-production-provider-k-20260928`, and `E` to `/home/nes/.local/share/age319-production-provider-k-20260928`. `C` is `/home/nes/.npm-global/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex`; `T` is `$R/src-tauri/target/debug/deps/private_root_join-9ce3a407f5473e5f`. The logs and exact images are frozen in `E`; `SHA256SUMS` verifies them. The existing private Codex auth file was read for F tests but was not copied or changed.
An early format check preceded the final format pass; its failed log is retained as `runner-fmt-check.log` and the passing final check is `runner-fmt-final.log`.

| Directory | Exact command (log) | Result |
| --- | --- | --- |
| R | `CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo build -p oulipoly-agent-runner -p oulipoly-kernel-broker --bin oulipoly-agent-runner --bin oulipoly-kernel-broker -j2` (`featureless-build.log`) | Pass; both images frozen. |
| R | `CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo test -p oulipoly-state --lib disposable_executable_plan_record_is_one_use_and_schema_checked -j2` (`state-admission-final.log`) | 1 pass. |
| R | `CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo test -p oulipoly-kernel-broker --lib no_effect_script_plan_recomputes_exact_sources_and_environment -j2` (`broker-plan.log`) | 1 pass. |
| R | `CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo test -p oulipoly-agent-runner --features age319-closed-fresh --test age319_closed_entry -j2` (`runner-closed-entry.log`) | 1 pass. |
| R | `CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo build -p oulipoly-agent-runner -p oulipoly-kernel-broker --features age319-private-broker-fixture --bin oulipoly-agent-runner --bin oulipoly-kernel-broker -j2` (`private-build.log`); `CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test private_root_join --no-run -j2` (`private-paired-compile.log`) | Pass. |
| B | `CARGO_BUILD_JOBS=2 cargo build --features private-v30-admission --bin agent-bash -j2` (`bash-build.log`); `CARGO_BUILD_JOBS=2 cargo test --features private-v30-admission --test private_v30_source -j2` (`bash-private-source.log`) | Pass; 4 tests. |
| R | `AGE319_PRIVATE_JOIN_ONLY_MODE=normal_model_held OULIPOLY_AGE319_RUNNER_IMAGE=$E/private-runner $T --exact original_runner_joins_once_behind_persistent_root_pid1 --nocapture --test-threads=1` (`held-model-admission.log`) | Pass, 8.45 s; same-key retry, changed plan/identity refusal, State reopen, zero root effects. |
| R | `AGE319_PRIVATE_JOIN_ONLY_MODE=normal_model_provider_native_codex_f_turn AGE319_TEST_CODEX_ELF=$C AGE319_TEST_CODEX_AUTH=/home/nes/.codex2/auth.json OULIPOLY_AGE319_RUNNER_IMAGE=$E/private-runner OULIPOLY_AGE319_BASH_IMAGE=$E/private-bash $T --exact original_runner_joins_once_behind_persistent_root_pid1 --nocapture --test-threads=1` (`real-f.log`) | Pass, 142.34 s. |
| R | `AGE319_PRIVATE_JOIN_ONLY_MODE=normal_model_provider_native_codex_f_turn_old_pending_debt AGE319_TEST_CODEX_ELF=$C AGE319_TEST_CODEX_AUTH=/home/nes/.codex2/auth.json OULIPOLY_AGE319_RUNNER_IMAGE=$E/private-runner OULIPOLY_AGE319_BASH_IMAGE=$E/private-bash $T --exact original_runner_joins_once_behind_persistent_root_pid1 --nocapture --test-threads=1` (`old-pending.log`) | Pass, 135.39 s. |
| R, B | `cargo fmt --all --check`; `git diff --check` | Pass; final logs in `E`. |

## Exact blocker and next boundary

The private K consumes a different descriptor-built plan and file-ledger grant behind its fixture gate; it does not consume the fresh State executable plan or this admission. Removing that gate would leave no production spent-K transaction and no State-bound output/exit/physical-Q/unknown readback. A sound next slice must revalidate the same config/account, cwd, inherited environment, resolved executable, argv and stdin **at physical launch**, durably mark one K consumed before a possible effect, and reconcile output/exit/Q or unknown after reply loss or restart without a second spawn. Scripts need normal pathname execution and a fresh source check at launch; the preflight inode is evidence, not an exact-inode exec promise. No host-root service or sudo was available to test an installed path. Review, owner close, selector cutover, and AGE-353 stress remain root-owned.
