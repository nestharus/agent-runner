# AGE-319 featureless installed async recipient — 2026-09-29

## Disposition

The default-disabled installed Runner now receives a real async Bash child's
selected W payload on the original D-bound root while that Runner is busy.
The original Runner persists an exact F request ID before submission, decodes
and validates the bytes actually returned by Broker F, writes and fsyncs its
own exclusive `0400` receiver receipt, spends the one-use F token, and checks
the exact ACK and root terminal. The receipt is in the Runner's data directory,
outside Broker State. Lost F and ACK replies use exact request-ID recovery and
readback. The installed test exercises these paths in a disposable root-mapped
namespace with featureless Runner, Broker, launcher and real Bash images.

The sleeping-recipient successor path is **not proved or enabled**. Host-root
admission remains closed. This increment does not restore the host installed
pair or run AGE-353 busy/sleeping benchmarks.

## Production change and boundary

After normal provider Q drains, the installed Runner reads the original root's
terminal. A `notify` listener with selected W advances through terminal repair
to `pending_f`; the Runner submits F with its durable request ID. The received
payload must match terminal row, root, owner, session, source, attempt, lane,
generation, digest and length, and its child event must carry drained/closed W
and matching stdout/stderr hashes. The Runner fsyncs the receipt and directory
before ACK. ACK readback and terminal must name the same grant. Failure leaves
the root unsettled rather than claiming a caller result or close. Ordinary
non-notify and sync roots take their prior path.

The original F protocol's existing Broker ACK is a pinned-peer one-use manual
ACK. It does **not** independently certify the Runner's receipt file or bind
that file into the installed terminal certificate. The receipt is independent
and retained for review, but loss or tampering after ACK would not invalidate
the current manual-ACK terminal. That stronger cross-store receipt join is a
remaining production gate for broad deployment.

The normal provider in the connected fixture remains live until the selected
W exists. It serves only as a namespace lifetime barrier. Its output is not
used as F receipt or ACK authority. Early provider exit before child W remains
unproved, as found in the earlier connected async report.

## Observed connected result

The extended `age319_featureless_installed` case retained direct Runner,
copied FD, unsupported shape, stale pair, help lost-L-reply/duplicate, normal
provider, real Bash sync, Broker restart and later-root controls. It additionally
ran one real async Bash child. The provider was observed still live when Bash
selected W was present. One physical `async` effect appeared; the selected W
was tree-drained/output-closed; its raw stdout/stderr bytes and full source
event matched the independent receiver receipt made from F bytes. The sidecar
held one `manual_ack` with one mailbox delivery attempt. The async root had a
zero-exit installed terminal with PID1 ECHILD, parent wait, and retired work.
After Broker restart, another root admitted and produced only its own `later`
effect. The test observed five entry records in total.

The second successful run deliberately discarded both F and ACK replies in the
featureless Runner and recovered through exact F and ACK readbacks. The final
run also rejected reuse of the same ACK token from the original pinned process
and read the identical installed terminal after Broker restart.

## Commands and results

- `cargo check --locked -p oulipoly-agent-runner --no-default-features` — passed.
- `cargo build --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher -p oulipoly-agent-runner --bin oulipoly-agent-runner --no-default-features` — passed.
- From the read-only Bash trunk, with target output inside this worktree: `CARGO_TARGET_DIR=/home/nes/projects/agent-runner/worktrees/age319-featureless-async-20260929/src-tauri/target/age319-bash-featureless cargo build --locked --bin agent-bash` — passed.
- `AGE319_FEATURELESS_RUNNER_BIN=$PWD/src-tauri/target/debug/oulipoly-agent-runner AGE319_FEATURELESS_BASH_BIN=$PWD/src-tauri/target/age319-bash-featureless/debug/agent-bash cargo test --locked -p oulipoly-kernel-broker --test age319_featureless_installed --no-default-features -- --nocapture` — passed once with ordinary replies (161.94 s), then with dropped F and ACK replies (168.42 s), then with those reply losses plus one-use replay and post-restart terminal equality (157.06 s).
- `cargo check --locked -p oulipoly-agent-runner --features age319-private-broker-fixture --no-default-features -q` — passed.
- `cargo test --locked -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_fresh_recipient_socket private_fresh_recipient_delivery_ack_collision_and_restart -- --exact --nocapture` — passed, including its pinned-recipient negative child controls.
- `cargo fmt --all --check` and `git diff --check` — passed.

After the private compatibility check, the featureless installed images were
rebuilt in this worktree. Final `sha256sum` values are Runner
`f3a343b3bd638032dcbadd3a385964889e14fb341a77dd9c8f77027bded38fe3`,
Broker `e129b6c5e14e7e26cce610d6657aea352864299f73e7a7e55f9e87e9233a9e97`,
launcher `57056319763191602a02b87c1877e3ebc4c692ece13331755b6d69de402a2b71`,
and Bash `741a1975fbe7fa666aaac5195cc81697ae626df4bfe5a4899b54a8b7decdc54e`.
The full private connected suite was not rerun; the private Runner compiled and
the focused original-recipient socket regression passed.

The builds emitted existing unused-code warnings. No Bash trunk file, host
installation, host State, service, alias, PR, merge, or extra worktree was
changed.

## Sleeping successor and AGE-353 blocker

The private connected successor uses a deliberately launched installed Runner
process and a live original D-bound Runner peer to approve its offer. The
featureless production path has no durable obligation that selects and starts
an authenticated successor for one pending row after the recipient disappears,
and the existing `AdmitSuccessor` call requires that live original pinned peer.
Promoting the private `__age319-*` entry would bypass that missing scheduler
and recovery contract. The sleeping case, its exact restart recovery, and the
AGE-353 busy/sleeping benchmarks therefore remain closed. A real non-root
caller/root Broker test and all production proof are still required before
host-root admission can open.
