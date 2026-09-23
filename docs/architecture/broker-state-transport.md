# Broker State transport staging (AGE-319)

The broker's v30 `pid-identity.db` is the intended sidecar truth. Its protected
main, WAL and SHM files and retained SQLite connection belong to the broker.
`broker-state-generation-v1`, `broker-state-read-v1` and
`broker-state-write-v1` use a fresh 16-byte challenge,
the existing pinned peer credential checks, a fixed broker endpoint and bounded
JSON frames. Requests carry the v30 source generation, root ID, owner generation
and exact attempt ID. A bound guardian obtains the generation with challenged
`Y` before its first `W`; a restarted broker returns the same persisted source
generation. Requests carry no database pathname or copied owner row.

The broker checks the live root registry, consumed entry, bound guardian,
domain and supervisor before State access. It derives guardian and driver
process identities from pinned host processes. Only that guardian can publish
an owner or accept an attempt; only its exact child Runner driver can reserve.
The v30 `broker_completion_owner` row marks owners published through this path.
Migrated v29 owners and attempts can be read for reconciliation but cannot
reserve or accept through the new writer. Publication, reservation and
acceptance use the existing State rules; exact owner, attempt, claim and phase
readback uses the same retained connection. A lost write reply is resolved by
an exact `R` read, never by replaying a write or scanning historical attempts.
An accepted row is debt, not a worker grant. Legacy v4 N/k remains closed.

## Required launcher and migration gate

1. Install an entry-version gate in the broker and every supported CLI/GUI
   launcher. The gate must reject old Runner/guardian/driver/helper images and
   any route that opens the retired user sidecar when the broker v30 generation
   is active. A missing or mismatched broker version is an admission refusal.
   Private fixtures must use the same protocol against their private broker
   endpoint; their storage is never an authority for an installed endpoint.
2. Stop and join every supported old writer: Runner CLI/GUI, guardian, driver,
   wake and mailbox workers, provider runtime, receipt helpers, and State
   cross-store repair. Before copying, prove the source version/fingerprint,
   quiescence of known process images and no remaining source DB main/WAL/SHM
   writer handles. Hold the existing sidecar authority fence. Unknown writers
   or inability to establish quiescence blocks cutover.
3. Take one consistent SQLite backup from the quiesced v29 main plus committed
   WAL into a root-only staging directory. Validate the complete sidecar
   schema, integrity and expected State continuity head. Fsync the copy and
   parent; atomically publish the fixed `sidecar/pid-identity.db` name. Only
   then call `activate_quiesced_copy`, which mints v30 source generation.
   Restart before publication discards staging; restart after publication but
   before activation refuses service until the staged copy is validated and
   activation is completed. Restart after activation reopens the same
   generation. Rollback to v29 requires a separately stopped-service recovery
   operation; never open a v30 copy through the old writer API.
4. Move the complete State sidecar writer topology and every write-driving
   read to broker operations before enabling that gate. Preserve StateDb's
   separate admission ordering and bounded repair cursor, exact attempt PK,
   short SQLite transactions, wake claim lifecycle, source notification,
   mailbox ACK, attach and physical Q. An old process may still write a
   user-owned retired pathname, but the new broker never reads it and the
   installed launcher never sends work to its endpoint. A stale copy cannot
   affect the new root-only storage. This is the enforceable boundary; the
   user-owned old pathname cannot be physically forbidden to every historical
   same-UID executable without changing ownership of its ancestor.

This commit supplies only the versioned owner/reservation/acceptance transport
and exact readback. It does not perform the copy, install the launcher gate,
route production writers, create wake claims in v30, prepare a new native
grant, or release a worker. These dependencies keep normal CLI/GUI native
admission and positive K closed.
