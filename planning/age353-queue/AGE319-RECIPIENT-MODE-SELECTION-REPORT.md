# AGE-319 production async recipient mode selection

## Implementation

The installed original Runner now selects the recipient from the immutable
selected Bash source event. A normal provider wait recorded before Broker
source capture selects sleeping and the one Broker-launched successor. A
matching live provider process, resolved and pinned by the host-namespace
Broker at source capture, selects busy and the original F/receipt/ACK path.
Missing, malformed, or racing evidence selects neither route. The disposable
`successor-mode` file is gone; lost-start-reply injection remains gated by the
private fixture flag.

Normal provider PID1 attempts a create-new, synced start receipt with local
PID, host PID, and start time after spawn. It writes a synced wait receipt
after `waitpid`. The wait receipt is required for normal Q. On a write failure,
PID1 still waits for every reserved Bash child to drain
before returning unknown. Broker freezes the normal lifecycle observation in
the create-new source event before State selects W. State checks that event
against the root-owned normal parent and start/wait receipts on selection and
readback. A later provider exit cannot change the event's selected mode.
Terminal readback exposes the verified selected child event even before the
parent Q permits an immutable root execution terminal, so a busy original
can read W while its provider is still active.

The original Runner reads the existing selected-W successor decision before
choosing an offer ID. On retry it uses that exact offer and checks its W,
root, session, and row; a new UUID is minted only when no decision exists.
The Broker/State D-bound admission and one-use start gates remain the delivery
authority.

## Proof

The disposable test uses no mode marker. It holds the provider live through
the busy W, checks the host PID/start-time start receipt against the selected
event's pinned identity, and confirms that State exposes W before parent Q.
For sleeping, it waits for normal PID1's wait receipt before releasing the
Bash child and checks that the selected event contains the same wait evidence.
It also drops the successor start reply, checks the distinct candidate and
one start, and reopens Broker for readback.

From this worktree, with no host install or host State changes:

```bash
CARGO_TARGET_DIR="$PWD/src-tauri/target/age319-bash" cargo build --locked --no-default-features --manifest-path /home/nes/projects/agent-bash-tool/trunk/Cargo.toml --bin agent-bash -q
cargo build --locked --no-default-features -p oulipoly-agent-runner --bin oulipoly-agent-runner -q
cargo test --locked --no-default-features -p oulipoly-kernel-broker --lib missing_or_damaged_wait_never_claims_sleeping
AGE319_FEATURELESS_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" AGE319_FEATURELESS_BASH_BIN="$PWD/src-tauri/target/age319-bash/debug/agent-bash" cargo test --locked --no-default-features -p oulipoly-kernel-broker --test age319_featureless_installed -- --exact disposable_root_featureless_l_help_lost_reply_and_duplicate --nocapture
cargo fmt --all -- --check
git diff --check
```

All passed on the final source. The focused unit test passed 1/1. The
featureless four-image test passed 1/1 overall: busy original 134.66 seconds,
sleeping distinct successor 132.46 seconds, 267.14 seconds overall. Its Bash
image SHA-256 was `741a1975fbe7fa666aaac5195cc81697ae626df4bfe5a4899b54a8b7decdc54e`.
The test verifies exact State W and receipt evidence, one successor decision
and start, a distinct candidate, successor F/receipt/cert/ACK, dropped start
reply recovery, and Broker restart readback. Existing workspace compiler
warnings remain.

Exploratory runs found and corrected two boundary mistakes: normal PID1
cannot pin a host PID from its nested PID namespace, and root terminal
execution is absent while the busy provider still holds parent Q. The final
source records the host PID/start time via host proc and exposes verified
selected child W separately from root execution.

## Limits

The mode is a snapshot at Broker source-event capture, the physical selection
point preceding State W insertion. If the provider dies during observation or
the required start or wait witness is missing or damaged, the result stays unknown. A
captured event remains authoritative across delayed State insertion and
restart. This slice does not open host-root L admission or authorize a new
candidate if the original D-bound Runner exits before approval. A Broker
restart can read a spent successor start but cannot recreate its live
candidate or grant delivery authority from the ledger alone.
