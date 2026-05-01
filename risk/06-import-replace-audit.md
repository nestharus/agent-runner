# 06-import-replace - Phase 4 Audit Risk Report (Rev 3)

**Verdict: HIGH / NOT CLEARED**

Rev 3 closes the specific Round 2 blocker. The journal now carries the resolved
identity needed for deterministic DB recovery, keeps the normalized canonical
records in a journal-attached file, and deletes recovery artifacts only after
postimage verification, fresh export verification, and SQLite commit all
succeed (`proposals/06-import-replace.md:282`,
`proposals/06-import-replace.md:328`,
`proposals/06-import-replace.md:368`,
`proposals/06-import-replace.md:420`).

The proposal is still not cleared because the revised journal lifecycle is not
race-free inside the documented cooperative-lock threat model. Rev 3 writes and
may delete shared per-session journal artifacts before acquiring
`SessionLock`. Two concurrent `import-replace` invocations for the same resolved
session can overwrite or remove each other's recovery signal before one of them
observes `session-busy`.

Note: `risk/06-import-replace-audit-history.md` is not present in the current
checkout. This review used the current Round 2 audit file plus the Round 1
history available from git, which records AIR-R1-F01..F04 and the Round 1
closure path.

## Closure Check

| ID | Rev 3 audit result | Notes |
| --- | --- | --- |
| AIR-R1-F01 | CLOSED | Provider-native rendering remains the write contract; canonical JSONL remains input/hash/round-trip oracle. |
| AIR-R1-F02 | CLOSED WITH NEW R3 BLOCKER | The original durable-recovery gap is addressed, but the new pre-lock journal race blocks clearance separately. |
| AIR-R1-F03 | CLOSED | The lock claim remains scoped to `SessionLock`; non-cooperating writers remain residual. |
| AIR-R1-F04 | CLOSED | Canonical-record field loss remains explicit and tested. |
| AIR-R2-F01 | CLOSED | Resolved journal identity, canonical recovery source, delayed deletion, and recovery tests are now specified. |

### AIR-R2-F01 - CLOSED

Round 2 required three proposal changes: persist resolved recovery identity,
preserve enough canonical postimage material to rebuild `session_turns` without
stale resolver rediscovery, and move postimage/fresh-export verification before
journal deletion.

Rev 3 now freezes `session_id`, `chain_id`, `active_segment_id`,
`provider_name`, `storage_type`, and `jsonl_path` before mutation
(`proposals/06-import-replace.md:264`). The journal stores those fields plus
`canonical_records_path`, preimage/postimage hashes, and expected turn count
(`proposals/06-import-replace.md:328`,
`proposals/06-import-replace.md:560`). Recovery explicitly rebuilds DB rows from
`canonical_records_path`, refreshes the frozen segment/chain, and does not infer
DB rows from provider-rendered bytes (`proposals/06-import-replace.md:352`,
`proposals/06-import-replace.md:432`). Journal deletion is now the final durable
cleanup after postimage verification, fresh export verification, and commit
(`proposals/06-import-replace.md:305`,
`proposals/06-import-replace.md:310`,
`proposals/06-import-replace.md:315`,
`proposals/06-import-replace.md:316`).

The new test rows cover rename-only recovery, ambiguous hash quarantine,
canonical-record preservation, and no deletion before verify
(`proposals/06-import-replace.md:645`,
`proposals/06-import-replace.md:648`,
`proposals/06-import-replace.md:649`,
`proposals/06-import-replace.md:650`). That satisfies AIR-R2-F01 at proposal
level.

## Findings

| ID | Severity | Status | Summary |
| --- | --- | --- | --- |
| AIR-R3-F01 | HIGH | open | Per-session journal artifacts are written and may be deleted before acquiring the session lock. |

### AIR-R3-F01 - Pre-lock journal publication is racy

Rev 3's protected mutation window is supposed to be guarded by
`SessionLock` (`proposals/06-import-replace.md:242`,
`proposals/06-import-replace.md:247`). However, the success flow writes
`<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl` and
`session-<session_id>.pending` before acquiring the lock
(`proposals/06-import-replace.md:282`,
`proposals/06-import-replace.md:287`). The durable side-effect section repeats
that ordering: canonical file first, journal second, both before lock and before
transcript temp write (`proposals/06-import-replace.md:560`).

Those paths are keyed only by session id (`proposals/06-import-replace.md:410`,
`proposals/06-import-replace.md:562`). A second concurrent `import-replace`
process for the same resolved session can therefore overwrite the first
process's canonical records and pending journal before it tries to acquire the
lock. If the second process then receives `session-busy`, Rev 3 allows it to
unlink the journal and canonical records file because it has not mutated the
transcript itself (`proposals/06-import-replace.md:287`,
`proposals/06-import-replace.md:573`).

That breaks the recovery invariant. A lock-owning process may reach DB
reconstruction with `canonical_records_path` missing or containing another
operation's records; a crash after transcript rename may leave startup recovery
with the wrong pending entry or no entry at all. This is not an external
non-cooperating writer problem. It is a race between two instances of the new
cooperative command, so it is inside the documented threat model.

Required proposal change:

- Acquire `SessionLock` before publishing per-session journal artifacts; or
- Use operation-unique journal/canonical paths plus an ownership token so only
  the lock owner can publish, consume, or delete the active per-session pending
  entry; and
- Add a concurrency test where two `import-replace` processes target the same
  session, one wins the lock, the loser exits `13`, and the winner's journal,
  canonical records, transcript, and DB update remain intact.

## Passed Checks

- The public surface remains CLI-only and additive; no GUI/Tauri command,
  provider spawn, quota refresh, config edit, or cross-provider migration path is
  introduced (`proposals/06-import-replace.md:56`,
  `proposals/06-import-replace.md:697`).
- Provider-native JSONL remains out of the public input surface, and supported
  writes still require a lossless renderer (`proposals/06-import-replace.md:167`,
  `proposals/06-import-replace.md:217`).
- `other` storage still fails closed (`proposals/06-import-replace.md:226`,
  `proposals/06-import-replace.md:263`).
- Under-lock preimage recheck still protects the normal preimage TOCTOU gap once
  the lock is held (`proposals/06-import-replace.md:291`).
- Ambiguous startup recovery now quarantines the journal and preserves canonical
  records for manual inspection (`proposals/06-import-replace.md:441`,
  `proposals/06-import-replace.md:609`).

## Audit-History Note

This remains a proposal-phase audit. No implementation was reviewed. Phase 5/6
should not consume Rev 3 until AIR-R3-F01 is revised or explicitly accepted by
the human owner.
