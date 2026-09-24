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

The current source supplies the versioned owner/reservation/acceptance
transport, exact readback, an inert offline copy API, a broker ingress latch,
and a gate for the fixed installed Runner image. It does not perform an
installed cutover, gate every packaged launcher, route all production writers,
create wake claims in v30, prepare a new native grant, or release a worker.
These dependencies keep normal CLI/GUI native admission and positive K closed.

## Offline v29 snapshot and v30 entry fence

`BrokerSidecar::stage_offline_v29_snapshot` is an inert preparation API. It,
`publish_and_activate_quiesced_copy`, and `activate_quiesced_copy` require
`QuiescedCutoverProof`, which has no constructor in the normal production
build until the installed writer census and launcher gate exist. Unit tests
construct it internally; the private broker fixture feature exposes a
fixture-only proof and activation helper for temporary namespace storage.
The snapshot API requires host root and root-owned, non-writable storage
ancestry. It checks an
exact `pid-identity.db` regular single-link source and present WAL/SHM/journal
artifacts, refuses a preexisting live `sidecar/`, opens v29 read-only in WAL
mode, and verifies the current completion-schema fingerprint. It computes a
full `sqlite_master` plus typed-row fingerprint for every table, checks SQLite
integrity and foreign keys, uses `VACUUM INTO` to include committed WAL rows,
then compares source and copy. It checks the source pathname/inode and artifact
identity again. For every retained mailbox or completion-event payload, it
requires the old content address, single-link read-only file, unchanged inode
identity, exact length and SHA-256, and matching row metadata and protocol.
It copies each distinct digest once through no-follow descriptors into the
root-only stage, records source and staged file identities in a root-only
manifest, and rewrites only the payload paths, JSON references, and input
compatibility carriers to the fixed broker-owned address. A projected
full-table fingerprint compares all schema and rows after those explicit
transformations. Files and directories are fsynced before publication.
The returned `sidecar-stage-<uuid>/` is never loaded by broker startup. A
failure leaves at most an inert root-only stage for installer inspection; no
live name is changed. The API is intentionally not a CLI or activation path.

The broker now rejects all legacy entry, join, source, work and diagnostic
opcodes whenever it opens a v30 sidecar. The challenged, already-bound
`Y/R/W` State transport and authenticated read-only `I` route observation
remain available, along with `i/X/x` for ingress observation and administrative
transition. The durable draining state refuses `Y/R/W/I` as well. This is a
closed service gate while production callers still open the retired user path.
It also blocks new roots
after a v30 restart; it is not a claim that an old manually invoked binary
cannot write a user-owned historical pathname.

The missing installed coordinator must create a durable launcher version gate
*before* publication, stop and join every supported CLI/GUI Runner,
guardian, driver, maintenance/wake/mailbox worker, provider runtime and helper,
fence their respawn, and inspect open main/WAL/SHM writer handles. An old image
or an unrouteable new image must fail before State/mailbox side effects. A
cooperative `MailboxAuthorityFence` alone cannot establish this against an
old binary. The coordinator must select the exact v29 source, hold the fence,
stage once, revalidate the source and State continuity head under quiescence,
then call the publication method. It rechecks the exact source and complete
copy including payload identities, atomically renames the stage directory to
the fixed `sidecar/` name
without replacement, fsyncs the parent, and invokes v29-to-v30 activation.
Activation refuses a DB-only copy that still has payload references without
broker-owned files and the custody manifest. New notification bytes have a
root-side repository precursor, but no external FD/bytes wire ingress or
recipient delivery/ACK authorization route is opened by this change.
Normal broker startup never calls either migration method.

Crash handling is deliberately closed: an un-published stage is ignored; a
published v29 directory makes broker startup refuse service; a committed v30
generation reopens unchanged. A failed copy leaves the old v29 source as the
only live truth. After publication, rollback needs a separately stopped and
verified recovery procedure, never an ordinary v29 writer open. The installed
coordinator and complete production caller routing are still required before
this source can perform a live cutover. Do not issue native v4 K, a new worker,
or Q from copied v29 records.

## Broker ingress prerequisite added after the offline copy

The broker now owns a durable `entry-gate.v1` latch in its root-owned State
directory and holds a singleton `entry-gate.lock` for its process lifetime.
The challenged `i` request reports `legacy-open`, `draining`, or
`broker-v30-closed` from broker state only; it never reads the retired user
sidecar. A host-root process in the host PID namespace can explicitly close
the latch with challenged `X`. The broker serializes this transition with its
request dispatch, fsyncs the marker, and refuses all ordinary opcodes
afterward, including Y/R/W. An invalid or partial marker prevents restart.
If a prerequisite fails before publication, host root can explicitly abort
with challenged `x`, which persists `open` and resumes legacy admission.
Abort refuses whenever the fixed `sidecar/` name exists. Normal service
startup does not change the latch.
The fixed `/usr/local/libexec/oulipoly/oulipoly-agent-runner` image and the
explicit kernel entry mode query `i` before helper, worker, CLI, GUI or State
effects. They refuse a missing broker, `draining`, and `broker-v30-closed`.

This is a broker ingress and new-image prerequisite, not a quiescence proof.
The Linux release workflow ships a Tauri `.deb` but does not package the
broker service or binary. Its `/usr/bin` GUI path therefore cannot be switched
to a mandatory broker query in this source slice without breaking currently
supported installs. The user-local raw binary and historical installed images
also remain outside the fixed-image path check. A future release must package
the supervisor, broker, CLI and GUI entry gate as one coordinated versioned
installation before retiring old images.

`writer_census::observe_open_state_handles` is a bounded negative observation
for a future installed coordinator. It requires the serving root broker's
detached procfs and initial PID/user namespaces. It pins each visible process
by pidfd, boot ID, starttime and PID namespace, then records executable inode
and any open source main/WAL/SHM descriptor by inode or exact path, including
deleted artifacts. It fails on uncertain live process reads or source pathname
replacement. All observed handles, including those of an unowned old process,
are blockers; the broker never kills them by name or UID. An empty observation
cannot authorize a snapshot: a new process or descriptor can appear after the
scan, and an mmap may outlive its FD. The scanner has no route to
`QuiescedCutoverProof` and does not run a cutover.

The current systemd unit uses `KillMode=process` and does not own every CLI,
GUI, guardian, driver, wake, maintenance, mailbox, provider, receipt-helper or
State cross-store writer. No installed inventory binds all those processes to
exact executable images and PID incarnations, stops respawn, joins their
children, or inspects every open main/WAL/SHM writer handle under a held
sidecar authority fence. Existing old images can still write the user-owned
path without making any broker request. Consequently the latch cannot build
`QuiescedCutoverProof`, publish a stage, or lift v30 routing. A future
installer must own all supported entry paths, including packaged GUI paths
outside the fixed Runner image, and provide that process census and join.
If it crashes after closing the latch, the broker remains closed on restart;
an operator may abort before publication or handle postpublication recovery
under stopped-service authority. The service does not automatically activate
a stage.
