# AGE-319 normal provider lifetime for async Bash — 2026-09-28

## Result

The connected async fixture no longer holds its normal provider until child W.
It now holds the *child* workload until the normal PID1 has recorded the
provider's exit, asserts normal Q is absent while that child is live, releases
the workload, and verifies the existing Bash Q/W, selected F, independent
receipt, durable ACK, root terminal, and owner close assertions. The focused
case and all 10 connected cases pass serially. A two-thread run passed 9/10;
the existing sync case again timed out at owner close with
`state_sidecar_outstanding_unknown=true`. Its normal physical Q and caller
publication were settled. The same concurrent sync symptom was recorded in
`AGE319-CONNECTED-ASYNC-BASH-REPORT.md`; a subsequent isolated sync rerun
passed. This is a residual concurrency failure, not a green parallel suite.

## Cause and correction

`normal_physical.rs` previously used normal namespace PID1's `waitpid(-1)`
until `ECHILD`, then published `tree_drained=true`. `fresh_provider.rs` forks
the Bash worker from the Broker after `setns` into the normal provider's PID
namespace. Namespace membership does not make that worker PID1's child. The
provider and its own adopted descendants can therefore exit, causing
`ECHILD`, while the Broker-owned child PID1 is still running. Exiting normal
PID1 then kills that nested work before its physical Q, source W, and recipient
delivery. The prior provider hold masked this gap.

The Broker now writes a root-only reservation for the exact child physical
grant under the normal admission before consuming the child's K. It holds an
exclusive lock on that admission directory through the durable K write. Normal
PID1 takes the same lock after its ordinary provider wait and checks every
reserved child before authoring Q. A reservation with no consumed K is inert;
a consumed K stays pending until the matching child attach, drain, PID1 wait,
and exited PID1 incarnation prove physical completion. The lock closes the
race between provider exit and child K: PID1 either sees the durable grant or
publishes Q before the Broker can reserve, in which case the Broker refuses a
new child. The normal provider's exit status and output remain its own Q.

There is no child lifetime deadline. A spent child K with missing or damaged
physical proof keeps the normal Q unknown and the namespace alive; it cannot
be retried as another K. Child source W, recipient F/receipt, and ACK remain
separate from the parent's physical drain decision. A best-effort
`provider-exit.json` observation lets the fixture establish the order without
making its write failure terminate PID1 while an attached child may be live.

## Exact verification

Working branch `age319-async-parent-lifetime-20260928` started at main
`77da6ffb584f9025bd11c23efb78e174d8dc52d0`. Bash trunk was read-only;
the prebuilt featureless image at
`/tmp/age319-connected-async-bash-target/debug/agent-bash` has SHA-256
`741a1975fbe7fa666aaac5195cc81697ae626df4bfe5a4899b54a8b7decdc54e`.
Tests copy images into disposable schema-2 installed directories under private
root-mapped namespaces; no host install, State, alias, service, real provider
API, or unrelated process was used.

From this worktree:

```bash
cargo build --locked --no-default-features -p oulipoly-agent-runner --features age319-private-broker-fixture --bin oulipoly-agent-runner
```

Passed. The first focused test invocation failed to compile because
`admission_id` was moved into `parent_from_normal`; the code now uses the
verified returned parent stamp. With that corrected:

```bash
AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" AGE319_CONNECTED_BASH_BIN=/tmp/age319-connected-async-bash-target/debug/agent-bash cargo test --locked --no-default-features -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_connected_control root_mapped_connected_l_delivers_real_bash_async_to_recipient_and_acks -- --exact --nocapture
```

Passed 1/1 in 40.97 s. The final source (including the best-effort interim
exit observation) was then exercised with:

```bash
AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" AGE319_CONNECTED_BASH_BIN=/tmp/age319-connected-async-bash-target/debug/agent-bash cargo test --locked --no-default-features -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_connected_control -- --test-threads=1 --nocapture
```

Passed 10/10 in 189.17 s, including async, sync, ordinary help, diagnostics
help, certificate refusal/restart, normal model, and owner close.

```bash
AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" AGE319_CONNECTED_BASH_BIN=/tmp/age319-connected-async-bash-target/debug/agent-bash cargo test --locked --no-default-features -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_connected_control -- --test-threads=2 --nocapture
```

Failed 9/10 in 112.34 s: only
`root_mapped_connected_l_runs_real_bash_sync_child_and_closes_owner` stalled at
owner close with `state_sidecar_outstanding_unknown=true`; normal physical
`state=drained`, publication `state=settled`, and source physical outstanding
was zero. The bounded isolated rerun:

```bash
AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" AGE319_CONNECTED_BASH_BIN=/tmp/age319-connected-async-bash-target/debug/agent-bash cargo test --locked --no-default-features -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_connected_control root_mapped_connected_l_runs_real_bash_sync_child_and_closes_owner -- --exact --nocapture
```

Passed 1/1 in 30.67 s. This slice does not resolve the concurrent sync owner
close signal. It also does not establish live provider API behavior, sleeping
recipient activation, or AGE-353 benchmark capacity. The frozen four-image
archive remains stale and must be rebuilt only after joined source.

After the final Runner rebuild, the installed image hashes were Runner
`1c82689784e7428134aa6fe08b9717e14ffdb6f8593802dfc5d8f256f7aa993b`,
Broker `6c33e27460cece307717417b287b5245b893c9bfe557de2b7815737184e38246`,
launcher `2b01869077fac55790ea9cf0ee3dfc601d74d2dec31cc1136a90bcae31278917`,
and Bash as above. The exact focused async command above passed again with
this final pair, 1/1 in 34.90 s. The following final-pair checks ran serially:

```bash
AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" AGE319_CONNECTED_BASH_BIN=/tmp/age319-connected-async-bash-target/debug/agent-bash cargo test --locked --no-default-features -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_connected_control root_mapped_connected_l_runs_real_bash_sync_child_and_closes_owner -- --exact --nocapture
AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" AGE319_CONNECTED_BASH_BIN=/tmp/age319-connected-async-bash-target/debug/agent-bash cargo test --locked --no-default-features -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_connected_control root_mapped_connected_l_reaches_ordinary_help_with_exact_one_use_custody -- --exact --nocapture
```

Sync passed 1/1 in 28.43 s; help passed 1/1 in 13.63 s.

`cargo fmt --all --check` and `git diff --cached --check` passed on the final
source and report.
