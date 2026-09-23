# Kernel broker source slice (AGE-319)

This Linux-only broker and Runner entry are uninstalled, opt-in source. The
`OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1` path reads an existing State/mailbox
domain without mutation, reserves a broker root, pins its exact direct-child
host guardian, and waits for native owner publication and broker readback before
releasing the recovery driver. It then requests a single-use root child join
only for CLI help or offline diagnostics. Provider/recovery CLI, GUI and TTY
entry refuse before broker reservation because native service identity still
uses host and namespace-local PIDs interchangeably beyond the owner handshake.
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
can carry its exact accepted grant ID. `H` only prepares durable debt. A pinned
guardian records the accepted request as never forked after `H`; it does not
enter the legacy direct `Command::spawn` path. There is still **no nested
launch, worker gate, source/ACK or physical-drain receipt**. The broker does
not treat a prepared record as execution or drain authority.
Allocated attempts retain their existing `no_new_privs`/seccomp. The broker's
`SO_PEERCRED` plus `SCM_CREDENTIALS` comparison excludes the private tested
transferred-socket child PID namespace case; installation still needs a
privileged host-path security review. The private fixture is a source/protocol
test, not a host-root sudo or deployed continuity proof.

## Remaining interfaces

- The merged State/runtime/Runner PID readers translate namespace-local PIDs
  into the caller's procfs observer and fail closed on changed or ambiguous
  identity. The observer tag in the child environment is a drift check, not
  broker authority. This root join creates a PID namespace without a separate
  mount namespace, and broker and guardian identity reads still use `/proc`
  paths. A workload with host-root `sudo` could replace that mount; the current
  source does not authenticate the host-procfs invariant against that case.
  Before service admission, bind broker identity reads to a pinned host procfs
  observer that the workload cannot replace, and bind the guardian and nested
  worker's host PID identities to broker-attested incarnations. A private
  procfs fixture proves translation mechanics only. Keep service-requiring
  CLI, TTY, and GUI refusing until their remaining bindings exist. Current
  supported entry is help/offline diagnostics.
- Extend the positive `H` preparation into a one-use consumed grant and
  broker-owned gated worker launch under a nested PID namespace/PID1. Transfer
  the worker control and capability descriptors as part of that atomic
  interface. Replace the guardian's local `Child`, session cancellation, and
  session liveness drain model with a broker/PID1 control handle and physical
  namespace-drain receipt, tied to source/ACK obligations. The unpinned
  guardian still uses the legacy direct spawn; the pinned path cannot run work.
  A work ID or `inside root` classification cannot grant execution. Replace
  allocated-attempt NNP/seccomp only with that custody.
- Reconcile host/local PID fields, adopted descendants, result ACK versus
  physical drain, and registry retirement across broker and WSL restart.
- Reconcile this branch's sidecar v24 with AGE-353's separate v24 migration
  before deployment. Run authorized privileged host sudo and paired Bash
  acceptance; the private userns fixture does not establish either.
