# WU-16-01 Release-Scripts Contract

Phase 6a contract authored by the orchestrator from the approved
problem map (`research/16-release-scripts-problem-map.md`), the
approved proposal (`proposals/16-release-scripts.md`), the Phase 4
risk reports, and the Phase 5 hookpoints
(`research/16-release-scripts-hookpoints.md`).

The contract is the binding interface between Phase 6b (test writer)
and Phase 6c (code writer). The test writer never sees product code;
the code writer reads tests + the Step 6b output index + this
contract.

## 1. Scope

In-scope files (per ticket Code Boundary, problem map §1, hookpoints
§2):

- `.github/workflows/release.yml` (publish step `softprops/action-gh-release@v2`).
- `src-tauri/tests/release_yml_contract.rs` (extend WU-13-01
  structural test).
- `README.md` (insert binary-install snippet adjacent to existing
  source-build snippet; preserve source-build snippet verbatim).
- `scripts/README.md` (optional one-line cross-reference).

Out-of-scope (anti-scope):

- Bare-binary platform-suffix contract (WU-13-01) — UNCHANGED.
- `.deb` / `.dmg` / `.msi` Tauri bundle contents — UNCHANGED.
- The seven adapter scripts themselves — UNCHANGED.
- `scripts/migrate-model-names.sh`, `scripts/tests/`,
  `scripts/README.md` (as a release asset) — NOT uploaded.
- Frontend (`src/`), Rust runtime (`src-tauri/src/`) — UNCHANGED.
- Runtime version-skew detection — DEFERRED.
- `scripts.tar.gz` bundle — REJECTED in favor of individual files.
- Adding to system PATH automatically — OUT OF SCOPE.

## 2. Behavioral contract

### 2.1 release.yml publish step

The `softprops/action-gh-release@v2` step at
`.github/workflows/release.yml:177-181` MUST upload, in addition to
the existing `artifacts/*` glob, the following exact paths from the
checked-out repo (Q-G binds Option A):

```
scripts/claude-code-turns
scripts/codex-turns
scripts/anthropic-usage
scripts/chatgpt-usage
scripts/zai-usage
scripts/claude-code-locate-transcript
scripts/codex-locate-transcript
```

Encoding requirements:

- Use a YAML block scalar (`files: |` + indented entries) so each
  non-empty trimmed line is one path/glob. This matches the
  documented `softprops/action-gh-release@v2` multi-line `files:`
  semantics (locked by hookpoints Q-A / A1).
- The `artifacts/*` glob MUST be retained as one of the entries.
- All eight entries (the existing `artifacts/*` plus the seven
  script paths) MUST appear with no other entries added or removed.
- Order is not asserted; the structural test parses entries into
  a sorted set.
- The publish step's other `with:` keys (`tag_name`,
  `generate_release_notes`) and the surrounding job graph
  (`actions/checkout@v4` at `:168`, `actions/download-artifact@v4`
  with `merge-multiple: true` and `path: artifacts` at `:169-172`,
  the `Create tag` `run:` block at `:173-176`) MUST remain
  unchanged.
- `actions/checkout@v4` is the source-of-truth for the
  `scripts/<name>` paths; no copy-into-`artifacts/` step is added.

### 2.2 Structural test extension

`src-tauri/tests/release_yml_contract.rs` MUST extend the existing
`release_yml_restores_windows_and_target_suffixed_bare_binaries`
test (Q-H binds extension over a sibling test). The current single
scalar-equality assertion at `:253-262` is the replacement seam:

- Replace `string_at(...) == "artifacts/*"` with a multi-line set
  parse and a `BTreeSet` exact-equality assertion against the
  required eight entries: `artifacts/*` plus the seven AC-1 script
  paths verbatim.
- Reuse `step_by_uses` (`:327-332`) and `string_at` (`:341-346`).
- Reuse the existing `BTreeSet` import at line 2 of the test file.
- The parse rule MUST be: split on newlines, trim each line, drop
  empty lines, collect into a `BTreeSet<String>`. Do not use regex
  or glob inference. Membership is asserted by exact-string
  equality.
- The bare-binary `collect_step_run_bare_binary_hits` assertion at
  `:269-278` MUST remain unchanged (anti-scope: WU-13-01 contract).
- All other assertions in the test file MUST remain unchanged.
- The test MUST fail RED on this branch's pre-fix HEAD (the
  publish step currently emits `files: artifacts/*` only; the
  scripts are absent).

Helper-function citations Phase 6b MUST honor:

```
step_by_uses    src-tauri/tests/release_yml_contract.rs:327-332
string_at       src-tauri/tests/release_yml_contract.rs:341-346
BTreeSet import src-tauri/tests/release_yml_contract.rs:2
```

The `with.files` parsed value is the YAML scalar's textual content
(softprops accepts a multi-line scalar with one entry per line).
`string_at` returns this content unchanged; the test parses it.

### 2.3 README.md install snippet

`README.md` MUST receive a new binary-install paragraph + fenced
shell block inserted between the existing source-build snippet
ending at line 350 and the `## Session Ingestion` heading at line
352. Insertion point is locked by hookpoints §2 entry 3.

Required content:

- A short prose paragraph stating that scripts are also available
  as release assets, recommending `gh release download`, and
  warning that scripts and binary versions must match for body
  ingestion to work — mismatched/stale scripts may silently omit
  `body`, leaving new ingests with empty `session_turns.body`.
- One fenced `bash` block containing exactly:
  - One `gh release download v0.1.X --repo nestharus/agent-runner`
    invocation with `--pattern` per script name (seven `--pattern`
    flags, one per AC-1 script basename) and a target `--dir`
    matching the existing source-build snippet's `~/.local/bin/`.
  - One `chmod +x` invocation listing all seven installed files
    under `~/.local/bin/`.
- The version placeholder MUST be `v0.1.X` (consistent with
  ticket §"Notes for Phase 2.5+" recommended snippet form).

Hard preservation requirements:

- `README.md:332-350` (the existing source-build snippet,
  including the `install -m 755 \` block listing five scripts) MUST
  remain byte-identical. The five-script source-build snippet is
  preserved by AC-3.
- `README.md:352` (`## Session Ingestion`) MUST remain unchanged.
- All other README content MUST remain unchanged.

Test-intent: AC-3 is doc-only and is verified by code-review; no
structural assertion encodes the README change. Residual-risk path:
`risk/16-release-scripts-test-residuals.md`.

### 2.4 scripts/README.md cross-reference (optional)

`scripts/README.md` MAY receive a one-line cross-reference inserted
after the opening paragraph (after line 5 in the existing file).
The line MUST read exactly:

```
For release-asset installation of the bundled reference adapters, see README §Reference quota adapters.
```

No command blocks, no parallel install instructions. If the line is
omitted, no test fails; the WU still meets ticket Code Boundary.

## 3. Test-intent track (binding for Step 6b)

| AC | Risk | Level | Fixture | Expected signal | Residual |
|---|---|---|---|---|---|
| AC-1 + AC-2 | Publish step omits one or more AC-1 scripts; structural test fails to detect future omissions | particular-integration (cargo test) | `release_yml_restores_windows_and_target_suffixed_bare_binaries` | `assert_eq!` of `BTreeSet` parsed from `with.files` against the eight-entry expected set | None — encoded |
| AC-3 | README install snippet missing or under-specified | doc | n/a (code review) | reviewer sees inserted snippet between :350 and :352 with all seven `--pattern` flags + matched-versions note | `risk/16-release-scripts-test-residuals.md` |
| AC-4 | Existing CI (`ci.yml`) breaks due to test extension | particular-integration (cargo test) | the same test file | `cargo test` green on existing CI matrix | None — encoded by AC-2 |
| AC-5 | `cargo fmt` / `clippy -D warnings` / `cargo test --no-fail-fast` regression | unit + particular-integration | repo-wide cargo workspace | gates green on Linux + macOS (CI) | live CI run is residual |
| AC-6 | Other release jobs regress (Linux .deb, macOS .dmg, Windows .msi/.exe, three suffixed bare binaries) | particular-integration (existing structural assertions in the same test file) | same test file's existing assertions | every existing assertion passes; bare-binary `collect_step_run_bare_binary_hits` set unchanged | live release run is residual |

## 4. Step 6b output index obligations

The Step 6b test writer MUST produce
`tmp/scratch/wu-16-01/phase6/step6b-output-index.md` with the
following content:

- Mapping from each AC (1, 2, 4, 5, 6) to the test file path +
  function name + line range that encodes that AC.
- A statement that AC-3 is intentionally not encoded by a
  structural test, with a citation to
  `risk/16-release-scripts-test-residuals.md`.
- A pre-fix RED-run capture: verbatim output of `cargo test --test
  release_yml_contract --no-fail-fast` against `bc6df8e` (the WU's
  base commit) showing the new assertion fails. Save the verbatim
  RED-run log to
  `tmp/scratch/wu-16-01/phase6/release-yml-contract-red-run.log`
  and reference it in the index.
- A statement that no Step 6c product code has been written.

Step 6b MUST NOT touch `release.yml` or any product code.

## 5. Step 6c input obligations

The Step 6c code writer MUST read, before changing any product
file:

1. This contract.
2. The Step 6b output index at the path above.
3. The test file (extended).
4. The Step 6b RED-run log.
5. `proposals/16-release-scripts.md` and
   `research/16-release-scripts-problem-map.md` and
   `research/16-release-scripts-hookpoints.md`.

Step 6c MUST then:

- Edit `.github/workflows/release.yml` per §2.1.
- Optionally edit `scripts/README.md` per §2.4.
- Edit `README.md` per §2.3.
- Run gates: `cd src-tauri && cargo fmt --check && cargo clippy
  -- -D warnings && cargo test --no-fail-fast` and (worktree root)
  `bun run check && bunx tsc --noEmit && bun run test`.
- Capture a verbatim GREEN-run log of the new assertion at
  `tmp/scratch/wu-16-01/phase6/release-yml-contract-green-run.log`.

If frontend gates fail due to environmental issues (e.g. `biome`
not installed, `bun install` 404s on font packages), document and
proceed — these are pre-existing env issues unrelated to WU-16-01;
any new frontend regressions caused by this WU are blocking.

## 6. Risk annotations

- `RISK-01`: silent drift if `scripts/` gains a new adapter and the
  publish-step list is not updated. Bound by §2.2 (BTreeSet exact
  equality detects extra/missing entries; new adapter forces
  deliberate test edit).
- `RISK-02`: structural test guards workflow shape only, not live
  release-asset materialization. Documented residual at
  `risk/16-release-scripts-test-residuals.md`.
- `RISK-03`: README version placeholder `v0.1.X` may go stale if
  not updated at each release. Acceptable: README is doc; the WU
  does not promise auto-substitution.
- `RISK-04`: matched-versions failure mode (binary-install user
  fetches scripts from a different release tag). Mitigated by
  README note.

## 7. Status

Status: ready for Phase 6b.
