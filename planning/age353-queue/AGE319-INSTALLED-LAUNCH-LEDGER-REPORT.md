# AGE-319 installed launch request ledger — 2026-09-28

## Disposition

This is a durable **request-only** prerequisite for the activated schema-2
Runner/Bash pair. Production Broker L still grants no root, Runner execution,
State effect, exit result, or physical drain. A supported headless CLI L now
publishes an inert root-owned `installed-launches/<request-id>.json` record and
returns `pending <request-id> <pair-generation>`. The installed launcher reports
this as unknown, exits 70, and never submits a new ID after a lost reply.
There was no host State/service/alias, Bash repository, or ticket mutation.

## Durable identity and readback

- The record is fsynced through the existing create-new JSON publisher before
  L replies. It binds canonical request and pair/source UUIDs, the exact
  launch-spec digest, connected launcher UID, and pinned host PID/boot/starttime
  and PID-namespace stamp. Broker startup validates the root-only directory,
  every record's inode/mode/name/content, and the current activated pair/source.
  An abandoned publication stage fails startup rather than becoming absence.
- An exact duplicate request returns `pending` from the existing record;
  altered content or owner refuses. No duplicate path can enter execution.
- The read-only `l` frame carries the same request and pair UUIDs. Production
  Broker accepts it only from the pinned installed launcher image and original
  live process/UID, with the activated pair and source still matching. It can
  be read while ingress is draining. A lost L reply is read back under that
  identity; no second L is needed. If that launcher incarnation is gone, the
  record remains durable, but no replacement launcher can claim its status.
- No root ID is recorded because L does not reserve one. No terminal status is
  emitted because no root terminal, State terminal, or physical drain can yet
  be proven. A process exit or missing PID is never turned into `exit`.

## Verification

Passed with default features disabled:

- `cargo test --locked -p oulipoly-kernel-broker --lib installed_launch_ledger --no-default-features`
  (a separate process publishes and exits without reply; another process
  reopens the record; same UID different process, changed UID/spec/pair/source
  refuse).
- `cargo test --locked -p oulipoly-kernel-broker --lib status_uses_exact_read_only_request_and_pair_frame --no-default-features`
- `cargo test --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker fresh_installed_preflight_accepts_only_supported_headless_cli_shape --no-default-features`
- `cargo test --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker v30_service_refuses_every_legacy_entry_and_work_operation --no-default-features`
- `cargo build --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher -p oulipoly-agent-runner --bin oulipoly-agent-runner --no-default-features`
- `cargo fmt --all --check`, `git diff --check`

An intermediate route test caught that the first `l` gate still admitted a
legacy nonactivated path; that source defect was fixed and the test reran green.
Formatting also failed before `cargo fmt --all` and then passed. Existing
featureless unused-code warnings remain. Full CRW cohort review, workspace
stress, disposable installed end-to-end, and host-root service/alias tests were
not run for this fast source handoff.

## Next boundary

Broker L must choose and durably bind one root ID to this request **before**
the first irreversible fork, using existing RootRegistry/EntryRegistry and the
v30 child grant rather than the private installed supervisor. Only a new
request record may cross that transition; a recorded duplicate remains
read-only. The terminal transition must then bind the exact State terminal
result and root-drain physical close proof, including PID1 ECHILD/parent-wait
evidence, before `exit <code> drained <request-id>` may be returned. The
request-only records created here cannot be retroactively treated as launch
authority. Featureless disposable-root and host-root Runner plus attached Bash
proof remain required before first installation or AGE-353 benchmarking.
