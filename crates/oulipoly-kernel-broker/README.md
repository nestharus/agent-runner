# Kernel broker source slice (AGE-319)

This Linux-only broker and Runner entry are uninstalled, opt-in source. The
`OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1` path reads an existing State/mailbox
domain without mutation, reserves a broker root, pins its exact direct-child
host guardian, and waits for native owner publication and broker readback before
releasing the recovery driver. It then requests a single-use root child join
only for CLI help or offline diagnostics. Provider/recovery CLI, GUI and TTY
entry refuse before broker reservation because native service identity still
uses host and namespace-local PIDs interchangeably.
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
Other operations reject all passed descriptors. A bare UUID or environment
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
host guardian, and releases the gate. The child validates its broker socket
peer, namespace placement, exact durable native owner/root binding and live
guardian before CLI dispatch. PID1 reaps adopted descendants while the
original Runner runs, reports its exit through the receipt, and stays live
afterward. The host guardian stays outside the root namespace. There is no
arbitrary five-second launch lifetime cutoff and no new `no_new_privs` or
seccomp on this path.

Entry and root records remain durable debt. Broker restart reattaches only to
the same live PID1 incarnation; a vanished or changed PID1 remains uncertain.
A consumed join never replays after an uncertain response, failed fork or
restart. The current source has no safe retirement or root drain protocol, so
it conservatively blocks another reservation once a join is consumed.

The existing `works/` registry can classify exact nested work namespaces, but
there is no guardian-authorized socket operation that launches accepted work.
Allocated attempts retain their existing `no_new_privs`/seccomp. The broker's
`SO_PEERCRED` plus `SCM_CREDENTIALS` comparison excludes the private tested
transferred-socket child PID namespace case; installation still needs a
privileged host-path security review. The private fixture is a source/protocol
test, not a host-root sudo or deployed continuity proof.

## Remaining interfaces

- Migrate native completion and provider custody to explicit host/local PID
  domains before admitting service-requiring CLI. The private `--model` probe
  reached the child but failed in current native identity handling; it now
  refuses before reservation. Add a validated TTY and GUI handoff or keep
  those modes refusing. Current supported entry is help/offline diagnostics.
- Add a broker-authenticated positive accepted-work grant from the host
  guardian, a one-use nested PID namespace/PID1 launch, physical drain and
  settlement receipts. A work ID or `inside root` classification cannot grant
  execution. Replace allocated-attempt NNP/seccomp only with that custody.
- Reconcile host/local PID fields, adopted descendants, result ACK versus
  physical drain, and registry retirement across broker and WSL restart.
- Reconcile this branch's sidecar v24 with AGE-353's separate v24 migration
  before deployment. Run authorized privileged host sudo and paired Bash
  acceptance; the private userns fixture does not establish either.
