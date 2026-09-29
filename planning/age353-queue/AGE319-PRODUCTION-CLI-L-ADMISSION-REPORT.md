# AGE-319 production installed CLI `L` admission — 2026-09-29

Base: `1221234ff4dd7919eb7571e76b30842c2a6a2b97`.

## Disposition

**Production `L` remains closed.** Removing its explicit guard exposed an
unfinished connected completion path in the requested featureless four-image
proof. The guard is retained before ledger reservation and root creation. No
host installation, service, package, or host State mutation was attempted.

This branch delivers the independently useful prerequisite: State's root
terminal reader now recognizes an exact normal CLI K consumed in State and
verifies its physical Q, PID1 wait, and output bytes in the Broker-owned
`v30/normal-provider/<admission ID>` directory. Previously it always required
an old `v30/fresh-provider/<handoff ID>.fresh-grant.json` even for the new
normal K path. That readback returned `parent_k_q:physical ...fresh-grant.json
lost` after async Bash W, preventing terminal completion. The new path rejects
an ambiguous old/new K and keeps missing or changed evidence unknown. The
old grant path remains in use for its existing callers.

`l` is already the exact, authenticated status route used by the installed
launcher; it has no comparable production refusal after fresh activation.
`M` is cancellation and remains production-closed; normal launcher completion
does not call it. Image, pair, generation, source, request, peer, descriptor,
and ledger checks in `L` were not relaxed.

## Proof and failed full admission

The focused bridge mode uses the actual featureless Runner, Broker, launcher,
and Bash copied into a disposable schema-2 pair with an activated fresh source.
It reaches a busy original, selected async Bash W, releases the held parent,
and requires State to settle a successful parent terminal from the new normal
K/Q path while the old grant file is absent. It also checks exact refusal
messages for a copied launcher image, stale pair generation, and unsupported
CLI syntax before any installed request record exists. Fixture-only reply-loss
controls remain in the full test.

The unrestricted four-image test was run repeatedly on the candidate that
removed the production guard. Before the State repair, the original test and
the UID-mapped variant both timed out after selected W with the missing old
grant error. After the repair, one run reached an exact async terminal and
manual ACK but did not reach caller completion; another reached that chain
then timed out on the later launcher; another timed out on the ordinary model.
One Broker log reported `fresh mailbox and State session admissions differ or
D is incomplete` during owner-close progression. That log is an observed lead,
not a proven cause of every timeout. No complete busy-and-sleeping final-code
proof was obtained, so opening host `L` would be premature.

The unrestricted test was also run on the final guard-retained code. Its busy
case hit the ordinary normal launcher's 30-second timeout; the test finished
in 91.86 seconds without reaching the sleeping-successor case. Fixture
admission bypasses the production guard, so this is a connected completion
failure independent of the guard.

An attempted mapped UID 1001 launcher/root Broker fixture was removed from
the committed test because the full chain failed before a trustworthy
UID-split terminal could be shown. The remaining host boundary is therefore
unproved. The focused test uses mapped UID 0 in a disposable namespace.

| Command | Result |
|---|---|
| `cargo build --locked --no-default-features -p oulipoly-agent-runner --bin oulipoly-agent-runner -p oulipoly-kernel-broker --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher` | Passed on final guard-retained code, with existing unused-code warnings. |
| `CARGO_TARGET_DIR=$PWD/src-tauri/target/age319-bash-featureless cargo build --locked --no-default-features --bin agent-bash` from the Bash checkout | Passed; output was written only to this worktree's target. |
| `AGE319_FEATURELESS_BRIDGE_ONLY_V1=1 AGE319_FEATURELESS_RUNNER_BIN=$PWD/src-tauri/target/debug/oulipoly-agent-runner AGE319_FEATURELESS_BASH_BIN=$PWD/src-tauri/target/age319-bash-featureless/debug/agent-bash cargo test --locked --quiet --no-default-features -p oulipoly-kernel-broker --test age319_featureless_installed -- --exact disposable_root_featureless_l_help_lost_reply_and_duplicate --nocapture` | Passed: 1/1, 110.50 s, after the guard was restored. |
| The same test without `AGE319_FEATURELESS_BRIDGE_ONLY_V1` | Failed repeatedly on the guard-removed candidate; the final guard-retained run also failed in the busy case after 91.86 s at ordinary normal launcher completion. No full positive proof. |
| `cargo fmt --all --check` and `git diff --check` | Passed. |

## Next dependency and host limits

The next slice must identify and repair the connected normal caller
publication/owner-close/status progression after async F/ACK, then pass both
busy-original and sleeping-successor four-image cases on one final head.
Only after that should production `L` be reconsidered. A separate fixed-path
host check must cover the installed image bytes, activated root State,
`/run/oulipoly-kernel-broker/control.sock` group access, and an actual
non-root launcher/root Broker UID split. GUI, PTY entry, and production `M`
cancel remain outside this CLI admission slice.
