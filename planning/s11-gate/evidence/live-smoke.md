# S11 Live Smoke Evidence Notes

These notes bind live artifacts that existed on the machine during the S11 gate. They are secondary to shipped tests in `planning/s11-gate/proposal.md` because the live files are external `/tmp` artifacts, not committed deterministic fixtures.

## Verified Files

| Claim fragment | Evidence file | Verified content |
|---|---|---|
| External wake initial dispatch | `/tmp/s11-e2e/initial10.log` | Contains `DISPATCHED ab_19e9eb3c740_11963_58a0b4405542b714` and a successful initial `OULIPOLY_RESULT`. |
| External wake initial dispatch for resumed response | `/tmp/s11-e2e/fresh8-live.stdout` | Contains `DISPATCHED ab_19e9df7551b_18292_df32f8f970b03621` and successful initial `OULIPOLY_RESULT` for session `ses_16208bf9bffeXcsjTwsS09uULG`. |
| External wake resumed response | `/tmp/s11-e2e/fresh8-manual-resume.stdout` | Contains same-session `WOKE 0` for `ses_16208bf9bffeXcsjTwsS09uULG`. |
| External wake initial dispatch for resumed response | `/tmp/s11-e2e/fresh9-live.stdout` | Contains `DISPATCHED ab_19e9e0162b3_57736_fbc6a07cdaecdf71` and successful initial `OULIPOLY_RESULT` for session `ses_161feaee1ffeS91qHwRaaX2gmc`. |
| External wake resumed response | `/tmp/s11-e2e/fresh9-manual-mailbox-resume.stdout` | Contains same-session `WOKE 0` for `ses_161feaee1ffeS91qHwRaaX2gmc`. |
| External wake exported session contains resumed response | `/tmp/s11-e2e/fresh13-export.json` | Contains exported session text `WOKE 0`. |
| External wake workload completion | `/tmp/s11-e2e/fresh10-workload-done.txt`, `/tmp/s11-e2e/fresh13-workload-done.txt` | Each contains `S11-WAKE-OK`. |
| External wake live sidecar delivery confirmation | `/tmp/s11-e2e/fresh10-xdg-data/oulipoly-agent-runner/pid-identity.db` | Python SQLite read verified `mailbox` rows for seq 1 and 2 as `CONFIRMED-DELIVERED`, `delivery_attempts=1`, `delivery_error=''`, and `delivered_by_invocation_uuid='7a46a1a5-844d-45e8-bd67-aecaf9cf9194'`. |
| External wake resumed invocation success | `/tmp/s11-e2e/fresh10-xdg-data/oulipoly-agent-runner/invocations/7a46a1a5-844d-45e8-bd67-aecaf9cf9194.result` | Contains `"status":"succeeded"` and `"success":true`. |
| S10 external launch smoke | `/tmp/s10-e2e/final.log`, `/tmp/s10-e2e/final2.log` | Each contains `S10-EXTERNAL-OK` and final `OULIPOLY_RESULT.success=true`. |
| S10 final smoke marker | `/tmp/claude-1000/-home-nes-projects-agent-runner/45ccb26a-8bb6-4486-9b1e-2226e29292a0/tasks/b1vgap7ge.output` | Prior retained evidence path named by the earlier gate package; use only if present locally and still containing `S10-FINAL-OK` and `"status":"succeeded"`. |
| XHIGH external smoke marker | `/tmp/claude-1000/-home-nes-projects-agent-runner/45ccb26a-8bb6-4486-9b1e-2226e29292a0/tasks/bvnh4nja0.output` | Contains `XHIGH-EXTERNAL-OK` and `"status":"succeeded"`. |

## Unverified Caller-Provided Labels

Local file search did not find concrete `/tmp/s10-e2e` lines containing `S10-FINAL-OK` or `S10-RESUME-OK`; the retained `/tmp/s10-e2e` files currently show `S10-EXTERNAL-OK` launch smoke. The S10 resume behavior is therefore bound in this gate to shipped deterministic integration tests, while the available live S10 files are cited only for external launch smoke.

The user-provided phrase `DISPATCHED -> CONFIRMED-DELIVERED 1 attempt -> WOKE 0` is represented by shipped S11 tests and live artifacts across the same smoke family: `/tmp/s11-e2e/initial10.log` contains the dispatch handle, the live sidecar DB rows show confirmed delivery in one attempt, and `/tmp/s11-e2e/fresh13-export.json` contains `WOKE 0`. Shipped tests assert the same semantics plus claim release for delivery-confirmed paths.
