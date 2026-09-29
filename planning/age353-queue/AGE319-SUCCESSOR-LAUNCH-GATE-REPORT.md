# AGE-319 Broker-owned installed successor launch gate

## Disposition

The selected-W successor now has a featureless, Broker-owned launch and
approval path in a disposable installed pair. The original D-bound Runner
chooses one immutable offer ID for its exact retained W, asks Broker to start
one candidate, reads that candidate's offer, and approves the already landed
cross-store admission. A distinct Broker child Runner receives F, writes its
own receipt from F bytes, certifies and ACKs it. The installed terminal joins
that successor ACK and owner close. The candidate does not inherit the
original attachment, and a durable start does not itself admit or deliver W.

This is not host-root L admission evidence. The real non-root launcher/root
Broker proof is still absent, so the host-root owner guard remains closed.
The original-exits-before-approval replacement case also remains closed.

## Launch and authority boundary

- Broker keeps `installed-successor-starts/<offer>.json` and
  `installed-successor-candidates/<offer>.json` under private root-owned State.
  The start is create-new and synced before spawn. It records the exact
  selected-W decision, D, pair/source generations, original process stamp and
  owner UID/GID. A failed or uncertain spawn consumes the start; no retry may
  mint a second candidate. Broker validates the ledgers on restart.
- `StartInstalledSuccessor` is restricted to the live original D-bound peer.
  Broker checks the selected W against the released root, exact pending-F
  notify terminal, absent original F/receipt and successor ACK, root admission
  fence, installed pair/source, peer UID and pinned Runner image. Host UID 0
  cannot pass this gate outside the disposable private-root fixture.
- Broker executes the fixed pinned Runner image as the original owner UID/GID
  and records the distinct child process before issuing a one-use connected
  descriptor. Candidate entry checks the live Broker socket peer and parent,
  installed pair/source, offer/D/W/root identity, UID and its own pinned
  process before it can offer on a fresh socket. Direct invocation and copied
  environment or descriptor values have no grant.
- Offer consumes the live descriptor proof. Admission rechecks the original
  D/root/selected-W offer and live Broker-held candidate. Successor F,
  receipt certification and ACK require that same live offered process.
  Durable readbacks handle uncertain replies without reopening launch
  authority. After ACK Broker reaps the child; restart may read the immutable
  start/candidate and terminal but cannot recreate their live gate.
- The original normal async path still sends its own F/receipt/ACK when no
  successor is selected. The disposable test selects the successor path with
  a private `successor-mode` marker; general scheduling policy is not changed
  by this slice.
- Fresh Bash notification repair now accepts the same exact certified
  successor ACK that terminal readback uses for a delivered row. Before this
  fix, restart repair required an original F grant even after successor ACK.

## Executed proof

The default-disabled four-image test uses disposable private-root Broker,
launcher, Runner and read-only-built Bash images. Its successor case exits the
initial provider before releasing held child W, keeps the original Runner
alive, and checks one exact decision, start, distinct candidate and offer.
It refuses wrong and duplicate start/approval controls, confirms candidate
start has no admission or delivery authority, then checks successor F/receipt/
ACK in the installed terminal and durable readback after Broker restart. The
fixture discards the start reply and recovers the same candidate by readback.
The original-delivery case retains busy-original, sync, lost F/ACK reply,
duplicate, restart, receipt-tamper and later-entry regressions. The private
successor test remains the cross-store admission regression; it is not used
as production entry proof.

Verification:

- `cargo check --locked -p oulipoly-kernel-broker -p oulipoly-agent-runner --no-default-features -q` and the final featureless Runner build passed. The targeted successor ledger unit test passed 1/1.
- The full default-disabled `age319_featureless_installed` test passed 1/1 with both disposable cases: original delivery in 208.06 seconds and distinct successor in 161.54 seconds. This run preceded the added lost-start-reply injection and the restart repair adjustment.
- The final successor-only run of the same four-image test, with the start reply discarded, passed 1/1 in 164.40 seconds. It reached exact successor ACK, installed terminal/owner close, and Broker restart readback with one start and one candidate.
- The final private `age319_successor_admission` `durable_exact_successor` selection passed 2/2. `cargo fmt --all -- --check` and `git diff --check` passed.

An exploratory lost-start-reply run reached the installed successor ACK and
terminal but timed out waiting for Broker restart. Startup repair reported
"fresh Bash row has no exact ACK provenance" because it looked only for an
original grant. The State repair change above was made from that failure;
the final run passed the same restart check. Existing unused-code warnings
remain in the workspace build. Bash was built from its read-only trunk with
`CARGO_TARGET_DIR` inside this worktree.

## Remaining boundary

The next transition is a real non-root launcher to root Broker installed
admission proof before opening host-root L. A separate recovery design is
needed if the original exits before candidate approval; neither a spent
start nor a recorded candidate grants replacement or F by itself. No Bash
trunk, host installation, host State, service or alias was changed.
