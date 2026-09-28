# AGE-319 owner mailbox fixture correction

2026-09-28 PDT. Runner source head `dcb6254bf0a56bd9ff0d1358e7a89ef20a268f93`; paired Bash source head `7b20932b4eb3f99e84fb33c625b928af04217f38`. These are the tested source commits; this report-only Runner commit follows the Runner source commit. Work stayed in the named Runner and Bash worktrees. No descendants, install, live service/config/DB change, selector cutover, handoff maintenance, PR, merge, or cleanup.

## Correction to the previous diagnostic

The previous report at Runner `bdd55a7e77cc9daec4e9e3989cc5fa1908605300` described `deliverable_mailbox_rows=1` as a blocker in the real Codex F-turn run. That count came from a synthetic `old-pending` row inserted by the private fixture **after** copying the historical v29 sidecar. It was unrelated to the original v2 notification and fresh F notification. The historical copy had zero matching rows. That run therefore did not establish a production mailbox debt.

The normal `normal_model_provider_native_codex_f_turn` mode now omits that insertion. A dedicated `normal_model_provider_native_codex_f_turn_old_pending_debt` mode runs the same paired path with the synthetic row intentionally present. Existing legacy handoff fixture modes keep their prior debt setup. The production owner inventory predicate and all mailbox obligations are unchanged.

| Paired mode | Exact owner inventory after root physical Q and Broker restart | Close readback |
|---|---|---|
| Normal real Codex F-turn | `deliverable_mailbox_rows=0`; no `old-pending` row | `close_eligible=false`, `state_sidecar_outstanding_unknown=true` |
| Historical debt variant | `deliverable_mailbox_rows=1`; exact `old-pending` row still pending | `close_eligible=false`, `state_sidecar_outstanding_unknown=true` |

Both modes prove the original v2 row and distinct fresh F row have their own native assistant ACK and exactly one delivery attempt. The original row joins its native recipient grant, completed turn and assistant response digest, and acknowledged listener. The fresh row joins its separate headless F ACK, second turn and assistant response digest. The first and second turn IDs differ. These exact joins are checked again after Broker restart. The original payload and fresh reserved F payload bytes/digests, one-use grants, wrong-token/turn/K refusals, and lost-reply readbacks remain covered by the paired fixture.

In the normal run, source generation `9d4ec3ca-c278-4b47-9b04-6fc726a51bc9`, root `8cb4da30-6a1e-4a40-b2ef-f7bb177e9313`, owner generation `c3fa8137-6962-44bf-969e-360db3dcb4e3` had zero deliverable rows after restart. State projection/native-channel pending were false; cancelling attempts, registered sources, retiring listeners, unresolved attempts, and native grants without Q were zero. The source effect had `accepted=1`; source/work retirements were `1/1`, outstanding work was zero, and PID1 exact absence plus terminal/ECHILD/parent-wait proof were true.

In the debt run, source generation `aceea30c-4dc5-45ff-b974-7579c8234331`, root `ace5cad5-74fa-43b2-b886-fab8b29c597d`, owner generation `7bd6ec06-cbec-49aa-b2d2-30a00e566128` retained exactly `(source_generation, session_id, seq) = (aceea30c-4dc5-45ff-b974-7579c8234331, old-pending, 1)`. It is kind `fixture`, handle `old-unacked`, inline payload `{}` (SHA-256 `44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a`), pending with zero attempts. It has no listener, source projection, bound State v2 identity, recipient grant, runtime session, wake claim, owner invocation, target or retained payload file. Wrong-recipient lookup finds no row and a changed source generation is refused. The exact diagnostic and full owner inventory persisted across restart. This negative case preserves the obligation and confirms the owner count includes it; no native ACK is allowed to settle it.

## Settlement scope

Fixture isolation is not a production settlement or owner close. Even the normal mode's zero deliverable rows leaves `close_eligible=false`: no joined State writer/retained-sidecar close transaction, fresh-lane admission fence, exact generation/Q/ACK revalidation under that fence, or durable closed-owner phase readback has been implemented. The debt mode has an additional intentionally undeliverable row. Second E, caller publication, selector cutover, and AGE-353 benchmarks remain separate gates.

## Verification

Runner: `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test private_root_join --no-run -j2`; paired Runner/Broker build; stripped Runner image; both real Codex `unshare -Urpfm --mount-proc` fixture modes; `cargo fmt --all --check`; `git diff --check`. Normal mode passed in 130.80 s; debt mode passed in 136.16 s. Logs are `src-tauri/target/age319-owner-mailbox-settlement-normal.log` and `src-tauri/target/age319-owner-mailbox-settlement-debt.log` in the Runner worktree. Bash: `cargo build --features private-v30-admission --bin agent-bash -j2`, `cargo fmt --all --check`, `git diff --check`. Bash only updates its private Runner image pin.

| Paired image | SHA-256 |
|---|---|
| Stripped Runner, matching Bash private pin | `c89af50dc9f4952845eb3f73faab674144dc19cb34c1ceb64ae4c05be7f075ef` |
| Broker | `927b7d1ff3146b0b55799bfe144c22d03e7c0f3275c9c34fa45cd3ae` |
| Paired test executable | `82c10e554cb02cd80581bd4f2c729bd26072488b845212714a5c5b546fc8ea74` |
| Bash | `9df5b0c676f282a0dffe64c10be1097db578a3613add16c7fd33ea656d4bcd61` |

Root owns PR, merge, and cleanup.
