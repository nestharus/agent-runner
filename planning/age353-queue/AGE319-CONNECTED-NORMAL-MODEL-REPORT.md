# AGE-319 connected normal model — 2026-09-28

## Result

The root-mapped schema-2 first-install fixture now launches an ordinary
`--model fixture-model "hello fixture"` through the activated installed
launcher and connected Broker. Its disposable `providers.toml` names one local
shell provider. The child uses the normal model selection, plan, admission,
physical K/Q, and caller publication path; it does not call the older private
provider effect route or any live provider API.

The fixture observed the exact L pair/source/root, one E and joined child,
U/D for that child, one State K row and one provider marker line, a Q with
exit status 0 and `tree_drained=true`, exact provider stdout/stderr bytes,
and a settled caller publication. The Broker retained the actual connected
control wait at code 0 with the same L request, pair/source, launcher, root,
and E process stamp. Root-drain readback showed a settled entry, PID1 ECHILD,
PID1 terminal proof, exact PID1 absence, parent-wait proof, and the exact
owner-close intent and closed-owner certificate. No normal owner-close
progression warning remained in the passing run. Duplicate L and connected
`l` retained `pending` without another E or provider effect.

The installed launcher still returned 70 on `pending`. These observations do
**not** establish `exit 0 drained <request-id>` or a delivered caller exit.
The existing L/l terminal publication gap remains.

## Source corrections

- The feature-enabled connected fixture previously could not call the
  production `run_normal_model` path. A fixture-gated dispatch now calls that
  same function from the ordinary connected child; the featureless path keeps
  its direct call.
- Owner-close scanning previously read D's cross-store session and invocation
  binding before K existed. U and the State admission can be visible while
  the retained mailbox session or invocation binding is still publishing.
  The scanner now uses a read-only K prefilter and performs the full exact D
  and invocation checks only after K is present. This removed the observed
  transient close warnings without a timing delay.
- The normal fixture supplies `OULIPOLY_DATA_DIR` and private model config
  under its disposable root. Without the data directory, the child failed
  before K. The Broker's separate legacy process-recorder diagnostic still
  reports a missing data directory in the test log; it did not block the
  connected normal terminal evidence above.

## Verification and limits

- Both root-mapped connected fixture cases passed: ordinary offline `--help`
  and ordinary normal `--model`. The normal case checks pair/source/root
  identity, K once, Q, output, publication, control wait, physical drain,
  owner close, and absence of close warnings.
- Featureless Runner and Broker bins built with `--locked` and
  `--no-default-features`; existing compiler warnings remain.
- The featureless Broker close-preflight unit test passed.
- `cargo fmt --all --check` and `git diff --check` passed.

Offline no-effect help remains a separate gap: it has no provider K/Q and no
normal owner-close selection. A fixed-path host installation, host service or
aliases, real Bash child, live provider, featureless installed end-to-end run,
crash timing, and AGE-353 stress/benchmark were not exercised. No host State,
service, aliases, Bash repository, or ticket were changed.

The nine-domain CRW observation and corrective revisit were not run because
this assignment prohibited subagents. That is explicit review debt for the
parent, not merge-readiness or production-readiness evidence.
