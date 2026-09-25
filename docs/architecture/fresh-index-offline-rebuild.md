# Frozen broker evidence index rebuild (private AGE-319 lane)

The feature-gated broker exposes detached maintenance commands:

```text
oulipoly-kernel-broker --offline-rebuild-fresh-index <retained-config-directory>
oulipoly-kernel-broker --offline-rebuild-fresh-index-v3 <retained-config-directory>
oulipoly-kernel-broker --offline-reconcile-fresh-index <retained-config-directory> <physical-account-id>
```

The installed path uses the fixed State root and fresh socket. Private fixtures use
`OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1` and
`OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1`. The source directory must be the
same inode registered with each retained route or manual operation, and its
current model and provider bytes must still produce the recorded digest,
roster, physical account identity, quota/auth command and terminal recognizer.
The CLI never opens State DB or its WAL.

The compatible fresh broker holds a process-owned shared POSIX lock on
`index-v1/admission.lock` from
startup until exit. Rebuild takes the exclusive lock without waiting and checks
that the broker socket is not accepting requests. A missing protocol marker or
active broker refuses before scanning or publishing. This is an offline
operation: stop the compatible broker before running it. PID1 may finish a Q
while the scan runs; all consumed K remain unresolved in the new generation.
Use the separate exact account reconciliation command after publication.

The importer reads retained v3 candidates and v4 policy decisions, v1 provider
grants and K, typed terminal records with independently observed physical Q,
v1 quota/auth effects and reuse references, and v1 manual quota intents/K/Q.
It checks sequence continuity per model/config, pin no-advance, source and plan
bindings, and physical Q readback. A missing selected decision, grant mismatch,
unknown record shape, changed source, uncertified Q, or conflicting reuse
refuses. It does not infer a healthy account from a Q filename. Original
artifacts, including any old State WAL, remain untouched.

A new `index-v1/generations/<uuid>` is fsynced and checked before a single
atomic manifest replacement. An interrupted prepublication generation is
invisible. A published generation whose storage disappears refuses on open.
Reconciliation revisits only the named account's unresolved references and
uses exact physical observation and typed terminal or effect readback before
settlement. Unknown or failed effects remain unresolved.

This is a narrow retained-format importer. Older candidate/policy versions,
missing registered source inodes, missing terminal records for already visible
provider Q, and unverifiable original environment bytes are not migrated.
The live selector and provider/effect index writers are still closed, so this
index is not authority for live eligibility or a general pre-index migration.

## Non-activating keyed v3 rebuild

The separate `--offline-rebuild-fresh-index-v3` mode uses the same exclusive
admission freeze and retained-source checks. It cross-checks an existing v2
route/account index against retained announcements, then stages route decisions
and cursors plus per-physical-account hashed keyed grant, effect, manual,
pending, typed source Q, and marker records. Provider Q without a terminal
certificate and effect Q without a verified result remain keyed pending debt;
their observed Q artifact is recorded separately as uncertain Q. The newest
quota and auth Q per exact command/environment source includes failed, empty,
invalid, and all valid windows. Equal-time conflicting Q remains unknown.
Typed provider failures are keyed by grant; account quota/auth markers and
model/config capacity markers have separate keys. An account catalog checks
that every staged physical account directory remains present at open.

Each keyed account write uses the object → audit → intent → root → pointer
publication protocol. The stage reads back every key, checks pending counts,
syncs account and route directories, and rereads model config digests before
the single v3 manifest replacement. A crash before the manifest leaves the
previous generation visible; rerunning the frozen rebuild creates a fresh
generation from the same retained evidence. The old State DB and WAL are not
opened.

**This is a private migration artifact, not live activation.** The existing
v2 `Index::open` intentionally refuses a v3 manifest; the service cannot use
it until a later coordinated keyed writer, selector, and admission cutover.
There is no v3 live reconciliation or routing authority in this slice.
