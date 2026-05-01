# 06-pause-handshake — Phase 8 Supported-Surface PR Review

**Termination signal:** `none`
**Verdict:** **LOW** — diff still reduces risk on the approved supported
surface (advisory session-scope refusal lease via `agents session
pause-handshake` / `resume-handshake`); ordinary fix-pass findings only,
all documentation-track. No invalidated assumption fires; net-value on
the harness consumer surface remains positive. The implementation
implements the contract algorithm and the side-effect contract holds.

The supported-surface termination signal is reported separately from the
LOW/MEDIUM/HIGH verdict, per `~/ai/workflows/pr-review.md`.

## 1. Inputs read

- `research/06-pause-handshake-contract.md` (Step 6a contract bridge).
- `proposals/06-pause-handshake.md` (Rev 4).
- `research/06-pause-handshake-problem-map.md`.
- `risk/06-pause-handshake-audit-history.md` (Round 1 + CodeRabbit
  passes 1–4).
- `risk/06-pause-handshake-supported-surface.md` (Phase 4 LOW, Rev 4).
- `risk/06-pause-handshake-process-tree-audit.md` (Phase 6
  PASS-WITH-ADVISORY).
- `git diff main..06-pause-handshake` — `src-tauri/Cargo.toml`,
  `src-tauri/Cargo.lock`, `src-tauri/src/lib.rs`,
  `src-tauri/src/main.rs`, `src-tauri/src/session_lock/mod.rs`,
  `src-tauri/tests/initiative_06_pause_handshake.rs`,
  `src-tauri/tests/fixtures/initiative_06_pause_handshake.rs`,
  `src-tauri/tests/fixtures/mod.rs`, plus the research/risk artifacts.

## 2. Termination evaluation (orthogonal, evaluated first)

### 2.1 Invalidated assumption — does the current problem framing still hold?

A1–A8 still hold against the implemented diff:

- **A1, A2** (resolver pass-through, session-id key): pause goes through
  `StateDb::resolve_resume` and locks on `resolved.active_session_id`
  (`src-tauri/src/main.rs:835-869`); no second resolver introduced.
- **A3** (running invocations are not session leases): no invocation
  rows are touched.
- **A4** (lease must outlive the process): the durable lease is the
  on-disk `session-<uuid>.lock` file with TTL metadata; the CLI process
  exits after printing the receipt (`session_lock/mod.rs:140-157`).
- **A5** (TTL crash recovery is sufficient for v1): TTL stored as
  `expires_at`; `acquire()` lazily evicts stale leases under sentinel
  flock; no daemon added.
- **A6** (sibling writers observe in their own PRs): no sibling writer
  paths were retrofitted in this diff. v1 surface remains advisory, as
  promised.
- **A7** (file-backed lock receipts owner-private): `0700` lock dir,
  `0600` sentinel/lock/marker/tmp on Unix
  (`session_lock/mod.rs:84-96, 274-282`). Tests assert
  `data_home/oulipoly-agent-runner` is absent on early exit
  (`tests/initiative_06_pause_handshake.rs:53-57`).
- **A8** (POSIX `flock(2)` + same-mount atomic `rename(2)`): module is
  Unix-only via `nix` and `cfg(unix)` permission paths; algorithm uses
  sentinel `flock(LOCK_EX)` and atomic `rename` of a sibling tmpfile.
  A8's documented invalidator (NFSv2/3, cross-mount rename, Windows)
  remains a deployment caveat, not yet fired.

No assumption is invalidated by the diff. **Termination signal #1 does
not fire.**

### 2.2 Non-positive value — does the diff still reduce risk on the approved supported surface?

The Rev 1 retired-risk table from §1.2 of the proposal stands:

- A second writer for the resolved session can now be refused with
  `13 session-busy` and a stable `lock_path` / `expires_at`.
- Wrong releaser is refused with `16 lock-token-invalid`; expired
  unowned release is refused with `17 lock-expired`.
- Crash-survivable cleanup via TTL + sentinel-flock-protected
  stale-eviction is implemented per the algorithm in contract §3.

Net-value on the supported surface is strictly positive. v1 lock
remains advisory until 06-import-replace / migration / repl / resume /
balanced-one-shot wire observers in their own PRs (§13 / R1-F02).
**Termination signal #2 does not fire.**

## 3. Diff vs supported surface (contract §1, §4–§7; proposal §3, §5, §8, §11)

### 3.1 CLI surface

`Subcommands::Session { command: SessionSubcommands }` with `PauseHandshake { session_id, ttl_ms: Option<u64> }` and `ResumeHandshake { session_id, token: String }` (`src-tauri/src/main.rs:159-191`). Matches contract §1 exactly.

`<session-id>` parsed as `Uuid` before any state/lock access; bad UUID
exits `2` with stderr JSON `invalid-session-id`
(`src-tauri/src/main.rs:801-806, 882-887`). Matches contract §6 and
proposal §2.

### 3.2 TTL policy

Code: default `60_000` ms, max `600_000` ms; over-max → exit `2`
(`src-tauri/src/main.rs:19-20, 808-816`). This matches the **contract**
defaults and the audit-history Pass 3 R3-F11 reconciliation. The
proposal §4 D3 text (`300000` / `1000` / `1800000`) is stale; the audit
history records that Phase 6 follows the contract. Documentation
divergence — see §6 Finding F2.

### 3.3 Pause success JSON

Stdout fields emitted: `session_id`, `chain_id`, `provider_name`,
`token`, `expires_at`, `lock_path`
(`src-tauri/src/main.rs:846-864`). Matches contract §4 and proposal
§3.1. Token is `pause_<32 lowercase hex>` from
`getrandom`-backed CSPRNG (`session_lock/mod.rs:336-361`). Matches
proposal D2.

### 3.4 Resume success JSON

Stdout fields: `session_id`, `token`, `released_at`,
`already_released` (`session_lock/mod.rs:23-29`,
`src-tauri/src/main.rs:907-916`). Matches contract §4 (four-field
shape).

The proposal §3.2 receipt shape (six fields plus optional `note`) was
narrowed by the contract; implementation follows the contract. Harness
consumer reads the contract, so net-value is preserved, but proposal
prose is stale — see Finding F2.

### 3.5 Error stderr

`emit_json_error` writes `{"error": {"code", "message"}}` on stderr
(`src-tauri/src/main.rs:991-1000`). Matches contract §4. `Busy`
surfaces only `expires_at`, not raw token material — consistent with
the audit-history Pass 1 watch signal and Pass 3 R3-F11.

### 3.6 Exit codes

| Code | Source | Code-side mapping |
|---|---|---|
| `0` | success | `run_pause_handshake`, `run_resume_handshake` |
| `1` | `operational-error` | DB/config/lock-IO errors, `LockError::Operational` |
| `2` | `invalid-session-id`, `invalid-ttl`, clap usage | up-front guards |
| `10` | `session-not-found` | `ResumeError::NoChainFound` |
| `11` | `ambiguous-session` | `ResumeError::Ambiguous` |
| `12` | `model-resolution-failed` | `UnknownModel` / `ProviderModelMismatch` / `ActiveSegmentMissing` / `ProviderNotConfigured` / `ProviderMissingResume` |
| `13` | `session-busy` | `LockError::Busy` |
| `16` | `lock-token-invalid` | `LockError::TokenInvalid` (incl. up-front malformed-token check) |
| `17` | `lock-expired` | `LockError::LockExpired` |

Matches contract §5. Proposal §5 explicitly reserves `12` (says "not
used here"); implementation and contract use `12` for resolver
model-resolution failures. This is the same proposal/contract
divergence as §3.2 — Finding F2.

### 3.7 Sentinel-flock + atomic-rename algorithm

`SessionLock::new` opens `<lock_dir>/sentinel.lock` with
`O_CREAT | O_RDWR` (idempotent) and never unlinks it
(`session_lock/mod.rs:81-98`). `with_flock` takes
`flock(LOCK_EX)` for the full inspect/mutate critical section
(`session_lock/mod.rs:217-236`). Acquire / release write a uniquely
named sibling tmpfile (`<prefix>-<pid>-<random_hex_128>.tmp`),
`fsync` it, and `rename` it onto the target path under the sentinel
flock; the directory is fsynced after
(`session_lock/mod.rs:140-148, 176-189, 263-294, 304-308`).

This implements contract §3 and proposal Rev 4 §4 / §6 / §8 verbatim.
The R3-F01 multi-stale-contender interleaving from the Phase 4
supported-surface report is structurally absent for the same reason
the report identified: serialization on the never-unlinked sentinel
inode plus same-directory atomic rename. The "Atomic acquire" and
"Stale acquire" oracles in §9.1 are exercised by
`concurrent_pause_only_one_subprocess_acquires_same_session` and
`concurrent_stale_pause_only_one_subprocess_replaces_expired_lock`
(`tests/initiative_06_pause_handshake.rs:97-126, 210-234`).

Implementation note (non-finding): the tmpfile is opened with
`create_new(true).write(true)` — i.e. `O_CREAT | O_EXCL | O_WRONLY`
rather than the contract's `O_CREAT | O_TRUNC | O_WRONLY`. Tmp names
are pid+128-bit-random scoped, so `O_EXCL` is strictly safer; not a
contract regression.

### 3.8 Side-effect contract (proposal §8, contract §7)

Only the lock-state files under `<XDG_DATA_HOME>/oulipoly-agent-runner/locks/`
are mutated:

- `sentinel.lock` (idempotent `O_CREAT`, never unlinked).
- `session-<uuid>.lock` (atomic-rename install / unlink-on-release).
- `session-<uuid>.released` (atomic-rename install on release).
- `<prefix>-<pid>-<rand>.tmp` (transient).

`StateDb::open_default()` is called for resolver-only access; no
`session_turns` / `session_chains` / `session_chain_segments` /
invocation / quota / config writes occur. No provider spawn, signal,
quota refresh, scanner, or migration is invoked. The
`StateDb::open` open-time effects (parent dir create, WAL, schema
ensure, chain backfill) are accepted per proposal §8, matching
06-locate / 06-export.

The tests' `assert_success` asserts `output.stderr.is_empty()`
(`tests/fixtures/initiative_06_pause_handshake.rs:316-320`), so any
unexpected stderr noise from a broader effect surface would fail.

### 3.9 Permissions / owner-private state

Lock dir `0700` (`session_lock/mod.rs:84-87`). Sentinel/lock/marker/tmp
files `0600` on Unix (`session_lock/mod.rs:92-94, 276-279`). No Windows
permissions path; the binary won't build on Windows because `nix` is
Unix-only — consistent with proposal §12 "Windows semantics are not
designed".

### 3.10 Migration / rollback

No state migration. Existing sessions are unlocked because no lockfile
exists. Rollback path: stop invoking the subcommands and (optionally)
delete files under `<XDG_DATA_HOME>/oulipoly-agent-runner/locks/`.
`sentinel.lock` is harmless to leave in place. Older binaries cannot
read or interpret these files; they are inert to v1-naive code paths.

### 3.11 Observability

Stdout JSON receipts and stderr JSON errors are the entire v1 surface.
No trace event, no audit row, no telemetry, no `agents session observe`
CLI (the `observe` API noted in §6 of the proposal is not in this
diff and is not required by the contract — sibling concern, OK to defer).

### 3.12 Adjacent surfaces not changed

- `agents session locate`, `schema-probe`, `export` — not in the diff
  (separate stacked PRs); pause does not call them.
- `agents repl`, `agents resume`, top-level `--resume`, balanced
  one-shot, `migrate-db`, `migrate-config`, `migrate_chain_segment` —
  not edited; v1 lock remains advisory until those land observer
  retrofits, exactly as proposal §13 / R1-F02 commits.

## 4. Phase 4 supported-surface advisories — closure check

| Phase 4 advisory | PR diff impact | Status |
|---|---|---|
| #1 README/§11 deployment caveats (Linux/macOS supported, Windows undefined; A8 NFS/cross-mount caveat) | README untouched in this diff | **OPEN — Finding F1** |
| #2 `agents session observe` ergonomics (R1-F05-supported) | not in scope; deferred to sibling adoption PR | **OK (deferred)** |
| #3 Sibling-PR observers should refuse-and-emit `session-busy` | sibling-PR concern; out of scope here | **OK (deferred)** |
| #4 "Expired-token-after-stale-replacement UX" wording | implementation behaves as advisory predicted (returns `16` if fresh acquire wins flock race ahead of stale-resume); README/§12 not updated | **OPEN — folded into Finding F1** |
| #5 Optional explicit "two stale contenders" §9.1 row | T8 `concurrent_stale_pause_only_one_subprocess_replaces_expired_lock` covers this in tests | **CLOSED** |

## 5. Test-residual artifact check

No `risk/06-pause-handshake-test-residuals.md` exists in the worktree.
Phase 4 supported-surface report carried no unverified residual that
collapses the net-value case. Phase 6 process-tree audit verdict is
PASS-WITH-ADVISORY (fixture stdio piping fix only). All §9.1 risk rows
have a mapped test except optional advisory item #5, which is in
practice covered by T8. No Test Audit firstness/residual collapse
escalation reaches this gate.

## 6. Findings

### Finding F1 (MEDIUM, fix-pass — documentation supported-surface gap)

**README updates mandated by proposal §10 are absent from the diff.**
`git diff main..06-pause-handshake -- README.md` is empty; `grep` for
`pause-handshake|sentinel|locks/` in `README.md` returns no matches.
Proposal §10 explicitly mandates documenting:

- Both command synopses (`pause-handshake`, `resume-handshake`).
- Pause and resume receipt fields (proposal §3 / contract §4).
- Token format and TTL default/max bounds.
- Exit codes `0`, `1`, `2`, `10`, `11`, `13`, `16`, `17` (and the
  contract-promoted `12` — see Finding F2).
- The sibling release marker `session-<uuid>.released` and same-token
  idempotent replay semantics.
- v1-advisory framing and deferred sibling adopters
  (06-import-replace, migration, resume/repl, balanced one-shot).
- The `~/.local/share/oulipoly-agent-runner/locks/` paragraph
  (sentinel + per-session `.lock`/`.released`).

This is the supported-surface obligation flagged by Phase 4 advisory
#1 (Linux/macOS v1-supported, Windows undefined, A8 invalidator
deployment caveat) and the §9.1 "README truth" row. The CLI surface
ships without operator-visible documentation; harness consumers reading
the proposal will see the proposal-stale (300_000/1_800_000) TTL and
six-field resume receipt, while operators reading `agents --help`
alone will not learn about the locks directory or the advisory-scope
caveat.

Net-value on the supported surface is still positive (the harness
consumer can read the contract / receipts directly), so this is not a
termination signal — but it is a public-surface documentation gap that
must close before this PR is shippable to operators.

**Recommended fix-pass action:** add a README section near the existing
`state.db` paragraph and CLI synopses listing the synopses, receipt
field tables (matching contract §4), exit codes (including `12`),
TTL bounds, marker path, advisory-scope sentence, and the A8/Windows
deployment caveat from Phase 4 advisory #1.

### Finding F2 (LOW, fix-pass — proposal vs contract reconciliation)

The Rev 4 proposal text contains stale wording in three places that the
Phase 6 contract supersedes (per audit-history Pass 3 R3-F09, R3-F11
and Pass 4):

1. **TTL bounds.** Proposal §4 D3 says default `300_000` / min `1_000`
   / max `1_800_000` ms. Contract §1 and implementation use default
   `60_000` / no minimum / max `600_000` ms.
2. **Exit code `12`.** Proposal §5 reserves `12` ("not used here").
   Contract §5 and implementation map `model-resolution-failed` to
   `12`. Proposal §10 README mandate omits `12`.
3. **Resume success JSON shape.** Proposal §3.2 lists six fields plus
   optional `note`. Contract §4 narrows to four fields (`session_id`,
   `token`, `released_at`, `already_released`); implementation matches.

These are documentation drift, not implementation defects (the contract
is the authoritative bridge per audit-history). They become
operator-facing if and only if Finding F1's README copy is sourced from
proposal §10 verbatim — which would propagate the stale namespace.
**Recommended fix-pass action:** when applying F1, source the README
content from contract §1 / §4 / §5, not proposal §3 / §4 / §5 / §10.
Optional: add a Rev 5 footnote to the proposal pointing readers at the
contract for TTL, exit-code `12`, and the resume receipt shape.

### Finding F3 (LOW, fix-pass — expired-token UX wording)

Phase 4 supported-surface advisory item #4 is unaddressed in the diff.
A `resume-handshake` whose token matches an expired lease can return
`16 lock-token-invalid` (rather than `0` with `note: released expired
token`) if a fresh `pause-handshake` won the sentinel-flock race ahead
of resume. Implementation honors this — `release()` only emits
`already_released: true` from a same-token marker, and a fresh acquire
removes the prior marker (`session_lock/mod.rs:146`) — so the `16`
outcome is the correct safety choice. The harness contract is unchanged
because exit `16` is already in §5.2.

The gap is documentation: §10 / §12 (proposal) and the README (when
written per F1) should acknowledge this outcome explicitly so a harness
consumer that supplies an expired-but-once-valid token does not treat
`16` as a contract violation. Non-blocking; fold into the F1 README
work.

## 7. No-regression summary

| Phase 4 LOW signal | PR diff impact | Status |
|---|---|---|
| Wire format / public API byte-identical to Rev 4 | All §4 receipt fields, `Busy.expires_at` only, token format, marker path, TTL semantics implemented as Rev 4 contract. | **preserved** |
| Side-effect contract bounded to lock dir | Verified by code path inspection and assertion that stderr stays empty on success. | **preserved** |
| Migration / rollback / observability LOW | No DB DDL, no telemetry, no schema migration; rollback by deletion still works. | **preserved** |
| R1-F01..R1-F04, R2-F01, R3-F01 closures | All preserved; sentinel-flock + atomic-rename implemented as designed. | **preserved** |

No supported-surface signal degrades. Termination signals do not fire.

## 8. Verdict

- **Termination signal:** `none`
- **Verdict:** **LOW**
- **Action:** advance the PR after the F1 README work (and folded F3
  wording) lands. F2 is doc-polish and may ride with F1.

The findings above are ordinary fix-pass items for the synthesize-and-post
gate. Group them under the README / docs concern; severity order F1 →
F2 → F3.

## 9. Boundaries

This review is read-only per role. No code, test, or proposal edits
were made. README copy and proposal Rev 5 footnote suggestions in
Findings F1 / F2 are inputs for the fix-pass agent, not actions taken
here.
