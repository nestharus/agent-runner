# v30 recipient work grant boundary

The broker-retained root-only sidecar is the sole notification authority after
v30 cutover. `read_bounded_recipient_selection` is a planning read. Its session,
row and digest fields are data, not a bearer grant. The requirements below
describe the full production route. The current source implementation and its
remaining admission gap are recorded at the end.

## Identity and requester

The exact live v30 driver may request selection and a work grant only while
the broker verifies the same source generation, root, running owner, guardian,
joined child and driver process stamp. The driver supplies no session, row,
path, payload or recipient PID. The broker chooses one pending row or a bounded
contiguous batch from its retained sidecar and verifies every retained payload
against its recorded digest and length. It checks State projection and the
sidecar generation before and inside the grant transaction.

A grant must also bind the original session's recipient identity. For a live
PTY, the broker must resolve one running runtime generation for the selected
session and pin its actual process and control endpoint to the root-owned
provider tree. For a headless wake, it must bind the original session to an
admitted exact wake successor and its child identity; a session string or
copied wake claim cannot establish this. No caller-selected path, PID, session
or row may substitute for these observations. If the recipient is absent,
ambiguous, dead, or outside the pinned tree, grant creation refuses.

## Durable grant and one use

The retained sidecar transaction records a broker-generated grant ID, source
generation, root ID, owner generation, driver identity, original session ID,
recipient process or exact wake-successor identity, selected row sequence and
payload identities, and a state of `reserved`. It also records the intended
transport and its pinned provider K identity. A unique active-grant constraint
per selected row and a small fixed maximum of outstanding grants per root bound
debt. The broker never silently replaces an active grant or scans unbounded
history to choose work. A grant is not delivery, consumption, or ACK.

Only the authenticated bound recipient or exact wake successor may consume
the grant. Consumption is one-way and atomic with recording the `consuming`
state before an irreversible transport action. The broker checks the live
process stamp, root tree, current sidecar generation, row identity and payload
bytes again at this boundary. A second consume, sibling, stale generation,
copied payload, or retired sidecar is refused. There is no arbitrary short
expiry: liveness and exact terminal evidence govern debt.

The live driver may cancel an unconsumed grant only after the broker proves no
provider K or delivery submission began. A proven dead recipient may leave
reserved debt for the exact running successor to reconcile; it must not be
reassigned by guessing that submission failed. A grant in `consuming` is
uncertain until a transport receipt or a physical no-start proof settles it.
The broker never retries an uncertain K or transport request just because a
reply was lost. This is necessary to prevent duplicate work.

## Readback and restart

The driver reads back by the broker-minted grant ID and its original root,
source and owner selectors after a lost reply. Readback returns the durable
state, exact selected rows and whether K is known started, known not started,
or unknown. The broker reopens these records on restart and reattaches only
processes whose boot ID, PID, start time, namespace, executable and root-tree
membership still match. An unknown K or unknown transport remains debt and
blocks a second grant for the row. A dead driver does not grant a sibling
driver permission to consume; succession needs a separately broker-proved
owner transition and exact durable handoff.

Transport handoff, source-retention release, recipient consumption, explicit
recipient ACK, Runner terminal, and physical Q are distinct facts. A PTY
control response can prove submission only to the extent its recorded nonce
and actual control peer permit. A headless spawn can prove only the spawn
boundary until the child attests its exact claim and consumes work. Neither
proves recipient ACK. Manual ACK requires a broker endpoint that authenticates
the actual recipient/session or an explicit delegated exact batch grant;
`--delivered-by` remains an audit label. Read-only human lookup may remain
available without new recipient authentication.

## Current source implementation and missing capability

The fresh socket now implements `F` requests for a recipient-bound submit,
exact request-ID readback, token ACK, explicit exact-ID batch delegation and
delegated ACK, plus lane-qualified local payload lookup. The broker derives
the recipient's process stamp from the challenged socket peer. It selects an
ungranted pending row, verifies the retained payload bytes, and requires a
sidecar source/attempt/row mapping equal to an accepted source in fresh State,
plus an exact recipient process attachment in both State and sidecar.
One grant per row is committed as `unknown` before the socket write. A full
write records `submitted`; both states remain unacknowledged until a valid
owner token or one-use delegated batch ACK commits. A lost reply is read back
by its persisted delivery request UUID without exposing the ACK token. An
explicit recovery request for that UUID returns the same verified bytes and
token to the exact recipient without minting another grant. Local lookup uses
exact lane, session and row identity, does not expose an ACK token, and does
not change ACK state. No 30-day timer resolves pending rows.

Delegation currently accepts 1–32 exact grant IDs per batch. The private
fixture proves noncontiguous batch ACK, but there is no pagination or
aggregate delegation flow for an arbitrarily large backlog. This remains a
mass-ACK usability gate; the bound must not be presented as unlimited ACK.

There is **no production writer** for `fresh_lane_accepted_source`,
`fresh_lane_recipient_attachment`, `fresh_recipient_source`,
`fresh_recipient_row_source`, or the recipient process binding. The private
executable fixture inserts synthetic accepted rows and tests the real broker
socket and storage transitions. Consequently
ordinary v30 entry cannot submit a positive recipient delivery on this
branch. The binding record also does not yet prove that a live PTY/provider or
headless wake successor belongs to the pinned root tree. The broker's native
K handler remains closed; v30 has no pinned provider K or broker-owned PTY or
headless wake transport route. A socket write proves submission to that
connected process only, never provider consumption or ACK. Source acceptance,
physical K/Q, Runner result and recipient ACK remain separate facts.

Legacy `mailbox_delivery` and `wake_coordinator` act on the user sidecar and
cannot consume this protocol. Existing v29 recipient/session/ACK continue on
their old route; an unqualified fresh-shaped session is refused there. The
installed Runner entry remains closed pending source, runtime binding and
provider transport integration. Unrestricted host sudo is unchanged.
