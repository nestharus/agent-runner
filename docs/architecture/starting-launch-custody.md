# Starting launch custody (Linux)

A Starting row does not imply that no process was spawned. Creator exit, including
an unreaped zombie, cannot by itself certify cessation of an unpublished tree.
AGE-354 adds prospective independent custody rather than reconstructing a child
PID after a crash.

## Ownership topology

```
creator ── readiness-acknowledged detached monitor M
   └── std::process::Child status proxy P (workload process-group leader)
         └── tree custodian C (separate process group, Linux subreaper)
               └── actual executable W (joins P's process group)
                     └── descendants, including changed groups / double forks
```

M is session-detached but remains the creator's child while that creator lives.
A dedicated parent-side thread owns M's consuming wait, so normal completion does
not rely on reparenting to PID1. Creator crash destroys that waiter but not M;
the environment's adopter then owns reaping M. Setup failure kills/reaps the
still-owned monitor before returning no launch authority.

M exists before Starting creation. It owns an exclusive-created proof inode and
a sequenced-packet endpoint. Contexts and configured commands retain the other
endpoint; fork inherits it before any child instructions can run. A stopped
pre-exec child therefore retains custody even after its creator dies.

C moves out of the workload kill group, establishes subreaping, records Begin to
M, and only then forks W. The executable does not retain the custody endpoint.
C records Done only after observing W's real status and reaching `ECHILD` as the
dedicated, non-transferring owner of that subtree. This is not a claim that
`ECHILD` in an arbitrary observer proves death. M writes quiescence only after
endpoint EOF and balanced Begin/Done records. A lost monitor or lost C is unknown,
not permission to reclaim. Both P and C retain their endpoints through actual
exit; even a stopped status proxy after workload exit withholds quiescence.
No timeout is proof.

P relays W's actual exit code/signal after C finishes and is reaped. Ordinary
`killpg(P, ...)` cleanup can kill P and W but cannot kill C; C continues accounting
for escaped descendants. Natural completion waits for the tree, not just W.
Creator death no longer supplies the executable's lifetime boundary. This may
retain a generation while a deliberately long-lived descendant remains active.

Only the launch operation's P is published as Running. Describe/policy helpers
have their own proxies/custodians but are never published as runtime launches.
Thus the published PID names a runner-owned status proxy, not the executable W.
PID consumers must treat it as the owned runtime-tree identity, not assume its
`cmdline`, immediate parent, or executable image identifies the provider binary.

## Spawn and execution boundaries

CLI headless, direct interactive, and both PTY paths install custody after their
stdio/group/session setup. External-provider dispatch establishes the same scope
before registry preflight; all synchronous provider operations, including describe
and policy, install custody. Allocated attempts share their already-established
context. The scope is thread-affine (not Send/Sync); it does not implicitly
propagate authority into new worker threads. Provider containment filters run *after* the custody pre-exec callback:
they apply to W, not C's required group separation.

After fork, the custodian/proxy branches use only raw syscalls and `_exit`, not
Rust allocation, locks, logging or destructors. W alone returns through normal
Command pre-exec/exec handling. P and C close their spawn-error-pipe copies; W
keeps its copy through actual exec, so no parent-publication/pre-exec wait cycle
is introduced. Creator-thread PDEATHSIG is disabled before the independent fork;
no temporary spawning thread owns the executable's survival.

The implementation requires Linux fork, subreaper and `close_range` support.
Unsupported setup fails closed before Starting authorization. Native macOS and
Windows custody implementations are not supplied here.

## Durable evidence and recovery

PID-sidecar migration 17 adds `runtime_generation_custody`. Generation creation
records the proof path/device/inode atomically with generation/admission fencing.
Quiescence must be read from that exact regular inode. Missing, replaced,
unreadable or incomplete evidence is unknown. No proof files are reused, replaced
or garbage-collected by this implementation.

Both global Starting reconciliation and same-session recovery require custody
proof and the existing conservative creator PID/boot/start observation. Recovery
also revalidates within its mutation transaction. Protocol-bearing Running rows
retain the proof requirement if their proxy has died. Late publication cannot
resurrect a recovered generation.

Finalization revokes future context configuration and awaits affirmative proof;
already-configured commands retain their endpoint until dropped. Thus even a
reusable configured command prevents premature quiescence. Failure to prove
quiescence withholds settlement, not fabricates success. Receipt ACK remains a
separate concern.

Legacy Starting rows have no prospective proof and remain unknown, including on
schema upgrade. No automatic backfill or creator-death clearance is supplied.
All Starting rows block global materialization, including rows without an
associated launching admission. Running rows without this protocol retain their
previous observation contract; this does not retroactively attest that contract.

## Verification boundaries

Private tests exercise creator SIGKILL with its zombie retained, stopped
pre-exec children, unpublished escaped descendants, exact proof-inode replacement,
late publication, both recovery consumers and actual successors through headless,
model REPL and default-provider entry paths. The external protocol's publication
barrier precedes sending its request on stdin: those entry tests prove custody of
the unpublished protocol process, not that a native model began executing.
Separate private process tests cover already-executing descendants. No installed
provider/model or production-state experiment is implied.
