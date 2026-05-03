# WU-13-01 Test Residuals

Step 6b produced tests for AC-3 and AC-5 only. The following contract section 8 risks remain outside Step 6b's executable coverage.

## R1 - Windows default-inherited ACLs

Unverified by tests: the tests validate functional lock behavior, not Windows ACL inheritance or current-user-only access.

Mitigation: Step 6c must rewrite `DECISIONS.md` D-006 to describe Windows as supported and explicitly document the Windows default-ACL choice.

## R2 - `fs4` MSRV / dependency tree

Unverified by tests: Step 6b cannot modify product `[dependencies]`, so it cannot encode the final `fs4` dependency feature shape.

Mitigation: Step 6c must add `fs4` with the contract-specified minimal sync feature shape and run the Rust gates, including the Windows target check.

## R3 - Cross-process helper discovery on CI

Partially verified by tests: `session_lock_cross_platform.rs` uses `std::env::current_exe()` and an ignored helper test to avoid a separate helper binary. This compiled on the Linux host.

Residual: Windows execution of the helper is still CI evidence.

Mitigation: Step 6c must run or trigger the Windows test/check path named by the contract.

## R4 - Trial-release evidence record

Unverified by tests: the structural YAML test cannot prove a real workflow-dispatch run publishes all release assets.

Mitigation: AC-6 remains an external evidence record with `workflow_run_url`, `workflow_run_id`, `release_url`, release tag, asset inventory, per-platform SHA-256 values, matrix artifact listing, and Windows bundle filenames.
