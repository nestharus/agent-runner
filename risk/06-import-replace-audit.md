# 06-import-replace - Phase 4 Audit Risk Report (Rev 2)

**Verdict: HIGH / NOT CLEARED**

Rev 2 materially improves the proposal: it stops writing canonical export bytes
to provider transcript files, adds a durable replace journal, narrows the
cooperative-lock claim, and makes DB field loss explicit. AIR-R1-F01, F03, and
F04 are closed at proposal level.

AIR-R1-F02 is not fully cleared. Rev 2 adds the right recovery mechanism, but
the proposed journal contract is still insufficient for deterministic
post-rename recovery because it does not persist the resolved identity required
to rebuild state rows, and the success flow deletes the journal before the fresh
postimage export verification. That leaves a Phase 6 implementer with a
mutation path that can lose the only recovery signal before proving the provider
transcript is export-readable.

Note: the requested prior files were absent from the current checkout, but they
exist in git at `4a598ac` (`risk/06-import-replace-audit.md`,
`risk/06-import-replace-supported-surface.md`, and
`risk/06-import-replace-audit-history.md`). This review used that committed
Round 1 material plus the current Rev 2 proposal.

## Closure Check

| ID | Rev 2 status | Audit result |
| --- | --- | --- |
| AIR-R1-F01 | Provider-native rendering replaces canonical-byte writes. | CLOSED |
| AIR-R1-F02 | Durable journal and startup recovery added. | PARTIAL / NOT CLEARED |
| AIR-R1-F03 | Cooperative-lock limitation is explicitly documented. | CLOSED |
| AIR-R1-F04 | Canonical-record field loss is explicit and tested. | CLOSED |

### AIR-R1-F01 - CLOSED

Round 1 blocked on Rev 1 writing canonical export JSONL directly to provider
transcript paths. Rev 2 changes the write contract: input remains canonical
JSONL, but the replacement file stores provider-native bytes rendered through
`CanonicalToProviderRenderer`; `other` storage is refused; lossy record classes
exit `15` before mutation (`proposals/06-import-replace.md:26-29`,
`proposals/06-import-replace.md:247-250`,
`proposals/06-import-replace.md:344-352`,
`proposals/06-import-replace.md:691-692`). The test-intent track now checks
that the provider path contains native JSONL and export after replace matches
the canonical import stream (`proposals/06-import-replace.md:553`,
`proposals/06-import-replace.md:569`).

This resolves the original audit blocker. Phase 6 still needs renderer-level
proof for Claude and Codex, but the proposal now gives implementers the right
contract and fail-closed behavior.

### AIR-R1-F02 - PARTIAL / NOT CLEARED

Round 1 blocked on the missing durable recovery signal after file rename and
before DB commit. Rev 2 adds a replace journal, startup scan, preimage/postimage
hash comparison, and explicit recovery tests (`proposals/06-import-replace.md:30-33`,
`proposals/06-import-replace.md:260-285`,
`proposals/06-import-replace.md:363-379`,
`proposals/06-import-replace.md:562-565`). That is the correct direction.

The closure is incomplete because the new journal/recovery spec has a blocking
gap described in AIR-R2-F01 below.

### AIR-R1-F03 - CLOSED

Round 1 flagged overclaiming around exclusive ownership. Rev 2 now says
import-replace acquires `SessionLock`, maps busy to `13`, cites the
06-pause-handshake lock primitive dependency, and explicitly scopes full writer
retrofit to sibling timelines (`proposals/06-import-replace.md:225-228`,
`proposals/06-import-replace.md:666-668`,
`proposals/06-import-replace.md:687-688`). The supported claim is now accurate:
`session-busy` is reliable inside the cooperative lock surface, while
non-cooperating writers remain a documented residual.

### AIR-R1-F04 - CLOSED

Round 1 flagged ambiguous DB field preservation. Rev 2 now says the DB helper
writes only fields present in `CanonicalRecord`, and
`parent_turn_id`, `is_sidechain`, and `is_compaction_boundary` are intentionally
written as `NULL` or defaults (`proposals/06-import-replace.md:353-358`,
`proposals/06-import-replace.md:439-443`,
`proposals/06-import-replace.md:469-471`). The test-intent track includes a
dedicated fixture for this explicit loss model (`proposals/06-import-replace.md:567`).

## Findings

| ID | Severity | Status | Summary |
| --- | --- | --- | --- |
| AIR-R2-F01 | HIGH | open | The Rev 2 journal/recovery contract is not sufficient to guarantee deterministic state recovery. |

### AIR-R2-F01 - Journal recovery is underspecified and cleared too early

Rev 2's durable journal is the new safety mechanism for the highest-risk crash
window. The journal format records only `operation`, `session_id`, `jsonl_path`,
`preimage_sha256`, `postimage_sha256`, `db_state_pending`, and `started_at`
(`proposals/06-import-replace.md:292-303`). Startup recovery then says to read
`jsonl_path` through the storage parser and, if the transcript matches
`postimage_sha256`, re-apply DB updates from transcript rows and refresh the
segment (`proposals/06-import-replace.md:363-374`).

That does not persist enough resolved identity to do the DB recovery
deterministically. The normal DB update API requires the resolved
provider/session identity, replaced path, and canonical records
(`proposals/06-import-replace.md:353-358`), and the state update needs the
resolved `provider_name`, `session_id`, `chain_id`, and active segment identity
(`proposals/06-import-replace.md:431-456`). The current-state map shows why this
matters: `session_turns` is keyed by `(provider_name, session_id, turn_id)`,
chain/segment rows drive resolver ownership, and partial stale rows can make
future resume/export select stale owners
(`research/06-import-replace-problem-map.md:121-129`). A recovery routine that
only has `session_id` and `jsonl_path` must rediscover provider/storage/chain
context from potentially stale DB/config state, which is exactly the state it is
supposed to repair.

There is a second ordering problem. The success flow deletes and fsyncs the
journal immediately after the DB transaction commits, then computes
`postimage_sha256` by reading the newly committed transcript through export
(`proposals/06-import-replace.md:279-285`). If the provider-native renderer
wrote bytes that do not actually round-trip through export, or if the final
export verification fails for any operational reason after DB commit, the
command has already removed the only durable recovery signal. This contradicts
Rev 2's own model that the private replace journal is the crash-recovery signal
(`proposals/06-import-replace.md:650-652`) and that unparsable or
neither-hash transcript states should be quarantined with the journal preserved
for operator recovery (`proposals/06-import-replace.md:529-532`).

This is not an implementation nit. Rev 2's main claim is that the durable
journal closes the post-rename/pre-DB gap (`proposals/06-import-replace.md:535-537`,
`proposals/06-import-replace.md:693-694`). With the current payload and deletion
order, Phase 6 can still produce a committed transcript/DB mutation that cannot
be recovered or even diagnosed by startup recovery.

Required proposal change:

- Persist the resolved recovery identity in the journal before transcript
  mutation: at minimum `provider_name`, `storage_type`, `chain_id`,
  active `segment_id` or an equivalent stable segment key, `session_id`,
  canonical `jsonl_path`, expected preimage/postimage hashes, and enough
  canonical postimage material or parser metadata to rebuild `session_turns`
  without relying on stale resolver output.
- Move fresh postimage export verification before journal deletion, or state
  that any post-DB verification failure leaves/quarantines the journal instead
  of deleting it.
- Add a recovery test that simulates stale or ambiguous resolver-visible DB
  rows after rename and proves startup recovery uses journal identity rather
  than rediscovery through the broken state.

## Fresh Rev 2 Assessment

No new adjacent-surface regression was found outside the recovery contract.
Rev 2 preserves the CLI-only scope, keeps provider-native JSONL out of the
public input surface, avoids provider spawn/config/quota changes, and leaves
resume/repl/trace/migration behavior unchanged
(`proposals/06-import-replace.md:56-70`,
`proposals/06-import-replace.md:626-653`,
`proposals/06-import-replace.md:696-701`).

The supported-surface Round 1 cosmetic issue about stale temp cleanup remains
worth carrying into implementation: §4 still says to clean matching temp files
in the target transcript directory rather than explicitly scoping cleanup to
`<resolved.jsonl_path>.tmp-import-replace-*`
(`proposals/06-import-replace.md:251-253`). This is non-blocking for the audit
verdict because the temp-file convention itself is per target path
(`proposals/06-import-replace.md:489`), but Phase 6 should make the narrower
scope explicit.

## Passed Checks

- CLI shape, input source behavior, receipt fields, and exit namespace still
  match the harness-requested surface (`proposals/06-import-replace.md:3-7`,
  `proposals/06-import-replace.md:385-426`).
- Resolver ownership remains delegated to the Initiative 06 metadata/resume
  path; no second ownership path is introduced (`proposals/06-import-replace.md:61-62`,
  `proposals/06-import-replace.md:244-246`).
- The under-lock preimage recheck remains present and protects the normal
  preimage TOCTOU gap inside the cooperative lock model
  (`proposals/06-import-replace.md:266-273`).
- State consistency still targets replacement of one resolved
  provider/session's turn rows and refreshes the existing chain/segment rather
  than creating a new chain (`proposals/06-import-replace.md:431-456`).

## Audit-History Note

This is a Phase 4 audit gate only. I did not review or change an implementation
because Rev 2 remains a proposal artifact. The report should block Phase 5/6
consumption until AIR-R2-F01 is revised or explicitly accepted by the human
owner.
