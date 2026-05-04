# WU-16-01 Release Scripts Test Residuals

This file records residual risks left after the Phase 6b structural test
extension for WU-16-01.

Contract source: `product-strategy/contracts/wu-16-01-release-scripts.md`.

Precedent: WU-13-01 kept live release evidence separate from merge-time
structural YAML coverage in `risk/13-release-restore-test-residuals.md`.

## Scope Of Executable Coverage

The executable Step 6b coverage is the existing Rust structural test:

`src-tauri/tests/release_yml_contract.rs`

Test function:

`release_yml_restores_windows_and_target_suffixed_bare_binaries`

The new assertion parses `softprops/action-gh-release@v2` `with.files`.

The parse rule is exact line membership only:

split on newlines,

trim each line,

drop empty lines,

collect into `BTreeSet<String>`,

compare to the eight expected release-file entries.

This guards the workflow shape before Step 6c product code is written.

## AC-3 README Snippet Residual

AC-3 is documentation-only by contract.

It is intentionally not encoded in `release_yml_contract.rs`.

Step 6c must update `README.md` by code review against the contract.

The reviewer should verify the binary-install snippet sits after the existing
source-build quota-adapter snippet and before `## Session Ingestion`.

The reviewer should verify all seven `gh release download --pattern` flags.

The reviewer should verify the matched-version warning for binary and scripts.

The reviewer should verify the stale-script failure mode is documented:

stale scripts may omit `body` silently,

and new ingests may leave `session_turns.body` empty.

No automated doc test currently checks that README command freshness.

This follows the contract's test-intent routing for AC-3.

## AC-2 Live Release-Asset Residual

The structural test verifies that the release workflow lists the exact assets.

It does not contact GitHub.

It does not prove `softprops/action-gh-release@v2` uploads assets in a live run.

It does not prove the GitHub release page exposes the seven basename assets.

It does not prove `gh release download` can retrieve each asset by pattern.

The live-release materialization residual remains until a release run or trial
release records the actual asset inventory.

Expected live evidence should include the release URL, tag, workflow run URL,
and the seven adapter script asset names.

This matches the WU-13-01 precedent where live release asset evidence remained
outside structural merge-time tests.

## AC-5 CI Residuals

Phase 6b compiled the Rust test targets locally.

Phase 6b captured the expected RED failure for the release YAML contract test.

Phase 6b does not prove final Step 6c product-code gates are green.

Step 6c must run or document the requested Rust gates:

`cargo fmt --check`,

`cargo clippy -- -D warnings`,

`cargo test --no-fail-fast`.

Step 6c must also run or document the requested frontend gates.

Live CI on Linux remains residual until CI executes the Step 6c result.

Live CI on macOS remains residual until CI executes the Step 6c result.

## AC-6 Release Bundle Residuals

The preserved WU-13-01 structural assertions continue to guard matrix rows.

They continue to guard artifact upload and download shape.

They continue to guard Linux `.deb` collection.

They continue to guard macOS `.dmg` collection.

They continue to guard Windows `.msi` and NSIS `.exe` collection.

They continue to guard target-suffixed bare binaries.

They do not prove a live release run produces every bundle artifact.

They do not prove the live release page contains every bundle artifact.

They do not prove live release downloads are intact for every platform.

Live release bundle materialization remains residual, following WU-13-01's
separation between structural workflow checks and release-run evidence.

Step 6c or release verification should capture live asset inventory when a
release run is available.
