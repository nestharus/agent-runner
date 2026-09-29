# AGE-319 production distinct successor: selected-W decision prerequisite

## Disposition

The **original-live sleeping successor is not delivered**. This change is a
safe prerequisite. The featureless installed Runner still receives and ACKs
the selected W as the original D-bound peer. No distinct production successor
is started, and this report does not claim successor receipt, ACK, terminal,
owner-close, or installed-certificate proof for the sleeping case.

The missing boundary is an installed successor launch/select gate. The current
installed entry route requires a Broker-owned connected control or child gate;
the only successor process entry is feature-gated `__age319-*`. Starting a
direct Runner with a copied argument or environment token would not establish
the candidate's exact process, UID, namespace, installed image, or one-use
launch ownership. Opening F/ACK through that route would be unsound. The
original-exit-before-approval path still needs a separate Broker-custodied
recovery design and remains unknown. Host-root admission remains closed until
there is real non-root caller/root Broker proof.

## Delivered prerequisite

State now has an additive immutable
`fresh_bash_wake_successor_decision` record. One selected-W obligation can name
only one offer request ID, and an offer request ID cannot name another W. The
record stores the complete original W obligation and is rejoined on readback
to the selected physical W, original D/J/recipient attachment, notify request,
retained mailbox row and accepted source/attempt. The writer requires the
original peer identity, an exact `notify` terminal at `pending_f`, the selected
row and payload, and an exact retry; a changed offer ID is refused. A decision
is scheduling evidence only. It neither chooses a process nor grants F.

Broker exposes the decision and readback only over the pinned original
D-bound socket, checks its root before writing, and takes the root admission
fence. In the default-disabled featureless path, a successor offer now needs
that durable decision and must agree with the selected W's session, row,
source, attempt, root, owner, original identity and payload. Admission
rechecks the decision. The private successor fixture retains its existing
offer protocol and remains a regression for the pre-existing cross-store
admission/F/ACK behavior; it is not production launch evidence.

The featureless installed test now confirms that ordinary original F/receipt/
ACK does not accidentally create a successor choice, including after Broker
restart. The private connected successor case contains original-peer exact
decision, duplicate and changed-ID assertions before its existing private
offer, but the two attempted runs stopped at earlier provider setup stages;
those assertions have **not** passed in an executed connected run.

## Verification and limits

- `cargo check --locked -p oulipoly-state -p oulipoly-kernel-broker --no-default-features -q` passed. The private-feature Runner/State/Broker check and private Runner build passed. The final featureless Runner/Broker/launcher build passed. Existing unused-code warnings remain.
- Bash was built from the read-only Bash trunk with `CARGO_TARGET_DIR` inside this worktree. No Bash source was changed.
- `cargo test --locked -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_successor_admission durable_exact_successor -- --nocapture --test-threads=1` passed 2/2. This checks the existing private cross-store successor offer and F/ACK regressions, not the new installed decision.
- The final-code featureless four-image disposable-root test passed 1/1 in 213.28 seconds. It preserves the provider-exits-before-held-child-W case, original live process, original F/receipt/ACK, owner close, installed terminal, no duplicate Bash effect, and exact W obligation readback. Its new decision assertions found no successor decision before or after Broker restart, as expected for the unchanged original-delivery path.
- An earlier featureless run reached selected W and original ACK but timed out because the normal physical K/Q grant file was absent during terminal readback; the exact rerun passed. Two private connected successor runs failed before the decision assertion: the first did not observe a normal physical grant, and the second observed the directory but not provider exit before its async child release. The second run's Broker log contained recorder-init warnings for missing `OULIPOLY_DATA_DIR`; the fixture deliberately removes that variable, so the warning is not established as the timeout cause. These failed runs are not counted as decision or production successor evidence.
- `cargo fmt --all -- --check` and `git diff --check` passed.

## Remaining gate

The next work unit must create one durable Broker-owned start/select identity
for the decision's offer ID, launch or select a distinct fixed installed
Runner through an authenticated entry gate, pin and recheck its process,
installed image, UID and namespace, and require the original live D-bound
peer to approve that exact candidate. It then needs a featureless connected
test showing successor F bytes and independent receipt/ACK join the original
terminal, owner close and installed certificate after provider exit before W.
Lost offer/admission/F/ACK replies and Broker restart must read back the same
generation without another launch, effect, or recipient. No part of this
commit authorizes a replacement when the original Runner exits before
approval.

No host installation, host State/service/alias change, extra worktree,
descendant dispatch, PR, merge, or Bash trunk edit was made.
