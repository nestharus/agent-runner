# AGE-319 installed caller certificate — 2026-09-28

## Result

The schema-2 root-bound installed `L` normal-model path now keeps the original
launcher process and request alive while it reads exact `l` status. The serving
Broker publishes a create-once, fsynced `installed-normal-terminal-v1` record
only after rereading the immutable L request, the connected control wait and
accepted E, released U/D and invocation, State K and caller settlement, exact
Q and publication, PID1 ECHILD and parent wait, physical root-drain proof, and
the closed-owner proof. The record binds request/pair/source/root/launcher,
control stamp and code, physical proof, and owner proof. Status repeats those
reads and compares them with the record before returning
`exit <code> drained <request-id>`; a missing or changed source remains unknown.
Neither a zero control wait nor process absence can publish an exit alone.

The disposable root-mapped normal-model fixture observed **actual launcher
exit 0**, one provider K/effect, exact Q/output/publication, closed owner, and
the persisted terminal record. The fixture's duplicate L does not launch a
second effect. Removing Q or changing its wait status after certification made
the original pinned launcher receive unknown on its next `l` read and exit 70.
With the caller held at the same point, a Broker restart revalidated the
certificate and delivered exit 0. Offline `--help` still has no K/Q or normal
close proof: its caller exits 70 with explicit unknown and no terminal record.

## Verification and limits

- Root-mapped connected fixture: all five cases passed (normal exit, missing
  Q, changed Q, restart readback, offline help) with feature-enabled binaries.
- `cargo build --locked --no-default-features -p oulipoly-agent-runner -p oulipoly-kernel-broker --bins --quiet` passed (existing warnings).
- `cargo test --locked --no-default-features -p oulipoly-kernel-broker --lib installed_launch_ledger --quiet` passed.
- `cargo fmt --all --check` and `git diff --check` passed.

The launcher has no deadline for a healthy `pending` normal-model run. Loss of
the status socket for 30 seconds returns explicit unknown; a definitively
absent connected wait after restart and changed or missing retained evidence
also return unknown. A clean physical close preflight that never finishes can
still remain pending; crash timing around terminal publication and sustained
AGE-353 stress/benchmark remain untested. The fixture is root-mapped and uses
feature-enabled test transport; no fixed-path host install, host State,
service, aliases, Bash repository, real provider API, or ticket was mutated.

The nine-domain CRW observation and corrective revisit did not run because
this assignment prohibited subagents. This is an explicit review gap for the
parent. The user accepted later stress risk for a fast merge; this branch is
committed and pushed for parent disposition, without a PR or merge here.
