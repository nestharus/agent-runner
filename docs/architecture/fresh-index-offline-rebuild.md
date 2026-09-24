# Frozen broker evidence index rebuild (private AGE-319 lane)

The feature-gated broker exposes detached maintenance commands:

```text
oulipoly-kernel-broker --offline-rebuild-fresh-index <retained-config-directory>
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
