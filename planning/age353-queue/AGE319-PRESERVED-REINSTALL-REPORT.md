# AGE-319 preserved reinstall operator report

## Goal

This operator installs the new pair over the one served first host. The new
pair is generation `4e0edeee-816a-5ff7-aa29-930a974adcd7`, package
`8367b3ea…98fc37d9`, built from `ba6f537a` plus Bash `6c0a83ba`. The host
currently runs old pair `2ca23e6d-fe12-5b8e-9656-970801e22377`, package
`045613d2…79af9e0`.

The old State, including failed attempt
`b8e06d25-70fe-4f1f-bfef-3541913346e1`, keeps its bytes and device/inode
identity, as do the old images and unit. Nothing is deleted, replayed or reset.
Admission, UIDs and the socket group are not weakened. This operator targets
only this campaign; it is not a migration framework.

## Choice: move State aside, not keep it in place

Source reading rules out keeping State in place:

- `first_install_activation.rs` pins the manifest and all four images by
  SHA-256, device and inode.
- Broker startup (`linux_main.rs` `serve`) checks that exact binding on every
  incarnation, so new images with the old State would refuse to start.
- Activation refuses a root that has already served (`entry-gate.lock` /
  `entry-gate.v1`).

Getting past either check would need a Rust change or a forged record, and both
are prohibited.

Moving it aside with `rename` is feasible and was already authorized. `rename`
keeps every inode, so the old activation record stays valid if the install is
ever renamed back. A copy would break that.

The new pair then goes through the unchanged first-install
`install/check/activate/start/readback` path with a **new empty** State. The
old served State is never offered to bootstrap or activation.

**Consequence for root:** the new Broker has a new source generation and
domain, and the failed-attempt record is not visible to it.

## Construction

The new operator is `packaging/linux/reinstall_preserved_v30.py`, with commands
`status`, `preserve` and `restore`. The README section "Preserved reinstall
over the served AGE-319 first host" gives the full root sequence.

Preservation scope is one root-only directory,
`/var/lib/oulipoly-age319-preserved-2ca23e6d-fe12-5b8e-9656-970801e22377/`,
containing:

- `state/`: the whole old `/var/lib/oulipoly-kernel-broker` tree;
- `libexec-oulipoly/`: the old `/usr/local/libexec/oulipoly`;
- `oulipoly-kernel-broker.service`: the old unit;
- `pre-stop-v1.json`: the pre-stop census and a live State snapshot;
- `baseline-v1.json`: the quiescent tree identities.

Some things are not moved:

- **Bootstrap lock:** `/var/lib/.oulipoly-kernel-broker-empty-v30-bootstrap.lock`
  is an empty file, reused by the new bootstrap under `flock`.
- **Runtime directory:** `/run/oulipoly-kernel-broker` is removed by systemd
  on stop (`RuntimeDirectoryPreserve=restart`). Its listing before the stop is
  recorded.
- **User data:** user-local aliases, old v29 data and any user-side records are
  not touched.

Identity means type, mode, uid/gid, nlink, device/inode, and file size, mtime
and SHA-256. It does not include ctime, atime or directory mtimes, which a
rename legitimately changes.

The stop rule works as the decision set it:

- Before any change, the only actor allowed is the unit's `MainPID`. It must
  equal `--broker-pid 768990` and run the installed Broker image inode.
- An actor is any process with an installed-image exe inode, membership of the
  unit cgroup, or a cwd/fd path under State, the runtime directory or the
  preservation root.
- Any other actor, or any process the census can't read, refuses before
  anything changes.
- After `systemctl stop`, the unit must be inactive or failed with `MainPID=0`,
  the old incarnation must be gone, and the census must find no actors and no
  unreadable processes. Otherwise it refuses with the stop kept and reported.
- There is no name or PID sweep, and nothing is ever signalled apart from the
  unit stop.

On interruption, each rename is atomic. `status` shows each item as original,
preserved, both or absent, along with whether it matches the baseline. A rerun
continues from the records and refuses any change since the baseline, and mtime
counts as a change: restored bytes are not treated as the same. `restore`
renames back only while the original paths are free. It refuses once a new
install occupies them, and it leaves the service stopped.

## Evidence

### Corrective candidate under the authorized two-goal Decision

The original candidate was `25969f0dc292fb1c7da7aa6e732669c279068bb0`, tree
`45493078fcc69ed65e6cc87097e5313b194838be`, parent/base
`ba6f537a419f77d5a3f6c4370d7ef0cb9d04b32e`. Its original commit object and
operator are retained in the correction logs. This correction stays on the
same owned tree and branch. Its delivery commit amends that candidate, retaining
the parent and human signature but removing the AI co-author trailer as the
Decision authorizes. The final exact commit/tree/parent and signature/push
readbacks are external proof records, avoiding a self-referential commit hash.

**Goal 1:** accept the genuine recorded served source and refuse a different
State file. `verify_old_install` now compares bootstrap identity with
`lstat(state/state.db)`, retaining the separate State-directory check. It
requires a regular, single-link, operator-owned, owner-only State file.
This is file-identity verification; it does not open or validate the sidecar
schemas. The existing activation-source and old-pair comparisons remain.

The fixture records now follow the actual Rust construction:

- `crates/oulipoly-state/src/mailbox/fresh_lane.rs:80-97` defines the complete
  bootstrap/lane record fields; `:162-203` gets metadata from `state.db` and
  embeds that device/inode. `:246-264` checks it against the sidecar's bound
  State file identity. `:821-859` constructs the independent lane identity.
- `crates/oulipoly-kernel-broker/src/first_install_activation.rs:28-47`
  defines all five file fingerprints; `:142-156` captures the manifest and
  four installed images and embeds the source unchanged; `:213-217` supplies
  device/inode/hash fields.
- The fixture uses those JSON shapes, local UUID stand-ins, installed-file
  fingerprints and an owner-only real SQLite container with a fixture record.
  It does not implement the Rust State, sidecar or lane schemas or execute Rust
  bootstrap/activation. Its provenance is source-derived construction, not a
  record emitted by a live Broker or a Cargo-built fixture binary.

**Goal 2:** final `status` compares all three old objects, selecting preserved
paths whenever those exist, including location `both`. The JSON includes all
three `matches_baseline` values and their `comparison_paths`. Only all three
present at preserved paths and matching give `verdict: "preserved"` and the
plain statement `preserved old State, images and unit match baseline`.
Before preservation, successful prechecks give `readiness` and `verdict`
`ready`. Every other normal status returns `verdict: "not ready"`; the CLI
prints the report and returns 1. Raised refusals still use the existing error
handler and return 1. Partial preservation and a completed restore are not
readiness/preservation success. Census/service fields remain diagnostics;
the final verdict establishes tree preservation, not health of the new Broker.

Correction proofs are in
`/home/nes/projects/agent-runner/planning/context-routing-20260929/reinstall-correction-logs/`:

- `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s packaging/linux
  -p test_reinstall_preserved_v30.py -v`: 9 tests, OK, exit 0
  (`reinstall-tests.log`). This covers acceptance before moving, unchanged
  physical retention, replacement-inode and directory-identity refusal before
  change, all three original paths occupied by a new fixture install, altered
  and missing preserved State/images/unit, and non-success for partial moves.
  CLI JSON/return-code tests feed actual fixture status through the production
  CLI adapter with root privilege and the host manager mocked; no host endpoint
  is invoked.
- `PYTHONDONTWRITEBYTECODE=1 python3
  /home/nes/projects/agent-runner/planning/context-routing-20260929/reinstall-correction-logs/run_correction_proofs.py`:
  exit 0 (`proof-run.log`, `proof-summary.json`). The genuine-record acceptance
  test fails by assertion against the original operator and against the
  directory-comparison mutant. The final-readback test also fails by assertion
  when `both` comparisons are omitted. Refusal/mismatch tests fail when status
  always exits 0. The corrected control passes all 9 tests. Complete test
  traces and tested source copies are retained, with no setup/import errors.
- `superseded-proof-selector-error.log` retains a first proof-run failure:
  the mutation selector matched two comprehensions and refused before tests.
  The selector was narrowed; no product change was made for that error.
- `git diff --check`: exit 0. No Rust build, Cargo run, cache edit, real-package
  rebuild, host State access, service action, host census or model workload.

These are producer proofs for root's independent source observer and Frame,
not admission. Full Core adoption, initial all-nine CRW exposure and owed
active-domain visits remain missing; the bounded focused observation required
by the Decision does not fill them.

### Original evidence retained; real-contract claim superseded

Original logs remain intact in
`/home/nes/projects/agent-runner/planning/context-routing-20260929/reinstall-maker-logs/`.
The original tests constructed the State directory identity, so their success
did not establish acceptance of an intact real bootstrap. The original final
status omitted `both` objects and returned 0 for refused readiness or false
comparisons. Those original use claims are superseded by the source observation
and this correction; the historical results below are not reruns or evidence
for the corrected source.

- **Packaging suite** (`packaging-linux-unittest.log`): 19 tests in the
  first-install, host-install, paired-bundle, versioned-island and new reinstall
  modules, all OK, exit 0. The reinstall tests also passed in 10 of 10 repeat
  runs (`reinstall-test-repeat-10.log`).
- **Shared front door, not run**
  (`packaging-linux-unittest-with-unbuilt-front-door-fixture.log`): its 5 tests
  fail with "build oulipoly-shared-front-door fixture binary first". Those files
  are unchanged, and a Cargo build is outside this task. They are unrun, not
  passed.
- **Mutation checks** (`run_mutants.py`, `mutants-summary.log`, `mutant-*.log`):
  the unmodified copy passes. Each of these mutants fails at least one test:
  - no drain check;
  - no fd census;
  - copy instead of rename (the operator's own baseline check refuses);
  - ignoring mtime;
  - no State-identity refusal;
  - no restore-occupancy refusal;
  - no refusal for unreadable processes.
- **Superseded logs** are kept in the `superseded-*` directories:
  - A racy readiness wait in the test once gave a false ERROR. It is now fixed
    with a readiness line from the holder process.
  - An earlier `skip-source-check` mutant survived only because of operator
    precedence, which made the mutant itself invalid.
  - An over-broad assertion let the unreadable-process mutant survive. It is
    now tightened.
- **Real-package argument check** (`real-package-argument-check.json`): a
  non-root read of the two package files only. Their SHA-256s and the
  generations `2ca23e6d…`/`4e0edeee…` match the records. The service bytes are
  identical in both (`26250bf2…`).

What the fixture shows: in a disposable directory tree, the operator runs
against a simulated service manager and real child processes read from `/proc`,
with the census limited to those children. It covers exact-identity move-aside,
a subsequent new first install, restore and its refusal, other-actor and
unreadable-process refusal before stop, an unconfirmed drain with the stop
kept, interrupted-rename partial locations, and refusal after a change. The
interrupted-rename test did not complete a successful resume.

What it doesn't show: host replacement, root privilege, real systemd or
`KillMode=process` behaviour, the full-host census, the UID split, restoration,
stress, or any Core/CRW review.

## Remaining uncertainty

The authorized Decision accepts the existing census bounds, interruption
edges, package-binding debt and five unexecuted front-door tests. A host
refusal, interruption or unexpected live actor returns to root with output
kept, without improvisation or a process sweep. None of this correction
expands those accepted debts into hardening work.

- **Host State and actors are unread.** No live census or State read was done.
  If the failed help attempt left a live Broker child, root PID 1 or driver,
  `preserve` will refuse before stopping. That is a consequential return, not
  something to sweep.
- **Stop timing.** How long the Broker takes to handle SIGTERM on the host is
  unobserved. If `systemctl stop` hits the timeout, systemd SIGKILLs the main
  PID and the drain census still decides.
- **Socket peers.** Unix socket peers without paths aren't mapped. A client
  that isn't running an image and holds only a socket isn't counted. It loses
  the socket when the Broker exits.
- **Package paths.** Both packages live under user-owned paths. Root checks
  `sha256sum` before starting and the pair generation at `readback`, but
  `install` itself doesn't pin the package hash.
- **Partial failure.** A refusal after the stop leaves the Broker stopped with
  the record kept. Root decides whether to restore and restart.
- **The help rerun** still has to capture the items the admission decision
  listed. The caller shell needs `/usr/bin/sg oulipoly` (bare `sg` is
  ast-grep). The new empty State and domain may interact with caller-side
  records; that is unobserved.
- **Core/CRW gaps are unchanged.** This report gives no admission or
  restoration verdict.

## Corrective construction witness for root

Purpose and expected contribution: reach physical preservation on the genuine
recorded source and give the operator a trustworthy final preservation verdict.
Intended host effect is not measured; the local observations above are fixtures.

What the user meets first is the existing README command sequence (foreground,
candidate), with exact package hashes and the initial `ready` and final
`preserved` meanings. Then the command's JSON (foreground, computed from current
prechecks or baseline comparisons) states the verdict; locations, comparison
paths, per-item results and census are available evidence beneath it. This
report and proof logs are inquiry material for root's observer and Frame.
Synthetic UUIDs, database contents and service behaviour exist only in tests.

This version replaces the directory-identity assumption, incomplete record
stand-ins, omitted `both` comparisons and success-looking refusal. The baseline,
rename mechanism, old-object retention, new empty State, no-sweep stop rule and
restore remain carried. No kept mechanism is removed. The identity correction
depends on the Rust bootstrap definition; the status verdict depends on all
three captured trees and their actual paths; the README claims depend on these
outputs. The low-detail JSON treatment keeps the existing operator form and
puts a decisive result beside its evidence. A refusal's reaction route is the
preserved output to root, then actual Frame/Decide. The next consumer is root's
independent observer; only root delivers commands to the user after admission.

No adapted agent or child was used. No native work remains running at return.
No host UID, restoration, stress, full Core or all-nine completion is claimed.
