# AGE-319 sleeping successor: exact W obligation prerequisite

## Disposition

The full production sleeping successor wake is **not delivered**. Host-root
successor admission remains closed. This branch records the largest bounded
prerequisite that can be joined to the existing featureless path without
granting an unauthenticated or copied Runner identity delivery authority.

The normal D-bound Runner remains alive when the provider exits before the
held async Bash child reaches selected W. The normal PID1 sees the child's
Broker-owned physical reservation and withholds normal Q until that child
drains. The Runner waits for Q and only then invokes the original recipient
F/receipt/ACK path. Thus the original Runner's pinned Broker socket can
approve a successor after W in this case. If that Runner itself exits, the
current code has no authorized Broker-custodied recovery transition; its D/J
and owner records cannot be treated as permission to impersonate the process.

## Change

On reconciliation of one selected original notify W, State now records one
immutable `fresh_bash_wake_obligation`. It names the original D session and
process identity, one mailbox row, selected source and attempt, lane and source
generation, root and owner generation, and the retained payload hash and
length. The request ID is the selected Bash request ID. A retry can only
recover the same record; conflicting request, row or source data refuses.
Readback rejoins the selected physical W, original D/J/attachment, notify
request, retained mailbox row and accepted source mapping. Broker restart
reconciliation can repair a missing record from the same selected W. The
record is evidence of delivery debt only: source W, receiver receipt, ACK,
logical terminal and physical drain remain distinct. It remains historical
after original ACK; a future scheduler must recheck the current terminal and
both stores before attempting admission.

The default-disabled featureless disposable-root test now lets its initial
provider exit while the Bash child is held before W. It waits for the provider
exit observation, checks normal Q is still absent, and pins the original
Runner's live PID/starttime and installed executable before releasing the
child. It checks the exact obligation against
W, original process identity, received F bytes and row, then reads the same
obligation after Broker restart. The test's original-recipient F/independent
receipt/ACK/terminal/owner-close, reply-loss, duplicate, busy original and sync
controls remain in place.

## Missing production transition

The Broker still needs a durable **choose/start successor** decision keyed to
this obligation. It must either launch or select exactly one distinct installed
Runner, retain its live process identity and authenticated UID, and bind that
process to the original D/J/owner and selected row before cross-store
admission. The receiving Runner then needs a production connected path to
obtain F bytes, persist its own receipt, request Broker certification and a
one-use ACK. If the original D-bound Runner exits before approval, a separate
explicit Broker recovery rule must authorize that approval from retained
evidence. The private `__age319-*` fixture exercises pieces of this join but
does not supply any of those production transitions. This branch does not
enable its private successor protocol in the featureless installed path.

## Verification

- `cargo check --locked -p oulipoly-state -p oulipoly-kernel-broker --no-default-features -q` passed after the initial implementation. The final implementation was rebuilt with `cargo build --locked --no-default-features -p oulipoly-agent-runner -p oulipoly-kernel-broker --bin oulipoly-agent-runner --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher -q`, which passed. Existing unused-code warnings remain.
- Bash was built from the read-only `/home/nes/projects/agent-bash-tool/trunk` with `CARGO_TARGET_DIR` set to this worktree's `src-tauri/target/age319-bash-featureless`. No Bash source was changed.
- `AGE319_FEATURELESS_RUNNER_BIN=$PWD/src-tauri/target/debug/oulipoly-agent-runner AGE319_FEATURELESS_BASH_BIN=$PWD/src-tauri/target/age319-bash-featureless/debug/agent-bash cargo test --locked -p oulipoly-kernel-broker --test age319_featureless_installed --no-default-features -- --nocapture` passed, 1/1, in 181.30 s on final test content, including the exact live original Runner check. The preceding corrected run without that final assertion also passed in 155.11 s.
- The first run of that case failed because the new test inspected the first normal-provider directory, which belonged to an earlier sync root. The second run reached the new State writer and exposed a mistaken equality between the Bash child W session and the original D recipient session. Both were corrected before the passing run. Neither failing run is counted as wake evidence.
- The featureless test proves provider exit before child W and preservation of the **original** Runner generation. It does not contain a distinct production successor generation, successor receipt or successor ACK, and therefore does not satisfy the requested full sleeping-successor acceptance case.
- `cargo test --locked -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_successor_admission -- --nocapture` passed 4/4. This verifies that the new State shape did not break the existing private cross-store successor tests; it is not production wake evidence.
- `cargo fmt --all -- --check` and `git diff --check` passed on final content.

No host installation, host State, service, alias, Bash trunk source, PR or merge was changed. The four-image root is disposable and user-namespace mapped. Its root UID does not prove the real host non-root Runner/root Broker split.
