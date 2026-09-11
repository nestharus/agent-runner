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
endpoint; configured Commands retain an Arc to the same parent descriptor rather
than duplicating an FD per command. Fork duplicates the descriptor table before
any child instructions can run. A stopped
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
propagate authority into new worker threads. The live-session binding worker
explicitly enters its generation's scope before capture/locator provider calls.
This does not preserve the binding listener after creator death or supply duplicate
ACK behavior. Provider containment filters run *after* the custody pre-exec callback:
they apply to W, not C's required group separation.

After fork, the custodian/proxy branches use only raw syscalls and `_exit`, not
Rust allocation, locks, logging or destructors. W alone returns through normal
Command pre-exec/exec handling. P and C close their spawn-error-pipe copies; W
keeps its copy through actual exec, so no parent-publication/pre-exec wait cycle
is introduced. Creator-thread PDEATHSIG is disabled before the independent fork;
no temporary spawning thread owns the executable's survival.

The implementation requires Linux fork, subreaper and `close_range` support.
Linux custody setup failures do not fall back to unprotected Linux execution.
Monitor readiness precedes Starting; per-command subreaper/group setup can fail
later during spawn and must take the fenced failure-finalization path.
Native macOS/Windows runtime launches retain their pre-custody launch/recovery
boundary: registration does not request this Linux primitive or register a proof.
No independent-tree custody guarantee is claimed for those platforms. The core
primitive itself still returns Unsupported there if directly requested.

## Durable evidence and recovery

PID-sidecar migration 17 adds `runtime_generation_custody`. Generation creation
records the proof path/device/inode atomically with generation/admission fencing.
Quiescence must be read from that exact regular inode. Missing, replaced,
unreadable or incomplete evidence is unknown. No proof files are reused, replaced
or garbage-collected by this implementation.

Within the same Linux boot, global Starting reconciliation and same-session
recovery require custody proof and conservative creator PID/start observation. Recovery
also revalidates within its mutation transaction. Protocol-bearing Running rows
retain the proof requirement if their proxy has died. Late publication cannot
resurrect a recovered generation.

An independent cessation certificate is available after a host reboot: the
host-local sidecar's recorded Linux kernel boot UUID and the current authoritative
kernel boot UUID must both be valid v4 UUIDs and differ. Any recorded child boot
must agree with its creator's epoch. The old kernel's entire process population
has ceased; missing unpublished PIDs are immaterial to that certificate. Both
recovery consumers and transactional RecoveredDead validation use it. This does
not write Q or assert that historical custody ever completed successfully.
Missing/malformed boot evidence, the same boot, and macOS/Windows absolute-start
sentinels do not supply this certificate. Cross-host copied databases and
independently virtualized procfs boot-ID views are outside this host-local identity
contract; differing IDs in those environments alone do not prove remote death.

Finalization revokes future context configuration and awaits affirmative proof;
already-configured commands retain their endpoint until dropped. Thus even a
reusable configured command prevents premature quiescence. Failure to prove
quiescence withholds settlement, not fabricates success. Receipt ACK remains a
separate concern.

Legacy Linux Starting rows have no prospective proof and remain unknown within
the same boot, including on schema upgrade. No automatic backfill or creator-death
clearance is supplied. An authoritative ended-host-boot certificate is independent
of that missing proof. All Linux Starting rows block global materialization, including rows without an
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

## Correction limits and unresolved topology

The supervisor keeps its 50ms polling cadence after both output drains disconnect;
EOF does not establish proxy/tree exit. Configuration no longer allocates a new
FD after Starting. Configuration errors and errors from the post-registration
private barrier attempt fenced StartupFailed finalization and retain any cleanup
failure diagnostic. General resource/storage failure can still prevent durable
finalization; no successful mutation is promised under unavailable storage or
process-wide exhaustion.

Resident costs are not solved by the descriptor reduction: one monitor M and its
wait thread per generation, plus P and C per active command, remain. Ten thousand
one hundred simultaneously active one-command generations imply 30,300 extra
processes and 10,100 extra monitor-wait threads over direct W children, excluding
creators/drains/provider infrastructure. This is a source-derived task count, not
a memory/throughput measurement or an established concurrency requirement.

Potential reductions require distinct choices, not deletion of custody owners:

- A process-local pidfd/epoll wait reactor can replace per-M wait threads while
  retaining exact consuming wait ownership. It saves threads only where multiple
  generations share a creator, adds pidfd/reactor lifetime handling, and must not
  steal another subsystem's children with waitpid(-1).
- A cross-creator broker could share monitoring/reaping, but requires a real
  pre-spawn authority-transfer protocol and an ancestor/adoption arrangement.
  Existing sibling M cannot certify C's children using its own ECHILD.
- Removing P requires replacing the std::process::Child/status/group contract;
  removing C without replacing its cancellation-independent tree ownership loses
  proof on ordinary killpg. A native-W handle plus separate custody completion is
  a viable API redesign, not a transparent one-process optimization.
- A delegated cgroup-v2 subtree can provide kernel membership/cessation evidence
  without per-command P/C, but needs deployment authority, pre-exec membership,
  escape prevention, exact subtree identity, and explicit wait/reap ownership.
  Unsupported Linux environments must not silently fall back to weaker custody.

The candidate keeps global Starting serialization. Known-session quarantine could
preserve that session's non-overlap while admitting unrelated sessions; adopting
it requires deciding whether global materialization represents a protected
single-producer/global-resource invariant or only session overlap. Sessionless
unpublished work needs an explicit affected-authority boundary or stays globally
uncertain. No count cap, age expiry, assumed missing-proof clearance, or silent
change of that admission invariant is implemented. Existing Running sessions are
not automatically stopped by this predicate. Root must decide the topology and
availability scope before treating this candidate as complete.
