# AGE-319 retained owner mailbox row diagnostic

2026-09-28 PDT. Runner base `a2e8993aa403ba0615ae16658414b258abbf3f7d`; Bash base `54db9d54e660bbb84a48205dec3284564bb38608`. This work used only the named Runner and Bash worktrees and the requested external report copy. There was no installation, live service/config/DB mutation, selector cutover, handoff maintenance, descendant agent, PR, merge, or cleanup.

## Result

**Blocked: owner close remains false.** The real Codex paired F-turn/dual-ACK fixture passed in 130.87 seconds. Its exact State/retained-sidecar readback proves that `deliverable_mailbox_rows=1` is **not** the original v2 notification or the distinct fresh F row. It is an unrelated synthetic fixture row with no live recipient or ACK authority. The row remains pending with zero attempts after root physical Q and Broker restart. No mailbox disposition or eligibility count was changed.

The new read-only fixture diagnostic enumerates the exact deliverable key, compares it with original v2 custody, checks the bound State v2 identity and retained-sidecar source/listener/grant/session records, and repeats after Broker restart. It refuses a changed source generation and a wrong recipient session. The negative controls passed without changing the row.

## Exact row and recipient evidence

The paired run used retained sidecar source generation `92f76f5b-e78d-4415-a5ef-cc1caea7075d`, root `16964fd9-caa9-43cd-baab-6a3b596a79ca`, and owner generation `bb572359-4f06-48ea-bcdd-72cfa1f86e43`. Its sole deliverable row is **`(source_generation, session_id, seq) = (92f76f5b-e78d-4415-a5ef-cc1caea7075d, old-pending, 1)`**:

| Field | Exact readback |
|---|---|
| Kind / handle | `fixture` / `old-unacked` |
| Source | `INSERT INTO mailbox` in `private_root_join.rs` after the v29 historical DB was copied into the retained Broker sidecar; historical copy has zero matching rows |
| Payload | Inline bytes `{}`; SHA-256 of those bytes `44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a`; no retained payload file, digest or byte-length fields |
| Listener / owner | No `completion_event_listener` for sequence 1; `owner_invocation_uuid=NULL`; no projected `completion_continuation_source` for the handle; no `invocation_completion_v2_identity` in the bound State DB |
| Recipient | Only the literal session key `old-pending`; `target_kind=NULL`, `target_id=NULL`, zero `session_runtime`, zero `session_wake_claim`, zero v2/native recipient grants. There is no live recipient identity in this fixture to read or ACK it. |
| Phase | `delivered_at=NULL`, `delivery_attempts=0`, no ACK or delivery token |

The original accepted v2 notification has a different row sequence and a real listener/session/source grant. Its selected native Codex assistant ACK atomically marked that exact row delivered, acknowledged its listener, and left one delivery attempt; wrong token, turn and selected-K controls refused. The fresh F row lives in `v30/sidecar/pid-identity.db`, has its own grant, source, session, sequence and second-turn ACK, and also has one delivery attempt. Their ACK transactions cannot authorize sequence 1 in the retained sidecar. The paired test verifies the original payload bytes/digest, the fresh F reserved payload bytes/digest, both real assistant receipts and exact ACK readbacks. For sequence 1, a wrong session returns no row and a changed source generation is refused.

After root physical Q and Broker restart, the complete owner inventory was unchanged: State projection/native-channel pending false, zero cancelling native attempts, zero registered sources, zero retiring listeners, zero unresolved attempts, zero native grants without Q, source effect `accepted=1`, and **one deliverable mailbox row**. Physical readback had source/work records and retirements `1/1`, zero outstanding work, `pid1_exact_live=false`, terminal/ECHILD/parent-wait proof true. `close_eligible=false` and `state_sidecar_outstanding_unknown=true` remained true. The exact row diagnostic matched before and after restart.

## Why settlement stops here

Sequence 1 is a separate, unreceived fixture notification. Neither native Codex recipient is its recipient, and the fixture defines no live `old-pending` reader, listener, grant or token. Marking it delivered from either native ACK, or excluding it from the owner inventory, would invent delivery. A future fixture must provide an actual recipient/read/ACK path for this distinct row if it is to be settled; until then owner close must refuse.

Even after a real mailbox settlement, owner close still needs the joined State writer and retained-sidecar writer fence, fresh-lane admission fence, exact generation/Q/ACK revalidation and durable phase-transition readback described in `AGE319-OWNER-CLOSE-REPORT.md`. Second E, caller publication, selector cutover and AGE-353 benchmarks remain separate gates.

## Verification and images

Passed in Runner:

```text
cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test private_root_join --no-run -j2
cargo build -p oulipoly-agent-runner -p oulipoly-kernel-broker --features age319-private-broker-fixture --bin oulipoly-agent-runner --bin oulipoly-kernel-broker -j2
strip --strip-unneeded src-tauri/target/debug/oulipoly-agent-runner
unshare -Urpfm --mount-proc env AGE319_PRIVATE_JOIN_INNER=1 AGE319_PRIVATE_JOIN_MODE=normal_model_provider_native_codex_f_turn AGE319_TEST_CODEX_ELF=/home/nes/.npm-global/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex AGE319_TEST_CODEX_AUTH=/home/nes/.codex2/auth.json OULIPOLY_AGE319_RUNNER_IMAGE=/home/nes/projects/agent-runner/worktrees/AGE-319-owner-mailbox-settlement/src-tauri/target/debug/oulipoly-agent-runner OULIPOLY_AGE319_BASH_IMAGE=/home/nes/projects/agent-bash-tool/worktrees/AGE-319-owner-mailbox-settlement/target/debug/agent-bash src-tauri/target/debug/deps/private_root_join-9ce3a407f5473e5f --exact original_runner_joins_once_behind_persistent_root_pid1 --nocapture --test-threads=1
cargo fmt --all --check
git diff --check
git diff --cached --check
```

Passed in Bash: `cargo build --features private-v30-admission --bin agent-bash -j2`, `cargo fmt --all --check`, `git diff --check`, `git diff --cached --check`. Bash's only source edit updates the private Runner image pin. The paired fixture log is `src-tauri/target/age319-owner-mailbox-settlement-paired.log` in the Runner worktree.

| Image | SHA-256 |
|---|---|
| Stripped Runner, matching Bash private pin | `cfad362761e33c1ab83471c05bd39064eb2883829038d5b5dc41464c3589ec4c` |
| Broker | `927b7d1ff3146b0b55799bfe144c22d03fe21ad03e7c0f3275c9c34fa45cd3ae` |
| Paired test executable | `180398cb7ff4ea340a612b0accda5144038bb21368d29b799eac28061e231884` |
| Bash | `5b94218973382a73d1931f93b4b54dbb7369c59af66daa5d71c9f133d06dd596` |
| Real Codex ELF | `167c0148a849d2444f1b5a7fb5f8bb2de1de5ae13a2a504b833fc765980f5cd9` |

Runner source commit `ec191d8058d7dd1a80e7308ad5c4d42ad6ddf8b2`; Bash paired pin commit `f16d44f7edb75a0501674b750352fc9a54887d85`. Root owns PR, merge and cleanup.
