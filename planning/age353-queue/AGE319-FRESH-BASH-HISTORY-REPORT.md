# AGE-319 fresh Bash closed-history slice — 2026-09-29

## Disposition

The featureless disposable-root connected path now crosses the fresh Bash
`0x90` parent probe, executes one physical Bash child, accepts its selected
tree-drained source, publishes the sync result, reaches an installed drained
terminal, and admits a later root after Broker restart. **Host-root installed
CLI admission remains closed.** No host installation, State, service, alias,
Bash checkout, or trunk file was changed.

## Implementation

The fresh Bash parent reader reopens the independent root, work, entry, grant,
source, sidecar, State, and installed launch records under the activated pair
and bootstrap source generations. It identifies exactly one live unfenced root
and validates every other entry with the existing prior-entry close join. That
join checks the exact immutable root fence, PID1 terminal/ECHILD/parent wait,
retired work and source obligations, State caller result and close cursor,
closed owner proof, and the installed terminal certificate. The fresh ledger
open is read-only and refuses absent storage. Normal entries now require their
installed certificate as well as the State publication; offline
entries retain their certificate digest marker. The fresh readback checks that
marker without writing it. Missing, stale, or mismatched evidence remains
uncertain. A fenced root cannot supply a Bash parent, and fresh admissions
still check the shared in-process fence after classification.

The fresh lane receives the installed pair generation and the **bootstrap**
source generation from activation. The v30 lane's own source generation is a
different identity and cannot open the installed request ledger. The old
Broker retains its root/owner authority; the v30 lane only reads these prior
closed records. No v2/v29 source was used to authorize v30 work.

## Connected evidence

`age319_featureless_installed` used the current default-disabled Broker,
launcher, and Runner and the existing featureless Bash image in a disposable
root-mapped user/PID namespace. It preserved the direct Runner, copied FD,
unsupported shape, stale pair, help lost-L-reply/duplicate, and ordinary
provider controls. After the closed help and normal roots, the real Bash sync
request crossed `0x90`; the Bash client deliberately lost C, K, Q, W, and
sync-begin replies and recovered through exact readbacks. One `bash` physical
effect was observed after the earlier `one` provider effect. The State selected
Bash source event was unique, tree-drained, output-closed, and bound to the
terminal root; one sync publication existed. The installed terminal had exit
code zero, PID1 ECHILD and parent-wait proof, and fully retired work. Broker
was then killed and restarted; a fourth root admitted and produced only its
one `later` effect and its own drained terminal. The test observed four entry
records.

The feature-gated private connected suite passed all 13 cases with a
feature-enabled Runner image, including real Bash sync, async recipient,
async successor and ACK, certificate tamper, and restart controls. The first
attempt used a featureless Runner image and failed at the private pause hook;
it is not counted as a product failure or passing evidence. A separate
featureless async recipient/successor connected case is still outstanding.

## Commands

- `cargo build --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher -p oulipoly-agent-runner --bin oulipoly-agent-runner --no-default-features` — passed.
- `AGE319_FEATURELESS_RUNNER_BIN=$PWD/src-tauri/target/debug/oulipoly-agent-runner AGE319_FEATURELESS_BASH_BIN=/tmp/age319-bash-target/debug/agent-bash cargo test --locked -p oulipoly-kernel-broker --test age319_featureless_installed --no-default-features -- --nocapture` — passed, one connected case after the restart and reply-loss additions.
- `cargo build --locked -p oulipoly-agent-runner --bin oulipoly-agent-runner --features age319-private-broker-fixture --no-default-features` — passed for private connected tests; the featureless Runner was rebuilt afterward.
- `AGE319_CONNECTED_RUNNER_BIN=$PWD/src-tauri/target/debug/oulipoly-agent-runner AGE319_CONNECTED_BASH_BIN=/tmp/age319-bash-target/debug/agent-bash cargo test --locked -p oulipoly-kernel-broker --test age319_connected_control --features age319-private-broker-fixture --no-default-features -- --test-threads=1 --nocapture` — passed, 13 cases.
- The real Bash sync case from that suite passed again after the final read-only ledger change.
- `cargo check --locked --workspace --no-default-features` — passed.
- `cargo test --locked -p oulipoly-kernel-broker --lib installed_launch_ledger --no-default-features` — passed, six tests, including missing-storage readback.
- `cargo fmt --all --check` and `git diff --check` — passed.

Existing unused-code warnings remain. The Bash image was the prebuilt
featureless binary from `/tmp/age319-bash-target`; this slice did not rebuild
or modify the Bash checkout.

## Remaining dependencies

Before host-root admission opens, prove the real host non-root launcher versus
root Broker UID boundary, fixed installed images and socket, and the complete
host L-to-drain chain. A featureless async Bash recipient/successor case with
busy and sleeping recipient controls remains to be run. The host-root `L`
guard stays closed; parent owns PR, merge, and cleanup.
