# 06-import-replace - Phase 4 Audit Risk Report (Rev 4)

**Verdict: LOW / CLEARED**

Rev 4 closes the Round 3 blocker. The proposal now writes normalized canonical
records first to an operation-unique staging path, acquires `SessionLock`, and
only then publishes the per-session canonical-records file and pending journal
under the lock (`proposals/06-import-replace.md:276`,
`proposals/06-import-replace.md:296`,
`proposals/06-import-replace.md:308`,
`proposals/06-import-replace.md:311`,
`proposals/06-import-replace.md:316`). A lock-busy contender deletes only its
own staging file and never creates or modifies the shared per-session recovery
paths (`proposals/06-import-replace.md:309`,
`proposals/06-import-replace.md:354`,
`proposals/06-import-replace.md:601`,
`proposals/06-import-replace.md:605`).

This removes the Rev 3 race where a second process could overwrite or unlink
the lock holder's `<session>.pending` / `<session>.canonical.jsonl` artifacts
before failing with `session-busy`. No open audit blockers remain at proposal
level.

Note: `risk/06-import-replace-audit-history.md` is still absent at HEAD. This
review used the current audit file, the Rev 4 proposal changelog, the
supported-surface/scope prior reports, and the Round 1 history available from
git commit `4a598ac`, which records AIR-R1-F01..F04.

## Closure Check

| ID | Rev 4 audit result | Notes |
| --- | --- | --- |
| AIR-R1-F01 | CLOSED | Provider-native rendering remains the write contract; canonical JSONL remains input/hash/round-trip oracle. |
| AIR-R1-F02 | CLOSED | Durable journal recovery remains specified and is now published under lock. |
| AIR-R1-F03 | CLOSED | Lock claim remains scoped to `SessionLock`; non-cooperating writers remain residual. |
| AIR-R1-F04 | CLOSED | Canonical-record field loss remains explicit and tested. |
| AIR-R2-F01 | CLOSED | Frozen recovery identity, canonical recovery source, delayed deletion, and recovery tests remain in place. |
| AIR-R3-F01 | CLOSED | Per-session journal artifacts are no longer written or deleted before acquiring the session lock. |

### AIR-R3-F01 - CLOSED

Round 3 required eliminating the pre-lock per-session journal publication race
or adding equivalent operation ownership. Rev 4 chooses the cleaner reorder.

The only pre-lock filesystem publication is now
`<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`,
which is explicitly operation-unique scratch state, not a per-session journal
artifact (`proposals/06-import-replace.md:276`,
`proposals/06-import-replace.md:302`,
`proposals/06-import-replace.md:599`). Handled failures before lock acquisition
unlink that staging path only and never publish
`session-<session_id>.canonical.jsonl` or `session-<session_id>.pending`
(`proposals/06-import-replace.md:296`,
`proposals/06-import-replace.md:605`,
`proposals/06-import-replace.md:609`).

After `SessionLock` acquisition, the lock owner atomically renames staging to
the per-session canonical-records path, fsyncs the journal directory, and then
writes the pending journal under the same lock
(`proposals/06-import-replace.md:308`,
`proposals/06-import-replace.md:311`,
`proposals/06-import-replace.md:316`,
`proposals/06-import-replace.md:443`). The journal schema includes
`operation_uuid`, tying the staged input to the active operation
(`proposals/06-import-replace.md:370`,
`proposals/06-import-replace.md:384`).

The required concurrency test was added: two subprocesses target the same
session, exactly one wins, the loser exits `13 session-busy`, unlinks only its
staging file, leaves no per-session journal/canonical files, and performs no
transcript mutation (`proposals/06-import-replace.md:704`). That is the exact
failure mode AIR-R3-F01 required the proposal to pin.

Verdict: AIR-R3-F01 is closed.

## R1/R2 Regression Check

AIR-R1-F01 remains closed. Rev 4 does not loosen the provider-native rendering
contract: v1 writes provider-native bytes rendered from canonical records,
`other` fails closed, and lossy record classes exit `15`
(`proposals/06-import-replace.md:223`,
`proposals/06-import-replace.md:231`,
`proposals/06-import-replace.md:240`,
`proposals/06-import-replace.md:243`).

AIR-R1-F02 and AIR-R2-F01 remain closed. The journal still carries frozen
resolved identity plus `canonical_records_path`, recovery rebuilds DB rows from
that canonical file rather than stale resolver state or provider-rendered bytes,
and journal deletion remains last after postimage verification, fresh export
verification, and SQLite commit (`proposals/06-import-replace.md:357`,
`proposals/06-import-replace.md:383`,
`proposals/06-import-replace.md:401`,
`proposals/06-import-replace.md:455`,
`proposals/06-import-replace.md:471`).

AIR-R1-F03 remains closed within the cooperative-lock surface. Import-replace
acquires the same `SessionLock` primitive, maps busy to `13`, and keeps
non-cooperating external writers as a documented residual
(`proposals/06-import-replace.md:256`,
`proposals/06-import-replace.md:581`,
`proposals/06-import-replace.md:813`,
`proposals/06-import-replace.md:834`).

AIR-R1-F04 remains closed. The DB update still writes only fields present in
`CanonicalRecord`; `parent_turn_id`, `is_sidechain`, and
`is_compaction_boundary` are intentionally written as `NULL` or defaults, with
test intent preserved (`proposals/06-import-replace.md:437`,
`proposals/06-import-replace.md:545`,
`proposals/06-import-replace.md:713`).

## Race-Free Assessment

For the documented cooperative-lock threat model, Rev 4 is race-free on the
artifact ownership axis that blocked Rev 3:

1. Two contenders can both validate input and write staging files, but the paths
   are keyed by `operation_uuid`, so they cannot overwrite each other
   (`proposals/06-import-replace.md:269`,
   `proposals/06-import-replace.md:276`).
2. Only one process can acquire `SessionLock` for the resolved active provider
   session id. The loser exits `13` before any per-session artifact exists for
   that process (`proposals/06-import-replace.md:308`,
   `proposals/06-import-replace.md:309`).
3. The winner is the only process allowed to publish
   `session-<session_id>.canonical.jsonl` and
   `session-<session_id>.pending` (`proposals/06-import-replace.md:311`,
   `proposals/06-import-replace.md:316`,
   `proposals/06-import-replace.md:611`).
4. Failures after lock acquisition preserve the journal and canonical records
   file for recovery; success deletes them only after both verification gates
   and the SQLite commit (`proposals/06-import-replace.md:343`,
   `proposals/06-import-replace.md:623`,
   `proposals/06-import-replace.md:627`).

The prior bad interleaving is no longer possible: a busy contender never touches
the winner's per-session recovery files, and the winner's recovery source is
published only while it holds the lock. The remaining non-cooperating writer
risk is unchanged and already outside the v1 contract.

## Findings

No open audit findings.

## Audit-History Note

This remains a proposal-phase audit. No implementation was reviewed. Phase 6
should implement the Rev 4 ordering exactly: UUID-scoped staging before lock,
per-session canonical/journal publication only under `SessionLock`, and cleanup
limited to the current operation's owned artifacts.
