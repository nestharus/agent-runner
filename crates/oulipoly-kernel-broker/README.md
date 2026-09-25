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

## Private fresh provider execution

The private `^` request is a nonactivating interactive PTY handoff substrate.
Its private client accepts read/write master and slave descriptors with the
original released Runner's D key, session, selected account and plan digest.
The broker compares them with the released root and current headless broker route selection,
uses `TIOCGPTN` and `TIOCGPTPEER` to verify the real PTY pair, and creates an
exact `pre-k-nonactivating` artifact. Repeating the same pair is readback;
presenting another pair or any request after a provider K grant refuses. The
artifact does not hold a descriptor across restart and is never consumed by K,
Q, runtime generation registration or F. It cannot certify PTY liveness after
the challenged request ends.

The remaining work must first extend source-selected h/f plans to interactive
provider mode. Physical interactive K must then transfer that verified slave
into the broker's one-use selected launch, give the child a controlling
terminal, keep the master and control server with the original root, and
attest the broker's actual child incarnation into the fresh sidecar. The
ordinary runtime generation registrar accepts only a local direct child;
it cannot register the broker-forked provider. Until that broker-owned
registration and cross-namespace observer proof exist, no live generation or
native F claim follows from `^`.

The closed private route assembles the configured first command, arguments,
environment, cwd and stdin through the normal runtime launch path. A bare
first command is resolved against that child's effective `PATH` for each
attempt. The broker compares the passed descriptor with its own open of the
resolved path, binds that mount and inode and the sealed recipe to one-use K,
and invokes `execveat` on the original descriptor. The host kernel applies its
normal shebang, set-ID, file-capability and mount policy. User-owned and
mutable executable images are eligible. A recorded preflight hash, metadata
and xattr digest are observations; they do not certify bytes or metadata at
the later syscall. Path replacement leaves a prepared descriptor on its
original inode; an in-place update may change the outcome or cause an OS exec
error, which is recorded as provider exit and physical Q. An uncertain K or Q
remains explicit debt for caller-owned recovery, with no automatic replay.

This is source and private user-namespace evidence only. The selector stays
closed, and installed host sudo behavior is unproved.

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
The host entry includes the exact root capability it read back from its pinned
guardian as a separate J field. The broker checks its root/domain/supervisor
binding and supplies it to the joined child outside the caller-selected
environment. The child must still prove that capability to the guardian.
`V` takes the joined child's connected native-owner socket and an explicit
host-PID/starttime/boot witness from the durable owner. The host broker checks
the exact fsynced joined-child incarnation, root PID1 ancestry, root namespace,
guardian and driver incarnations, and the socket's host-side `SO_PEERCRED`.
It gives no launch authority and refuses missing older child stamps, changed
identities, and unrelated sockets. Other operations reject passed descriptors.
When H receives an `original-work-v1` intent with Bash's handle-bound
`delivery-helper` provenance, owner session/invocation, and the native one-use
registration authority, it records a **v3**
grant. H opens that exact sealed image through the accepted state-directory
descriptor and requires its inode and SHA-256 to match the intent and the
broker's fixed Runner image. The existing H and K wire frames are unchanged.
The existing V frame has optional owner-generation, session, invocation, and
registration-authority digest claims. A V caller that is not the joined child
must present the exact authority digest from the accepted intent and is
admitted only when the
broker classifies its live PID incarnation inside the exact work namespace,
finds that work's **consumed** v3 grant, rechecks its root/guardian/joined-child
bindings, and matches the caller's executable inode and bytes to the pinned
helper. The normal owner socket and live guardian/driver checks still apply.
Missing claims, old v2 grants, a different work or image, and incomplete work
placement refuse. New brokers read v2 and v3 grants; old brokers refuse v3 on
restart, so upgrading this broker is a versioned on-disk compatibility step.
The accepted intent binds the session and invocation to that authority. The
helper must present both exact owner claims, including after broker restart.
An intent with any native owner field but an incomplete helper, session,
invocation, or authority set refuses H instead of receiving a v2 grant.
Standalone work with no native owner fields retains the v2 H/K path, including
when it has a delivery-helper snapshot.
The Runner's native registration authority remains the separate one-use
session permission.
The guardian's challenged `B` readback verifies that a new context is the
exact one-use joined Runner child under the recorded root PID1; ordinary PID
ancestry cannot establish that across the broker's namespace fork.
The challenged `S` request accepts a source's already-connected guardian
socket and explicit host PID/boot/starttime witnesses. The broker compares the
request's actual socket credentials with the source witness, checks the bound
guardian's live incarnation and the passed socket's host-observed peer, then
requires the source in the exact root namespace or in a consumed accepted
parent work namespace matching the declared parent work ID. A source in one
work cannot claim another's scope. A separate outside controller can use
`cancel_outside` only for an existing positive H grant; the guardian still
checks its independent cancellation capability. `S` is a read-only
per-connection check; it
does not consume or replace the H/K grant and cannot certify work completion.
The Bash submit and cancel paths must call it on their actual connected socket
when they run in a broker-owned PID namespace; the separate Bash repository
has not yet been changed.
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

The distinct challenged `N` request carries the original guardian's retained
SHA-256 of `native-continuation-accepted-v1.json` and three descriptors: its
directory, `custodian-request.json`, and that receipt. The broker checks its
bound live guardian, root, owner UID, host PID namespace and pinned Runner
image, then verifies both named regular-file inodes and exact receipt/request
bytes. It writes a version 4 `native-continuation-v1` record into the same
`grants/` directory; versions 2 and 3 retain their original-work meaning.
An exact retry returns the same ID after reply loss or restart. A changed
attempt or evidence is refused. The guardian then binds that ID to accepted
revision 2 through State v28 and uses indexed exact readback after an
uncertain CAS. Broker fsync precedes State bind: a crash in between leaves an
unbound, nonlaunchable broker debt; a crash after State commit leaves the one
existing bound pair. N also pins the actual sidecar pathname and inode named
by the guardian's digest-bound request. A distinct challenged lowercase `k`
frame carries the exact grant/root/attempt/owner generation and receipt digest
plus four descriptors: accepted directory, request, receipt, and that sidecar.
Its read-only preflight rechecks the live guardian/root, named bytes and
inodes, and the exact v28 binding at the retained path. A different sidecar
with copied rows refuses. Original-work uppercase K and its seven descriptors
remain unchanged. The native frame has no argv or command; the only planned
entry is the broker's pinned Runner image at `__completion-root-worker-v1`.

Native k always returns `native K fixed Runner attach/release closed`. It does
not consume v4, fork, attach v29, or release a pre-exec gate. Exact retries and
broker restart repeat the same read-only decision; a lost reply cannot create
an unaccounted worker. The old uncalled `consume_native` transition was
removed because it accepted an arbitrary MailboxDb and permanently spent a
grant without a worker recovery protocol. The guardian still retains the
prepared/bound attempt and refuses host-local `Command::spawn`. A dead
original guardian cannot recover an unbound grant without a later recovery
protocol, so that case remains debt rather than a new grant.
The N pathname/inode pin does not prove it was the inode of the guardian's
already-open SQLite connection; State or the guardian must supply that exact
connection provenance before a future K may have effects.
Older prepared v4 records without the new sidecar pin can still replay the
same N grant ID, but native k refuses them as retained debt.

## Fresh v30 domain in the broker process

The default host-root service owns both `control.sock` and, when the separate
empty `v30/` publication already exists at startup, `v30.sock`. The old loop
alone owns the held release gate and mutable root, guardian, driver, work and
physical registries. The fresh listener runs in a separate thread with its
own `FreshV30Lane` and fixed `v30/state.db` and sidecar. Its handler cannot
select old State; the old handler cannot select fresh State. A failed fresh
open closes only the fresh endpoint and logs the failure. The old service may
run without `v30/`. After administrator initialization publishes `v30/`, the
default service must restart to bind the fresh socket. An abandoned exact
`.v30-fresh-<v4 UUID>` directory is recognized during old registry recovery;
arbitrary root entries still refuse restart.

Production fresh U, new D, F and all work effects remain closed until a
durable released-child handoff, real fresh invocation, Bash handle and
registration, one-use W, result and ACK are implemented. Challenged I and
exact d readback remain available. Private fixtures exercise U/D and
recipient protocol mechanics, but cannot authorize production effects. The
retired `--serve-fresh-v30` mode refuses in production; its private fixture
form remains only for an older paired test. The service unit still starts
the default mode. Old v29 rows and pending source, WAL, physical and ACK debt
remain in the independent old stores, with no row import.

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
- Add the Bash-side `S` call before submit and cancel, preserving its local
  same-PID-domain peer check for legacy entry. Build the witness from the
  durable root authority and the caller's host-observed incarnation; map root
  registration to `SourceScope::Root` and nested registration to its exact
  parent work ID. A separate outside cancel controller uses
  `SourceScope::CancelOutside` with the accepted work ID after reading the
  durable root/domain/supervisor/guardian binding; in-root cancellation uses
  its exact root or causal parent scope. Treat broker absence, refusal, or
  ambiguous response as a peer authentication failure without sending either
  request.
- Reconcile host/local PID fields, adopted descendants, result ACK versus
  physical drain, and registry retirement across broker and WSL restart.
- Reconcile this branch's sidecar v24 with AGE-353's separate v24 migration
  before deployment. Run authorized privileged host sudo and paired Bash
  acceptance; the private userns fixture does not establish either.
