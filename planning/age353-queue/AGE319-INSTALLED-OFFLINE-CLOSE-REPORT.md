# AGE-319 installed offline CLI close — 2026-09-28

## Result

An installed, connected `--help` root now returns the original launcher's exit
0 only after a durable `installed-offline-terminal-v1` certificate is published
and reread. The certificate binds the immutable L request, actual control wait
and E stamp, released U/D and exact invocation actor, the returned State root
effect, a zero-effect physical proof, and the old-side
closed-owner proof. Each status read recomputes this join, including after a
Broker restart; changed or missing evidence returns unknown. Supported
`diagnostics` uses the same offline intent route, including a nonzero exit
when State says `returned_failure` and the exact control wait agrees. No
provider result is synthesized.

The route requires no provider K, no source grant or accepted source effect,
no State child C/W or Broker child work or native grant, and no uncertain
registry readback. It fences
admission before requesting PID1 drain, then requires PID1's ECHILD receipt,
the parent wait, the original child gone, owner-close intent, and the State and
sidecar closed phase. The old sidecar now recognizes the zero-source offline
proof as a separate case; its normal K/Q and one-source cases retain their
existing acceptance rules. Normal model certification still requires K, Q,
publication, caller settlement, and the same physical close.
The next E admission revalidates the prior offline caller certificate against
the same physical and owner proof.

## Verification

- Root-mapped connected fixture: 8/8 passed. The `--help` case observed the
  actual launcher exit 0, empty provider/source/work inventories, admission
  fence, PID1 ECHILD and parent wait, owner-close intent and closed proof.
  `diagnostics --help` also exited 0 through the same no-effect close route.
  Restart readback delivered exit 0. A changed offline terminal certificate
  made the original pinned launcher exit 70 with unknown. Normal model exit,
  missing-Q refusal, changed-Q refusal, and normal restart cases passed.
- Focused broker drain tests reject absent/unfinished offline evidence and
  mixed grant, native, work, source, or uncertain records. The State sidecar
  test rejects mixed normal/offline proofs, unfinished return, nonzero work,
  and accepted source effects.
- Featureless paired Runner/Broker build and focused featureless tests passed.
  `cargo fmt --all --check` and `git diff --check` passed.

## Remaining limits and review debt

The connected fixture is a disposable root-mapped install using the private
test transport. It does not exercise a fixed-path host install, real host
State/service/aliases, the separate Bash repository, a real provider API, or
a nonzero installed diagnostics case. Sustained AGE-353 stress and benchmark remain
for the later pass. The existing healthy-pending wait has no fixed deadline;
socket loss and changed evidence remain explicit unknown. No host or ticket
mutation was performed.

The mandatory nine-domain CRW observation and corrective revisit did not run:
this assignment prohibited subagents. That review debt remains for the parent
before merge disposition. This branch is committed and pushed without a PR or
merge here.
