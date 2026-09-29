# AGE-319 connected control grant — 2026-09-28

## Result and effect status

This slice supersedes the inert L result in `AGE319-BROKER-OWNED-LAUNCH-REPORT.md`.
For a newly admitted schema-2 fresh-only L, Broker now forks the pinned Runner
under the launcher's UID/GID, captured cwd, stdio, arguments and environment.
The child inherits a connected Unix socket; the environment contains only its
FD number. The fixed Runner gate requires that socket, checks its live Broker
peer against the installed pair route, and validates the pair/source/request/
root message. A direct Runner with no socket or only a copied FD number closes.

Broker retains a process-stamped, one-use grant in its serving incarnation.
Fresh-only E consumes a kernel-credentialed ready byte from that channel,
reopens the exact durable L root binding, and reserves that UUID in the entry
registry. Direct E and a second E have no grant. The existing P/G/A registry
checks and held J consume the exact reserving entry process and its guardian;
the grant socket is closed in the forked guardian. Broker's accept loop remains
available while the control Runner performs IPC. A duplicate or recovered L
does not spawn. A failed/incomplete spawn, lost grant, or Broker restart leaves
the request pending/unknown with no retry effect.

**Actual effect:** a private root-mapped pair reached a durable exact E entry.
It did not reach J or terminal/drain: the fixture intentionally had no
`OULIPOLY_DATA_DIR`, so guardian setup stopped after E. Production L and `l`
still return pending, and the launcher reports unknown; no drained exit code
is claimed. End-to-end executable first-install work and the AGE-353 benchmark
remain outstanding. No host State/service/alias, Bash repository, or ticket
was mutated.

## Verification

- Root-mapped integration test with actual connected Broker and control Runner:
  `AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" cargo test --locked -p oulipoly-kernel-broker --test age319_connected_control --features age319-private-broker-fixture --no-default-features -- --nocapture` passed. It checked direct Runner refusal with no grant, a copied FD number, and an unrelated connected socket; one pending reply for exact duplicate L; Broker responsiveness while the control was paused before E; direct E refusal; and exact ledger-root E after release.
- `cargo test --locked -p oulipoly-kernel-broker --lib connected_control::tests::inherited_socket_authenticates_child_and_exact_e_is_one_use --no-default-features` passed. It exercises a real spawned child/socket, sender credentials, wrong-process refusal, exact reservation and replay refusal.
- `cargo test --locked -p oulipoly-kernel-broker --lib installed_launch_ledger --no-default-features` passed (4 tests, including lost reply/reopen and root-binding checks).
- `cargo test --locked -p oulipoly-agent-runner --bin oulipoly-agent-runner two_paired_entries_require_same_broker_generation_before_dispatch --no-default-features` passed.
- `cargo test --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker fresh_installed_preflight_accepts_only_supported_headless_cli_shape --no-default-features` passed.
- Featureless Runner, Broker and launcher build, `cargo fmt --all --check`, and `git diff --check` passed.

## Remaining boundary and review debt

The root-mapped test proves connected L through E, not configured P/G/A/J,
native K/Q, State terminal settlement, physical PID1 drain, or a real host
installation. The one-use/restart behavior is source-bound plus ledger tests;
there is no live crash-after-spawn stress test. The full nine-domain CRW cohort
and corrective observation were not run because this assignment prohibited
subagents. Parent review and AGE-353 should treat those as explicit debt, not
as a production-readiness claim.
