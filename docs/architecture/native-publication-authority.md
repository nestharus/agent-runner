# Native completion: publication and original custody

Native authority uses SQLite's **live transaction protocol**, not successful
return from the writer's COMMIT call and not detached recovery of synchronized
WAL bytes. Publication and original actor/Q/wait/file evidence are separate
requirements; neither substitutes for the other.

`MailboxDb::open_read_only` and State's physical-copy readers remain detached,
physically nonmutating observations. Their recovered rows may precede live
wal-index publication. They are not a fallback when authority access fails.
The native writer-authorized lanes use `open_existing_native_authority` and
`read_native_publication`: existing storage, existing namespace validation,
no schema installation, and no copied fallback. These live connections can
participate in recovery and update shared SQLite coordination artifacts.

## Consumer boundaries

- Runtime failure/cancellation receipts obtain their row from a live read
  transaction. Related runtime and original-drain facts use one transaction.
  The receipt is a point-in-time observation, not a portable writer fence.
- Original journal reconstruction runs outside new writer fences. Recovered
  custody retention compares the exact original drain again in State's authority
  transaction. Reusing an old recovered record performs the same check; immutable
  replay does not exempt it.
- Runtime cancellation supplements compare the actual row and original drain,
  and preserve the existing exact-Q requirement for uncancelled spawned recovery.
- Continuing channel duty binds the actual published original drain's domain to
  the exact State owner. It does not release the channel or assert cleanup.
- Native settlement/certification revalidates the actual row/digest (or retained
  supplement), process identity, terminal state and claim predicates before action.
- Owner handshake/domain discovery uses live sidecar reads. It remains a hint,
  not admission authority: `preflight_continuation_binding` checks the live domain
  and running owner's exact process identities under the existing admission fence.
- Source-response classification uses live State admission. Admitted bindings are
  immutable; a structured response remains an observation, not acceptance or ACK.
  Actual source acceptance retains its independent live transaction checks.

State authority operations acquire **State IMMEDIATE → sidecar namespace →
sidecar IMMEDIATE**, holding the sidecar fence through State commit. Replays pass
publication validation before returning. No new sidecar-first nested State
mutation or journal scan is introduced. Existing five-second sidecar contention
returns an error/retry obligation, not absence or completion.

Conflicting historical observations remain unchanged. Their callers must retain
uncertainty/retry responsibility; this change does not repair previously settled
production records or authorize rewriting an immutable replay.

## Verification boundaries

The private AGE360 fault catalog includes a narrowly armed LD_PRELOAD sync shim:
only the selected writer PID, a real commit-marked WAL frame, and a successful
actual fsync/fdatasync enter the hold. It runs with bundled product SQLite and
real original actors/drains. Tests compare detached versus live facts and invoke
the actual receipt and exported recovery consumers, then release the writer and
observe real publication/outcomes. Error and writer-loss controls record actual
live results instead of assuming rollback. The shim is test source only; it is
not installed or loaded by normal product images. Fixture deadlines/namespace
teardown are failure containment, never custody proof.

These finite experiments do not establish arbitrary crash/reboot durability or
universal storage availability. System-SQLite demonstrations and earlier failed
runs remain separate historical evidence.
