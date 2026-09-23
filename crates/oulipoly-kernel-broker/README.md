# Kernel broker source slice (AGE-319)

This Linux-only broker and Runner entry are uninstalled, opt-in source. The
`OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1` path reads an existing State/mailbox
domain without mutation, reserves a broker root, pins its exact direct-child
host guardian, and waits for native owner publication and broker readback before
releasing the recovery driver. It then requests a single-use root child join
only for CLI help or offline diagnostics. Provider/recovery CLI, GUI and TTY
entry refuse before broker reservation because the accepted-work execution and
physical-drain boundary is incomplete.
Ordinary Runner and Bash admission and allocated-attempt custody remain active.
This source is not a deployable AGE-319 restoration.

## Installed trust roots and protocol

The source-only unit is `packaging/linux/oulipoly-kernel-broker.service`. The
normal binary requires root in the initial user and PID namespaces, the
root-owned installed broker at `/usr/local/libexec/oulipoly/oulipoly-kernel-broker`,
a root-owned fixed Runner image at
`/usr/local/libexec/oulipoly/oulipoly-agent-runner`, and root-owned state and
runtime directories. It has no installer. `age319-private-broker-fixture`
permits path overrides only for UID 0 inside a noninitial user namespace.
Serving startup must create a fresh detached procfs mount tied to the broker's
PID namespace. Failure stops the broker. Process identity reads use that
descriptor, so replacing the visible `/proc` pathname does not redirect them.
The descriptor is not independently protected from an unrestricted host-root
workload that can inspect broker FDs or memory; this is a verified partial
observer improvement, not service admission authority.

Each Unix stream request carries a broker challenge and credentials checked
against the pinned connector's pidfd, boot ID, starttime and PID namespace.
`C` classifies; `E` reserves an exact outside entry; `P` prepares its direct
host child; `G` binds that guardian, native domain, and supervisor incarnation;
`A` reads back the exact live binding. The old `L` operation stays disabled.
`J` is the new one-use join: a bounded JSON invocation and exactly five
`SCM_RIGHTS` descriptors for stdin, stdout, stderr, cwd, and an exit receipt.
`V` takes the joined child's connected native-owner socket and an explicit
host-PID/starttime/boot witness from the durable owner. The host broker checks
the exact fsynced joined-child incarnation, root PID1 ancestry, root namespace,
guardian and driver incarnations, and the socket's host-side `SO_PEERCRED`.
It gives no launch authority and refuses missing older child stamps, changed
identities, and unrelated sockets. Other operations reject passed descriptors.
A bare UUID or environment
marker grants nothing.

`J` authenticates the original entry and exact root/domain/supervisor/guardian
binding. It validates the help/offline-diagnostics command family, UTF-8
argv/environment, unique safe environment keys,
non-TTY regular/pipe/character standard descriptors, a directory cwd, and a
receipt socket. It rejects provider/recovery and GUI entry, TTY and socket standard streams, dynamic
loader control variables, and internal authority environment keys. The broker
executes only its pinned fixed Runner image under the entry's UID/GID/groups,
with the validated argv and environment; it never accepts a command path.

The broker fsyncs `join_consumed` before forking. It creates a root PID
namespace, fsyncs the identity of its persistent PID1, then permits PID1 to
fork the Runner. The Runner remains at a pre-exec gate until the broker pins
that exact child, verifies direct ancestry, namespace, UID/GID and the live
host guardian, fsyncs the child's exact identity, and releases the gate. The
child validates its broker socket peer, namespace placement, exact durable
native owner/root binding and connected guardian through `V` before CLI
dispatch. PID1 reaps adopted descendants while the
original Runner runs, reports its exit through the receipt, and stays live
afterward. The host guardian stays outside the root namespace. There is no
arbitrary five-second launch lifetime cutoff and no new `no_new_privs` or
seccomp on this path.

Entry and root records remain durable debt. Broker restart reattaches only to
the same live PID1 incarnation; a vanished or changed PID1 remains uncertain.
A consumed join never replays after an uncertain response, failed fork or
restart. The current source has no safe retirement or root drain protocol, so
it conservatively blocks another reservation once a join is consumed.

The `works/` registry classifies exact nested work namespaces. The challenged
`H` request lets the exact bound host guardian submit its fsynced positive
`root-work-accepted-v1.json` after acceptance. It carries the exact agent-bash
initiator executable, intent, cwd, state directory, and accepted receipt
descriptors. The broker pins that executable to the live original initiator;
it does not accept a path or a caller-selected command. It checks the
guardian's in-memory acceptance digest and the live original source
incarnation in the root or causal parent's PID namespace, then records exact
descriptor inode bindings. The `grants/` ledger binds
root PID1, the historical exact joined Runner child, owner generation,
supervisor, work ID, request digest, and a
consumed causal parent's grant and live namespace. It fsyncs prepared and
one-use consumed records; malformed recovery stops the broker. A work record
can carry its exact accepted grant ID. `H` only prepares durable debt. The
challenged `K` operation rechecks those five descriptors and the live accepted
source, fsyncs one-use consumption, enters the exact root or causal parent
namespace, creates one nested PID1, fsyncs its work record, and releases the
fixed worker behind a pre-exec gate. PID1 remains the reaper and writes a
terminal receipt after its last child has been reaped. The challenged `Q`
operation reports `work-drained` only when that receipt matches and the exact
PID1 is observed dead; a missing receipt or failed observer remains uncertain.
Broker restarts can reattach live work or read the retained terminal receipt.
No workload lifetime cutoff, NNP, sandbox, capability reduction, or user
namespace substitution is imposed by `K`.

The pinned guardian still records `H` as never-forked debt; it does **not yet
call `K`**, adopt a broker worker handle, route cancellation, use `Q` for its
result, or settle source/ACK and retirement. Thus the broker's positive launch
is privately exercised through its protocol fixture, not normal paired work
service. A consumed grant or work record alone is never reported as execution
or drain authority. Terminal receipts do not themselves retire grants or work.
Allocated attempts retain their existing `no_new_privs`/seccomp. The broker's
`SO_PEERCRED` plus `SCM_CREDENTIALS` comparison excludes the private tested
transferred-socket child PID namespace case; installation still needs a
privileged host-path proof. The user accepts deliberate malicious host-root
tampering as outside the same-host trust boundary; this source makes no
adversarial containment claim. The private fixture is a source/protocol test,
not a host-root sudo or deployed continuity proof.

## Remaining interfaces

- The merged State/runtime/Runner PID readers translate namespace-local PIDs
  into the caller's procfs observer and fail closed on changed or ambiguous
  identity. The observer tag in the child environment is a drift check, not
  broker authority. The broker now reads identity through a detached procfs
  descriptor and a private fixture verifies that visible `/proc` replacement
  does not change those reads. A private adversarial probe also showed that
  root with `CAP_SYS_PTRACE` can reopen that descriptor through
  `/proc/<broker>/fd` even after the broker sets nondumpable. That deliberate
  host-root tampering is outside the accepted trust boundary. Keep
  service-requiring CLI, TTY, and GUI refusing until the guardian's positive
  work handoff and identity bindings are integrated. Current supported entry
  is help/offline diagnostics.
- Replace the pinned guardian's local `Child`, session cancellation, and
  session liveness drain model with the broker `K`/`Q` worker handle. Add
  durable cancellation and worker terminal/result forwarding tied to exact
  source/ACK obligations. The unpinned guardian still uses legacy direct
  spawn; the pinned path still cannot run normal work. A work ID or `inside
  root` classification cannot grant execution. Replace allocated-attempt
  NNP/seccomp only when the complete custody path is paired and verified.
- Reconcile host/local PID fields, adopted descendants, result ACK versus
  physical drain, and registry retirement across broker and WSL restart.
- Reconcile this branch's sidecar v24 with AGE-353's separate v24 migration
  before deployment. Run authorized privileged host sudo and paired Bash
  acceptance; the private userns fixture does not establish either.
