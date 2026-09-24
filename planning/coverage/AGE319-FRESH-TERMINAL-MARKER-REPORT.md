# AGE-319 fresh terminal marker candidate

Source base: `18d4d767`. Scope: private fresh broker route only. The selector remains closed; no installed broker, host sudo, provider traffic, other worktree, or old State/WAL mutation was used.

## Change

- The broker freezes the normal runtime terminal recognizer identity with each independently verified config candidate. After an exact provider K and verified physical Q, route history reclassifies the original stdout, stderr, and wait status through that same recognizer and durably records the terminal result, selected model/account/config/plan, invocation binding, grant ID, and Q digest. A missing or unknown Q does not create a terminal record and still prevents route selection.
- Confirmed inband quota and tentative quota block the account until a healthy physical quota Q for that same account/model/config is newer than the rejected provider Q. Cross-root quota reuse resolves to its physical source Q; a reused healthy receipt from before the rejection is not verification. The account effect admission path materializes prior terminal Q before considering reuse.
- Typed availability, rate limit, and storage contention create distinct markers with explicit 5-minute, 1-minute, and 2-minute recovery intervals, respectively. A newer healthy quota Q can also verify those markers. Auth rejection from the runtime's non-quota auth diagnosis is separate and requires a newer successful auth-refresh Q. Generic nonzero exits and untyped unknown results do not become quota markers. A cancellation used to drain descendants does not erase a typed provider rejection.
- Eligibility checks every unresolved marker, so a later short availability event cannot hide an earlier quota rejection. Explicit pins cannot bypass a marker. An already held route is checked again before one-use K; its old decision does not authorize an account rejected after selection. A rejected held choice remains a durable choice and requires a new linked attempt for a different route.
- The ledger is append-only in `v30/fresh-provider`; marker records are not erased. Each choice recomputes release from current verified effect evidence or the typed recovery interval. Provider K without Q remains live/unknown without a lifetime cutoff. No old State or WAL path was added.

## Evidence

- `cargo fmt --all -- --check`, `git diff --check`, and `CARGO_TARGET_DIR=src-tauri/target CARGO_BUILD_JOBS=2 cargo check -p oulipoly-agent-runner --features age319-private-broker-fixture -j2`: pass.
- Focused broker tests: 10 pass. New cases cover typed zero-exit quota, generic nonzero, auth, cancellation, structured storage contention, old/new quota Q ordering, multi-marker interaction, exact ledger restart/readback, lost Q with no invented marker, two-account fallback, pin refusal, held choice K refusal, and old WAL sentinel isolation.
- The real private user-namespace provider fixture passed for typed quota Q and the pre-existing long K/physical Q test with `OULIPOLY_AGE319_PROVIDER_IMAGE=src-tauri/target/debug/age319-fresh-provider-fixture`.
- Private Runner root modes passed individually: `normal_model_provider_quota_available`, `normal_model_provider_quota_restart`, `normal_model_provider_no_pin`, `normal_model_provider_reply_loss`, `normal_model_provider_quota_reply_loss`, `normal_model_provider_quota`, and `normal_model_provider_auth`. These are private fixture results, not installed host proof.

## Integration limits

- Auth markers are deliberately conservative. The current private Runner starts auth refresh after a failed/empty/invalid first quota effect. If a quota source still reports healthy while provider work reports auth rejection, the marker remains blocked until an explicit new auth-refresh Q is available; this branch does not add a new auth orchestration request.
- Existing durable private route-candidate JSON from an older broker image lacks the new recognizer field and fails closed on read. An integration that must resume such private history needs a versioned compatibility or migration decision; no old history is silently reclassified.
- The classifier is the current bounded in-tree provider recognizer plus the existing non-quota auth diagnosis. External provider text outside those contracts may remain untyped. No new token inference from exit status was added.
- The exact supported model shapes, root-authorized config path, broader routing policy, installed host sudo and publication gates remain separate root integration obligations. This patch does not open the selector or claim full product restoration.
