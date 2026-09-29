# AGE-319 installed terminal/drain prerequisite — 2026-09-28

## Result

The Broker now retains the actual `waitpid` result for its one connected
control Runner in a fsynced, create-once `installed-control-exits` record. The
record binds the original L request, pair/source generations, root, launcher
process stamp, connected control process stamp, accepted E, and exit code or
signal. A failed read/write or changed identity is unknown; a duplicate L still
cannot create another effect. Restart validates each exit record against its
immutable L record. This is a prerequisite only: neither `L` nor `l` publishes
`exit <code> drained <request-id>` from it.

The root-mapped connected fixture observed exact E/P/G/A/J, U/D, ordinary
offline Runner `--help`, fresh `returned_success`, and the retained control
wait (code 0). The control stamp equals the original reserved E entry; the
receipt carries the same request/root/pair/source/launcher as L. The fixture
also calls connected `l` from the original launcher, checks `pending`, and
submits a duplicate L without another effect. After the root return,
`FreshV30Lane::require_released_invocation` succeeds for the exact released
child/session/registration, so the earlier invocation conflict is a D
publication window in this case. The close scanner still cannot select normal
owner progression for `cli_help`: it requires normal provider K, which help
does not create.

## Remaining terminal boundary

The fresh root effect is not a caller exit certificate. The stored control
wait is not PID1 ECHILD, process-tree cleanup, root-drain readback, or old-side
owner close. There is no exact no-effect help close path yet. The existing
normal provider close path requires K/Q and its own evidence; this run made
none. The launcher still exits 70 on pending, so the original caller has no
later status wait or delivered exit. On Broker death before the control wait is
saved, the wait is unknown; it is never reconstructed from process absence.
No terminal publication was opened.

Normal model work and a real Bash child have not been run through the connected
first-install path. A real fixed-path service/alias installation, host State,
provider K/Q, physical PID1/root drain, crash timing, and AGE-353 benchmarks
remain untested. The fixture uses root-mapped namespaces and feature-enabled
test transport; the Runner/Broker featureless binaries were built separately.
No host State, service, alias, Bash repository, or ticket was mutated.

## Verification and review gap

- Featureless `oulipoly-agent-runner` and Broker bins built with `--locked` and
  `--no-default-features` (existing warnings).
- Featureless Broker library tests: 61 passed, 1 existing 1 GiB test ignored.
  They include absent/changed wait readback, pending after a zero wait, lost L
  reply/restart, and duplicate L refusal.
- The root-mapped connected fixture passed with feature-enabled test transport.
  It tests the actual connected control wait; the private installed executor
  was not used for this launch.
- `cargo fmt --all --check` and `git diff --check` passed.

The nine-domain CRW observation and corrective revisit were not run because
this assignment barred subagents. This is review debt for the parent; the
receipt and fixture are not merge-readiness or production-readiness evidence.
