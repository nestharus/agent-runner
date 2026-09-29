# AGE-319 connected J handoff — 2026-09-28

## Achieved boundary

The root-mapped schema-2 first-install fixture now runs the connected Broker
launch through exact E, P/G/A, held J, release, child U/D, and an ordinary
offline Runner `--help`. The fresh State root effect reaches
`returned_success`. The launcher still reports pending/unknown with exit 70;
this is **not** an installed caller exit or proof of physical PID1 drain.

The fixture uses a feature-enabled Broker to map the root-only installation
into an isolated user/PID/mount namespace. It explicitly omits the fixture's
private child mode for this one connected test, so the child takes the normal
v30 release, U/D, and CLI entry path. This is not a literal featureless Runner
execution or a real host installation. The Bash image is pair-verified; no
real Bash child or provider work was executed here.

## Changes and evidence

- The v30 guardian passes an inert legacy positional argument to the exec'd
  driver. `run_with_root` selects the broker-owned v30 branch and never opens
  that argument. The guardian no longer calls `MailboxDb::default_path()` with
  no `OULIPOLY_DATA_DIR` before J.
- The connected fixture starts Broker without `OULIPOLY_DATA_DIR` and launches
  the caller with a cleared environment and private `HOME`. It checks that no
  user `state.db` or `pid-identity.db` appears under that home. The durable E
  root equals the installed L ledger root. The bound entry has one distinct
  original entry, guardian, driver and joined child; the released U receipt
  preserves those exact identities. Fresh State admission and fresh sidecar D
  rows share one request key. The ordinary help text is emitted and the fresh
  root effect is recorded as `returned_success`.
- Direct Runner without a socket, with a copied FD number, or with an
  unrelated connected socket cannot enter. Direct E is refused before and
  after the connected grant. A second L remains pending without another E.
  An exact second J receives an explicit Broker error and creates no second
  child. The connected control socket stays only with the original host
  control; the guardian closes it, and the child receives no control grant.

## Verification

- Root-mapped connected fixture with feature-enabled binaries: `AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" cargo test --locked -p oulipoly-kernel-broker --test age319_connected_control --features age319-private-broker-fixture --no-default-features -- --nocapture` passed.
- `cargo build --locked -p oulipoly-agent-runner -p oulipoly-kernel-broker --bins --no-default-features` passed (existing warnings only).
- Featureless focused tests passed: Broker connected one-use E test, Broker installed launch ledger (4 tests), and Runner paired-entry generation gate.
- `cargo fmt --all --check` and `git diff --check` passed.

## First-install limit and review debt

The exact fresh State terminal settlement, physical root/PID1 drain, and
installed L-to-caller exit delivery are not wired/proved by this test. A
featureless Runner against actual fixed host paths, a real Bash child, normal
provider K/Q, real service/alias installation and AGE-353 benchmarks remain
unexecuted. The fixture's final `returned_success` is a fresh root effect
record, not that missing caller/drain certificate.

One exploratory run emitted a Broker `normal owner close progression blocked:
fresh invocation/session/registration readback conflict` warning while U/D
was in flight. The passing test establishes the offline result record, not
automatic normal owner close. That warning and crash/restart timing still need
observation with the terminal/drain work.

The full nine-domain CRW observation and corrective revisit were not run
because this assignment prohibited subagents. Parent review must retain that
debt and the untested real-install/terminal boundary; this report is not a
merge-readiness or production-readiness claim. No host State, service, alias,
Bash repository or ticket was mutated.
