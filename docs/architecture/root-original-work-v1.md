# Root original-work v1

`original-work-v1` extends the existing Linux completion guardian and
`RootSupervisor`. It does not add a service, socket, election, `WorkOwner`, or
database recovery loop. Completion continuation keeps its v2 records and
meaning; original work uses a separate protocol on the already inherited
`OULIPOLY_COMPLETION_ENDPOINT`.

## Topology and authority

A runner entering without an endpoint wins or joins the existing guardian
election. The guardian issues a `root-authority-v1` grant containing the exact
completion domain, stable supervisor authority, root id, guardian and root
process incarnations, and a random capability. A fresh join and an inherited
join are different tagged messages and cannot be combined. Joins must come from
the same executable image as the guardian when creating a fresh root.
Inheritance instead requires the exact grant and an exact live descendant of a
registered root context or of an active root-owned worker; this admits the
descriptor-pinned runner copy used by completion registration without reducing
the boundary to UID or socket possession.

The grant, endpoint, and `OULIPOLY_ORIGINAL_WORK_REQUIRED_V1=1` marker are
inherited through the provider tree. The marker distinguishes a current paired
tree from legacy completion-continuation-v2 callers that legitimately inherit
only an endpoint. Before each provider launch, Runner joins a private Linux
session keyring named `oulipoly-paired-original-work-v1:<random-v4-uuid>`;
before exec of each accepted root-owned Bash worker, the guardian installs the
same kind of ring in that child alone. Both paths link the preceding session
ring into the new one so inherited provider keys remain searchable. The ring
survives fork, exec, setsid, and reparenting. For an adopted worker descendant,
agent-bash also checks its live ancestor chain for the guardian's bound private
owner socket. That veto survives ordinary session-keyring reset while the
guardian lives. Neither marker grants paired work. A descendant of the
entry/provider branch can be adopted outside the guardian's tree; after a
keyring reset and loss of environment markers, this branch has no durable
lineage witness in the current topology. Full negative admission under that
order remains unresolved.
In a marked tree, agent-bash fails closed if either endpoint or grant is
missing or invalid. A genuine standalone caller with no paired lineage keeps
the existing behavior; an endpoint-only legacy caller also keeps the existing
completion-continuation-v2 topology. A grant without the endpoint is always
malformed paired context and fails closed.

```text
runner entry/provider tree
        |
        | root-authority-v1 (fresh or inherited)
        v
existing completion guardian + one endpoint
        |
        +-- existing RootSupervisor completion-continuation-v2 operations
        |
        `-- original-work-v1 operation
                |
                `-- root-owned agent-bash worker
                        `-- workload tree
```

The worker performs output capture and the existing completion publication,
but it has no election, endpoint, replay, or root acceptance authority. It is
an exact child of the root guardian in a distinct session. The guardian retains
its `Child`, exact Linux process identity, control channel, causal children,
cancellation duty, wait status, and final drain duty. This replaces the
per-request agent-bash guardian/supervisor pair only for a paired v1 request.

## Intent, acceptance, and grant

Agent-bash creates its normal private handle directory and delivery-helper
snapshot, then atomically persists `root-work-intent-v1.json`. It opens and
passes pinned descriptors for its executable, intent, cwd, and state directory.
The guardian checks:

- protocol, domain, authority, root capability, and exact peer incarnation;
- root or nested registration shape;
- nested peer descent from the exact active parent worker and that parent's
  private per-work child capability;
- intent digest and identity;
- executable identity against the submitting process image; and
- intent, cwd, and state-directory device/inode identity.

For a nested request, the guardian enters the child id into the parent's causal
set before exclusive acceptance. It then creates and fsyncs
`root-work-accepted-v1.json` with `O_EXCL`. Only that transition makes effects
possible. An existing acceptance never authorizes launch replay. A missing or
lost response is reported as outcome-unknown and agent-bash does not resubmit.

Acceptance does not grant execution. The worker registers the existing
completion-continuation operation and sends `P` (prepared). Only the guardian
can answer `G` (execution grant). A durable cancellation already admitted in
the guardian pass answers `C` or `O` instead, so cancellation before grant has
zero workload dispatch. There are no time-based startup or read decisions;
state transitions, EOF, process identity, cancellation, and terminal evidence
drive the protocol.

The accepting client socket is retained by the root until `P`. This preserves
the existing rule that agent-bash does not report a successful start before its
completion-continuation registration is admitted. Durable acceptance and root
ownership still precede `P`, so client loss during registration cannot erase or
replay the work. Registration failure after acceptance returns
`effects_possible_no_replay` and is settled by the same root operation.

The guardian creates a separate 256-bit child capability for each accepted
operation. It is passed to that worker through a CLOEXEC descriptor, is exposed
to the workload environment only after `G`, and is required with the exact
parent work id for nested acceptance. Acceptance evidence stores only its
SHA-256 digest. Thus a root-wide grant or knowledge of another work id cannot
move a child to a different causal parent.

## Causal settlement and cancellation

A worker sends `R` when its own terminal proposal is ready. The guardian closes
that operation's child-admission window and sends `S` only after every admitted
child has produced its causal terminal outcome. The worker then publishes its
normal agent-bash terminal/completion event and sends `T`. Notification ACK is
not `T`: completion-continuation-v2 delivery continues under its existing
claims and evidence.

Explicit cancellation carries the exact root id, work id, per-work cancel
capability, and authenticated requesting process incarnation. It is accepted
only after the guardian exclusively creates and syncs
`root-work-cancel-v1.json`, including the actual cause and requester. A failed
receipt write leaves cancellation pending and has no process effect; bounded
per-operation backoff retries that same obligation. A different requester or
cause cannot be relabeled as the accepted one. Owner-exit, causal-parent, and
exceptional living-guardian control-loss cancellation use the same durable
root path and propagate recursively. A grant write failure or missing accepted
worker identity remains a pending receipt until the exact cause is durable; it
cannot issue another grant, cancel control, session signal, or terminal result
meanwhile. Before grant the worker publishes a no-launch cancellation result
and its current-v2 source observation when one was registered. After grant it uses the existing
supervisor cancellation machinery. If the control worker is lost, the guardian
signals the exact worker session and retains the operation until wait plus
session drain. If the guardian is lost, EOF makes the worker the last custodian;
it cancels and drains instead of continuing unowned.

The guardian writes `root-work-result-v1.json` only after it has observed the
exact worker wait, no process remains in that worker session, no unattributed
direct adopted child remains under the guardian, and every accepted causal
child has its own durable terminal root result and physical drain. The direct
child gate is conservative across concurrent operations: an unrelated adopted
child may delay a result, and cancellation cannot safely signal a child whose
exact work attribution was lost. That debt remains live until the child exits;
the gate does not claim exact per-work cancellation custody. This
also applies when the parent worker exits before `T`. The receipt
contains the causal child ids, physical-drain fact, worker wait status, and the
agent-bash terminal projection. A private per-operation result nonce makes an
existing conflicting artifact a terminal-integration failure rather than an
idempotent success. The guardian cannot retire while any original or completion
operation remains.

## Diagnostics and retention

Root acceptance, execution grant, cancellation, and terminal integration use
the existing DB-independent process recorder with root, supervisor authority,
work correlation, initiator, and worker identity where available. Agent-bash
also appends `root-work-diagnostic-v1.jsonl` in the handle directory for intent,
transport, peer, ambiguous acceptance, response, and cancellation phases. A
handle-local lock serializes the journal; it rotates once at 1 MiB and caps a
record at 128 KiB. A database failure therefore cannot remove the only protocol
witness. Result and cancellation persistence retries are individually scheduled,
so a failed durable write is not repeated on every guardian pass. The guardian
still visits the active operation vectors on its polling cadence to check which
obligations are due; capacity under multiple persistent failures is unmeasured.

Intent, acceptance, cancellation, result, JSONL diagnostics, its single rotated
generation, and its lock are children of the existing agent-bash handle
directory. They introduce no SQLite table or live query. An accepted paired
handle is retained until agent-bash has durably validated its accepted intent,
completion snapshot, and terminal outcome and written the separate immutable
`source-retention-release-v1.json` record. This does not change any
completion-continuation-v2 record or meaning. After release, the existing TTL,
delivery, and physical-custody gates apply. A nested handle is additionally
retained while its exact parent handle exists, so a live parent relationship
cannot be erased. The Bash reaper must gate an accepted root handle on the
exact durable `root-work-result-v1.json` before removal; source release alone
cannot prove the runner's result obligation settled. A root result follows its
accepted child results. This is a cross-repository
invariant requiring exact-pair verification. Root
authorities and their endpoint-local artifacts retire
only after no live root context or active original-work operation references
them. No startup or live loop enumerates these files globally.

## Version boundary

The paired wire versions are exactly `root-authority-v1` and
`original-work-v1`. They do not understand the retired AGE-319 notification
migrations, schema ordinals 18/19, enqueue/progress/consume commands, recovery
loop, or owner endpoint. Protocol or capability mismatch is a rejection, never
a fallback or replay grant.
