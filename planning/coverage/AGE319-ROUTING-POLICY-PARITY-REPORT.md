# AGE-319 fresh routing policy parity candidate

This is a source-only, private-fixture candidate based on `f57f6682`. The
production fresh selector remains CLOSED. No pair was installed or activated,
and no real provider was called.

## Implemented

- The broker now opens the model and provider TOML through a passed directory
  descriptor and uses the runtime's config loader to independently check the
  digest, account order, quota and auth commands, and complete roster. It
  checks again at selection. A durable source record binds all registrations
  and selection for one held root to the same directory device/inode.
- A second root with the same config, account, index, and environment can
  durably reference a fresh quota Q or a pending quota K. It does not start
  another quota K. The referenced effect is read back on each observation;
  unknown state retains both artifact paths. Failed or stale results do not
  become availability.
- A broker-local, cross-process `flock` serializes quota-first admission and
  auth-refresh admission per account. Auth refresh still refuses when a
  different root has an active or recently completed refresh; its error now
  names the other effect and artifact rather than implying success.
- Broker routing counts nonzero physical-Q failures in a 30-minute window and
  live consumed provider K in a 60-minute window. Three recent failures
  suppress an unpinned account only if another quota-eligible account remains;
  explicit pins retain quota eligibility and bypass that suppression. The
  unmetered fallback applies production's ten-invocation recent-error penalty.
  The receipt policy version is `fresh-account-effects-v2`.

## Verification

- `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --bin oulipoly-kernel-broker fresh_provider::tests`: 6 passed. These include config edit/forgery and same-byte different-directory refusal; quota window freshness; recent-failure threshold, fallback, and pin; auth lock; one-use K/Q; fresh and pending cross-root quota reuse without another grant.
- `cargo test -p oulipoly-runtime --test routing_matrix`: 54 passed. This characterizes the production route, not fresh parity.
- Private `original_runner_joins_once_behind_persistent_root_pid1` passed for `normal_model_provider_quota_available`, `normal_model_provider_no_pin`, `normal_model_provider_quota_reply_loss`, and `normal_model_provider_quota_restart` on the final broker/Runner images. The fixture compares old State/WAL bytes and verifies one-use effect and Q-gated result. Earlier in this Act, `normal_model_provider_auth` and `normal_model_provider_bad_config` also passed after the descriptor change and before the final source-record addition.
- The normal `src-tauri/target` cargo check with `CARGO_BUILD_JOBS=2 -j2`, `cargo fmt --all -- --check`, and `git diff --check` passed.

## Remaining exact product gaps

Production density routing uses canonical turn ingestion, projected 5-hour and
7-day burn, topology repair, marker verification, and historical failure
classification from `StateDb`. The fresh broker owns none of those fresh-lane
records yet and must not read or write the old State/WAL. The present
remaining-basis-points score is therefore not production density parity.
Only provider nonzero Q is counted as a recent failure here; production's
typed failure history has a different source. The recent auth-refresh refusal
does not yet coalesce a second root into a retry after a successful first
refresh. The source directory descriptor is chosen by the attested Runner;
the broker verifies its bytes and identity but does not independently derive
the user's configuration path from root-entry authority.

The private route still supports a narrow provider shape, carries the existing
bounded control frame, and has no source/recipient publication or installed
host-sudo proof. An uncertain provider effect remains unknown with its
attempt/artifacts; the Runner does not automatically replay it. A caller-owned
risky retry needs a separate explicit attempt path that retains the original
unknown. None of these gaps is waived by the passing private tests.
