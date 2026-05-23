---
eval_id: age-179-provider-docs
lifecycle: WRITE
owner: oulipoly-provider docs (AGE-174 family)
---

# AGE-179 Provider-Contract Documentation Eval

## Behavior

After AGE-179 merges, one or more of the following unwanted conditions may be present in the diff or merged content: a required section heading or GFM anchor is absent from `AGENTS.md` or `README.md`; a named API identifier (trait, type, field, variant, or re-exported type) from the verified source-of-truth surface is misspelled or missing in the section that documents it; a concrete-provider identifier prohibited by the forbidden-identifier rule appears in new prose or code-fenced examples; a required parse-only notice or layer-separation orientation sentence is absent or buried as a footnote; a required cross-link target does not resolve to an existing anchor or file; pre-existing content at or below the insertion boundary is modified; the `TODO(design)` block at `AGENTS.md` lines 218–237 is altered; the diff touches files outside the four permitted paths (`AGENTS.md`, `README.md`, `evals/age-179-provider-docs/eval.md`, and `DECISIONS.md`); or the eval file itself contains pytest-shaped constructs.

## Boundaries

- **In scope**: `AGENTS.md` lines 362 and above (new content only); `README.md` lines 988 and above (new content only); `evals/age-179-provider-docs/eval.md` (this file); `DECISIONS.md` (append-only additions to the pre-existing repository-tracked decision log); the git diff of branch HEAD against `origin/main` at `14242c03a0e26127c59daab81b9a4354a01f927d`.
- **In scope**: The six new anchor slugs the new content must introduce, the five pre-existing anchor slugs that must remain unmodified and unshadowed, and the six required cross-link targets (D-01, D-03, D-04, D-05, D-07, EP-8).
- **In scope**: Verbatim string matching for API identifiers sourced from `crates/oulipoly-provider/src/lib.rs` (lines 7–71, 260–375) and `crates/oulipoly-config/src/provider_implementation_ref.rs`.
- **Out of scope**: Pre-existing `AGENTS.md` prose (lines 1–361) and pre-existing `README.md` prose (lines 1–987) for any purpose other than unchanged-content verification (F-line-budget, F-todo-untouched).
- **Out of scope**: Runtime behavior of `crates/oulipoly-provider` or `crates/oulipoly-config`; Rust compilation, test results, or TypeScript type-checking results.
- **Out of scope**: Correctness of pre-existing workspace-structure listings (DD-01 drift is deferred to a future debt-sweep WU and is not asserted by this eval).
- **Out of scope**: Prose quality, style, or readability beyond the verbatim presence of required phrases and identifiers.

## Trace fields required

A future runner consuming this eval must assemble the following trace evidence roles:

- **Diff bundle**: output of `git diff origin/main...HEAD` scoped to `AGENTS.md`, `README.md`, `evals/age-179-provider-docs/eval.md`, and `DECISIONS.md`; and `git diff --name-only origin/main...HEAD` for the full changed-file list. The `DECISIONS.md` slice is used for additive-append verification only (deletion-hunk detection — no `-` lines permitted in the `DECISIONS.md` diff). Reference commit: `14242c03a0e26127c59daab81b9a4354a01f927d`.
- **New-content slices**: `AGENTS.md` from line 362 to EOF; `README.md` from line 988 to EOF — used for all positive-evidence pattern matching in findings F-anchors-present, F-trait-names-present, F-locator-reexports-present, F-toml-keys-present, F-no-banned-identifiers, F-parse-only-notices-present, F-okg4-layer-separation-present, F-glossary-four-entries, F-dd-02-redesign-note.
- **Pre-existing-tail slices**: `AGENTS.md` lines 1–361 and `README.md` lines 1–987 read from the working tree — used for byte-for-byte comparison against `origin/main` at `14242c03a0e26127c59daab81b9a4354a01f927d` (F-line-budget).
- **TODO block slice**: `AGENTS.md` lines 218–237 read from the working tree — used for byte-for-byte comparison against `origin/main` (F-todo-untouched).
- **Eval file content**: `evals/age-179-provider-docs/eval.md` read from the working tree (F-no-pytest-shape).
- **Cross-link resolution map**: for each `[text](target)` link in the new-content slices, the resolved heading slug (GFM-normalized) or file path, verified against the working tree (F-cross-links-resolvable).

## Findings

### F-anchors-present

- **severity**: HIGH
- **evidence_paths**: `AGENTS.md` (lines 362..EOF), `README.md` (lines 988..EOF), `AGENTS.md` (lines 1..361), `README.md` (lines 1..987)
- **summary**: One or more required section headings introduced by this WU are absent from the new content, or one or more pre-existing headings required as cascade cross-link targets are no longer present in their expected files.
- **positive evidence**: Absence of any of the following heading lines (with the GFM-normalized slug each must render to) in the new content:
  - In `AGENTS.md` lines 362..EOF:
    - `` ## Provider Contract Crate (`oulipoly-provider`) `` → `#provider-contract-crate-oulipoly-provider`
    - `### Per-Concern Traits` → `#per-concern-traits`
    - `` ### Locator Slot and `TranscriptLocator` Re-Export `` → `#locator-slot-and-transcriptlocator-re-export`
    - `` ### TOML `provider` Field (Implementation Reference) `` → `#toml-provider-field-implementation-reference`
    - `### Provider Term Glossary` → `#provider-term-glossary`
  - In `README.md` lines 988..EOF:
    - `## Implementing a Provider` → `#implementing-a-provider`
  - Also fires if any of the following pre-existing headings are absent from their respective files (must survive unmodified in the pre-existing ranges):
    - `AGENTS.md`: `### Model Command Syntax` (`#model-command-syntax`, line 118); `## Rust Workspace Structure` (`#rust-workspace-structure`, line 320)
    - `README.md`: `` ### Optional: `transcript_locator` `` (`#optional-transcript_locator`, line 465); `` ### `providers.toml` `` (`#providerstoml`, line 283); `### Adding a Model` (`#adding-a-model`, line 823)
- **non-fire cases**: All six new headings are present verbatim in the appended content; all five pre-existing cascade-target headings remain present in the pre-existing ranges; no proposed slug collides with any pre-existing slug (shadow-check clean per hookpoints research §5).
- **suggested_action**: Add the missing heading to the appropriate new section with the exact GFM-normalized slug listed above. Verify pre-existing headings were not inadvertently modified or removed. Slug derivation is documented in `planning/age-179-provider-docs/research/age-179-hookpoints.md` §2.
- **confidence**: HIGH

### F-line-budget

- **severity**: HIGH
- **evidence_paths**: `AGENTS.md` (lines 1..361), `README.md` (lines 1..987)
- **summary**: Pre-existing content at or before the insertion boundary has been modified; `AGENTS.md` lines 1–361 or `README.md` lines 1–987 are not byte-for-byte identical to the same ranges in `origin/main` at commit `14242c03a0e26127c59daab81b9a4354a01f927d`.
- **positive evidence**: Any byte difference between the working-tree content of `AGENTS.md` lines 1–361 and the same range in `origin/main` at `14242c03a0e26127c59daab81b9a4354a01f927d`. Sentinel check: line 361 of `AGENTS.md` must be exactly `EMPTY_POOLS.` and line 987 of `README.md` must be exactly `MIT`. Any deviation from either sentinel value constitutes positive evidence.
- **non-fire cases**: `AGENTS.md` lines 1–361 are byte-identical to `origin/main`; `README.md` lines 1–987 are byte-identical to `origin/main`; new content first appears at `AGENTS.md` line 362 and `README.md` line 988.
- **suggested_action**: Revert any modification to `AGENTS.md` line ≤ 361 or `README.md` line ≤ 987. New sections must be appended only; no pre-existing line may be altered. Use `git diff origin/main -- AGENTS.md` to locate the offending hunk.
- **confidence**: HIGH

### F-todo-untouched

- **severity**: HIGH
- **evidence_paths**: `AGENTS.md` (lines 218..237)
- **summary**: The `TODO(design)` block at `AGENTS.md` lines 218–237 ("Remaining TODO(design) Comments") has been modified, reorganized, or annotated; it is not byte-identical to `origin/main` at `14242c03a0e26127c59daab81b9a4354a01f927d`.
- **positive evidence**: Any byte difference between the working-tree content of `AGENTS.md` lines 218–237 and the same range in `origin/main` at `14242c03a0e26127c59daab81b9a4354a01f927d`.
- **non-fire cases**: `AGENTS.md` lines 218–237 are byte-identical to `origin/main`; no annotation, link insertion, reordering, or whitespace change has been applied to the `TODO(design)` block.
- **suggested_action**: Revert `AGENTS.md` lines 218–237 to their `origin/main` state. This block is explicitly prohibited from modification by the ticket anti-scope (`contracts/age-179-provider-docs.md § Forbidden behaviors`); cleanup is deferred to a future debt-sweep WU.
- **confidence**: HIGH

### F-trait-names-present

- **severity**: HIGH
- **evidence_paths**: `AGENTS.md` (lines 362..EOF)
- **summary**: One or more required API identifiers from `crates/oulipoly-provider/src/lib.rs` are absent or misspelled in the new AGENTS.md content, leaving the documented provider-contract surface incomplete and susceptible to silent drift.
- **positive evidence**: Absence of any of the following verbatim strings anywhere in `AGENTS.md` lines 362..EOF:
  - Per-concern trait names (source: `lib.rs` lines 12–61): `ProviderLaunch`, `ProviderPolicy`, `TerminalSignalRecognizer`, `ProviderQuota`, `ProviderSession`, `ProviderRotation`, `ProviderDiscovery`
  - Aggregation type (source: `lib.rs` lines 260–278): `ProviderCapabilities`
  - Wrapper type (source: `lib.rs` lines 297–375): `LocatorRequiredCapabilities`
  - Error type (source: `lib.rs` lines 64–71): `CapabilityError`
  - All five error variants (source: `lib.rs` lines 65–70): `Unsupported`, `LocatorRequiredButMissing`, `Invalid`, `Unavailable`, `Failed`
  Also fires for a misspelling that prevents verbatim match (e.g., `ProviderCapability` instead of `ProviderCapabilities`).
- **non-fire cases**: Every identifier above appears verbatim at least once in `AGENTS.md` lines 362..EOF; all five `CapabilityError` variants are named; no identifier is abbreviated or paraphrased.
- **suggested_action**: For each missing or misspelled identifier, correct the prose in the relevant new section (`### Per-Concern Traits` for traits and error types). Verify exact spelling against `crates/oulipoly-provider/src/lib.rs`. All five `CapabilityError` variants must be present by name — omitting any variant (UC-1/UC-2/UC-3) leaves the error surface undocumented with no CI detection.
- **confidence**: HIGH

### F-locator-reexports-present

- **severity**: HIGH
- **evidence_paths**: `AGENTS.md` (lines 362..EOF)
- **summary**: One or more of the eight re-exported locator types from `oulipoly_runtime::session_metadata` are absent or misspelled in the new AGENTS.md content.
- **positive evidence**: Absence of any of the following verbatim strings anywhere in `AGENTS.md` lines 362..EOF (source: `crates/oulipoly-provider/src/lib.rs` lines 7–10, `pub use oulipoly_runtime::session_metadata::{…}`):
  `LocatedTranscript`, `LocatorError`, `LocatorSource`, `ScriptKind`, `TranscriptLocator`, `TranscriptLookupMode`, `TranscriptRequest`, `UnsupportedStorageReason`
- **non-fire cases**: All eight type names appear verbatim at least once in `AGENTS.md` lines 362..EOF, most likely in the `` ### Locator Slot and `TranscriptLocator` Re-Export `` section.
- **suggested_action**: Add the missing re-exported type name to the locator-slot section. Verify exact spelling against `crates/oulipoly-provider/src/lib.rs` lines 7–10.
- **confidence**: HIGH

### F-toml-keys-present

- **severity**: MEDIUM
- **evidence_paths**: `AGENTS.md` (lines 362..EOF)
- **summary**: One or more TOML key names or validation error names required by the `ProviderImplementationRef` documentation are absent from the new TOML field section (A1.S4), or a TOML code-fenced example uses the Rust field name `crate_name` instead of the serde-renamed TOML key `crate`.
- **positive evidence**: Absence of any of the following verbatim strings in `AGENTS.md` lines 362..EOF:
  - TOML-facing key names (source: `provider_implementation_ref.rs` lines 7–15 with serde annotations): `path`, `crate`, `version`, `binary`, `script`
  - Validation error variant names (source: `provider_implementation_ref.rs` lines 19–29): `NoFlavor`, `MultipleFlavors`, `VersionWithoutCrate`
  Also fires if any code-fenced TOML example within the new content uses `crate_name` as a key (the `#[serde(rename = "crate")]` annotation at `provider_implementation_ref.rs:8` means the TOML-facing key is `crate`, not `crate_name`).
- **non-fire cases**: The TOML field section names all five keys and all three error variants; any TOML code-fenced example uses `crate` (not `crate_name`) as the key; the `crate_name`/`crate` rename is either documented explicitly or implicitly honored in all examples.
- **suggested_action**: Add the missing key or error name to the `` ### TOML `provider` Field (Implementation Reference) `` section. If a TOML example shows `crate_name`, replace it with `crate` to match the serde rename at `provider_implementation_ref.rs:8`. The error variant list must include all three: `NoFlavor`, `MultipleFlavors`, `VersionWithoutCrate`.
- **confidence**: HIGH

### F-cross-links-resolvable

- **severity**: HIGH
- **evidence_paths**: `AGENTS.md` (lines 362..EOF), `README.md` (lines 988..EOF), worktree file system
- **summary**: One or more Markdown links introduced in the new sections do not resolve: an in-file or cross-file anchor link does not correspond to an existing heading slug, or a file-path link points to a file absent from the worktree.
- **positive evidence**: Any `[text](target)` link in the new sections where `target` is (a) an anchor slug that does not match any heading in the named file, or (b) a file path that does not exist in the worktree. Required cross-links and their verified targets (from `planning/age-179-provider-docs/research/age-179-hookpoints.md` §3):
  - **D-01**: link targeting `README.md#optional-transcript_locator` must resolve to `` ### Optional: `transcript_locator` `` at README.md line 465.
  - **D-03**: link targeting `README.md#providerstoml` (cross-file) or `#providerstoml` (within README) must resolve to `` ### `providers.toml` `` at README.md line 283.
  - **D-04**: link targeting `README.md#adding-a-model` (cross-file) or `#adding-a-model` (within README) must resolve to `### Adding a Model` at README.md line 823.
  - **D-05**: link targeting `AGENTS.md#model-command-syntax` (cross-file) or `#model-command-syntax` (within AGENTS.md) must resolve to `### Model Command Syntax` at AGENTS.md line 118.
  - **D-07**: link targeting `conventions/terminal-signal-provider-vocabulary.md` (or a relative path resolving to that file) — the file must exist in the worktree.
  - **EP-8**: link in `README.md` new content targeting `AGENTS.md#provider-contract-crate-oulipoly-provider` must resolve to the net-new heading created by this WU in the same commit.
- **non-fire cases**: All six required cross-links are present in the new sections; all anchor targets exist at the expected lines; `conventions/terminal-signal-provider-vocabulary.md` exists in the worktree; no link uses a stale slug.
- **suggested_action**: For each broken link, correct the anchor slug or file path. Verify pre-existing heading slugs against hookpoints research §3. The EP-8 link requires that both the README new section and the AGENTS.md new section exist in the same commit — neither can be merged independently without breaking the other.
- **confidence**: HIGH

### F-no-banned-identifiers

- **severity**: HIGH
- **evidence_paths**: `AGENTS.md` (lines 362..EOF), `README.md` (lines 988..EOF)
- **summary**: One or more concrete-provider identifiers prohibited by the forbidden-identifier rule appear in the new sections' prose or code-fenced examples.
- **positive evidence**: Any case-insensitive match of the banned-identifier pattern defined verbatim in `contracts/age-179-provider-docs.md § Forbidden behaviors` in `AGENTS.md` lines 362..EOF or `README.md` lines 988..EOF. Pre-existing content (AGENTS.md lines 1–361; README.md lines 1–987) is not in scope for this finding. The banned pattern covers concrete provider names and company names used by the production runner (the pattern is enumerated in the contract, not reproduced here per eval-spec constraints).
- **non-fire cases**: Zero case-insensitive matches of the banned-identifier pattern in the new sections; all code-fenced TOML and Rust examples use provider-neutral placeholder names only (e.g., `acme-provider`, `example-provider`, `my-provider`).
- **suggested_action**: Replace any prohibited identifier with a neutral placeholder. Do not use concrete provider or company names in new prose, headings, or code-fenced examples. Verify by running a case-insensitive grep over `AGENTS.md` lines 362..EOF and `README.md` lines 988..EOF against the pattern from `contracts/age-179-provider-docs.md § Forbidden behaviors`.
- **confidence**: HIGH

### F-parse-only-notices-present

- **severity**: HIGH
- **evidence_paths**: `AGENTS.md` (lines 362..EOF), `README.md` (lines 988..EOF)
- **summary**: The required parse-only notice is absent from one or both of the sections that describe implementing traits or declaring a TOML `provider` reference (A1.S4 and A2.S6), or the notice is present but buried as a footnote or parenthetical rather than prominently displayed.
- **positive evidence**: Absence of either the verbatim phrase `Parse-only in this release` or the verbatim phrase `no effect on routing or execution` in the new AGENTS.md content (A1.S4 scope: `` ### TOML `provider` Field (Implementation Reference) ``) or in the new README.md content (A2.S6 scope: `## Implementing a Provider`).
- **non-fire cases**: Both A1.S4 and A2.S6 contain at least one of the two required verbatim phrases; the notice is placed as a standalone blockquote or visually distinct callout — not a footnote or parenthetical — in each section.
- **suggested_action**: Add a prominent parse-only notice to the missing section. The notice must contain the phrase `Parse-only in this release` or `no effect on routing or execution` (or both) and must be visually non-deniable (e.g., a blockquote `> **Parse-only in this release:** Dynamic loading and runtime dispatch are not implemented in this release; the \`provider = { ... }\` field is recorded by the parser but has no effect on routing or execution.`). This is the OKG-1 mitigation required by the risk profile §8.
- **confidence**: HIGH

### F-okg4-layer-separation-present

- **severity**: HIGH
- **evidence_paths**: `AGENTS.md` (lines 362..EOF), `README.md` (lines 988..EOF)
- **summary**: The required layer-separation orientation sentence is absent from one or both of the sections where it is mandated: A1.S1 (`` ## Provider Contract Crate (`oulipoly-provider`) ``) and A2.S6 (`## Implementing a Provider`).
- **positive evidence**: Absence from A1.S1 or A2.S6 of a sentence or adjacent sentence pair that simultaneously names `providers.toml` AND `ProviderCapabilities` AND contains the verbatim phrase `are currently independent` (or `two layers are currently independent`). All three elements must co-occur in a single sentence or adjacent sentence pair.
- **non-fire cases**: Both A1.S1 and A2.S6 contain a sentence (or adjacent sentence pair) that names `providers.toml`, names `ProviderCapabilities`, and states that the two layers are independent; the sentence makes clear that a `providers.toml` entry does not require a corresponding `ProviderCapabilities` implementation.
- **suggested_action**: Add the layer-separation orientation sentence to the missing section. Required phrasing (from proposal `age-179-AGE-179.md §OKG-4`): "`providers.toml` accounts configure runtime dispatch to CLI binaries; `ProviderCapabilities` trait implementations are a separate Rust interface layer that the runner may call for per-concern operations. The two are currently independent; a `providers.toml` entry does not need a corresponding `ProviderCapabilities` implementation to function." Both sections require this sentence independently.
- **confidence**: HIGH

### F-glossary-four-entries

- **severity**: MEDIUM
- **evidence_paths**: `AGENTS.md` (lines 362..EOF)
- **summary**: One or more of the four required glossary terms are absent from the `### Provider Term Glossary` section (A1.S5).
- **positive evidence**: Absence of any of the following verbatim term identifiers in `AGENTS.md` lines 362..EOF at or after the `### Provider Term Glossary` heading: `account`, `command`, `pool-member`, `implementation-reference`.
- **non-fire cases**: All four term names are present and defined in A1.S5; each definition is present as a bold-term entry or sub-heading with a one-sentence behavioral contract; required cascade cross-links accompany the relevant terms (D-03 with `account`, D-04 with `pool-member`, D-05 with `command`).
- **suggested_action**: Add the missing glossary entry to `### Provider Term Glossary`. Each of the four terms must be present with its own definition. The authoritative one-sentence definitions are in `contracts/age-179-provider-docs.md § A1.S5`. Note: the ticket acceptance signal erroneously refers to "three provider meanings"; the glossary must define all four regardless (OKG-3 resolution, risk profile §8).
- **confidence**: HIGH

### F-dd-02-redesign-note

- **severity**: LOW
- **evidence_paths**: `AGENTS.md` (lines 362..EOF)
- **summary**: The forward-reference note acknowledging the redesign document's distinct `Provider` entity concept is absent from the `### Provider Term Glossary` section (A1.S5).
- **positive evidence**: Absence of the exact string `docs/architecture/provider-accounts-redesign.md` anywhere in `AGENTS.md` lines 362..EOF.
- **non-fire cases**: A1.S5 contains a sentence or short paragraph that names `docs/architecture/provider-accounts-redesign.md` and notes that it uses a related but distinct `Provider` entity concept for a planned future redesign, distinct from the four current runtime glossary entries.
- **suggested_action**: Add a brief forward-reference note to `### Provider Term Glossary` naming `docs/architecture/provider-accounts-redesign.md` explicitly. The note should state that the redesign document's `Provider` entity (a planned CLI-tool entity with sub-entities `Account` and `DiscoveredModel`) is distinct from all four current runtime meanings. The file path must appear verbatim: `docs/architecture/provider-accounts-redesign.md`. See risk profile §7/DD-02.
- **confidence**: HIGH

### F-no-rust-or-ts-or-config-diff

- **severity**: HIGH
- **evidence_paths**: git diff output (`git diff --name-only origin/main...HEAD`), `crates/oulipoly-runtime/src/migration/mod.rs`
- **summary**: The diff includes modifications to files outside the four permitted paths, or the permitted file modifications include deletions or mid-file edits in the pre-existing content ranges rather than additive appends only.
- **positive evidence**: Any of the following in the diff against `origin/main` at `14242c03a0e26127c59daab81b9a4354a01f927d`:
  - Any file path other than `AGENTS.md`, `README.md`, `evals/age-179-provider-docs/eval.md`, or `DECISIONS.md` appears in `git diff --name-only`.
  - A deletion hunk (`-` lines) appears within `AGENTS.md` lines 1–361 or `README.md` lines 1–987.
  - A deletion hunk (`-` lines) appears anywhere in the `DECISIONS.md` diff (`DECISIONS.md` is permitted as append-only; no existing decision-log entries may be removed or modified).
  - `crates/oulipoly-runtime/src/migration/mod.rs` appears in the diff (the function `find_alternate_jsonl_with_boundary` must be entirely unchanged).
- **non-fire cases**: Only four paths appear in `git diff --name-only`: `AGENTS.md` (append-only after line 361), `README.md` (append-only after line 987), `evals/age-179-provider-docs/eval.md` (new file, no prior content), and `DECISIONS.md` (append-only additions to a pre-existing tracked decision log — no deletion hunks). No Rust source files, TypeScript files, Cargo files, or other configuration files are modified.
- **suggested_action**: Revert any unintended change to Rust sources, TypeScript files, configuration files, or other markdown files. Verify with `git diff --name-only origin/main...HEAD` that exactly four paths appear. If `crates/oulipoly-runtime/src/migration/mod.rs` is touched, revert it — modifications to `find_alternate_jsonl_with_boundary` are explicitly prohibited by the contract. `DECISIONS.md` is permitted because it is a pre-existing repository-tracked decision log appended to by this WU per the implementation-pipeline orchestrator's DECISIONS.md recording convention, not a product code or configuration change.
- **confidence**: HIGH

### F-no-pytest-shape

- **severity**: HIGH
- **evidence_paths**: `evals/age-179-provider-docs/eval.md`, repository file tree
- **summary**: The eval file or repository contains pytest-shaped constructs: pytest imports, pytest fixture decorators, `def test_*` function definitions, or a file exists at `tests/test_*.py` or `tools/<wu>-verify/*.py`.
- **positive evidence**: Any of the following in any file touched or created by this WU:
  - `import pytest` in any file
  - `@pytest.fixture` in any file
  - `def test_` in any file
  - A file at path matching `tests/test_*.py`
  - A file at path matching `tools/age-179-verify/*.py`
  Also fires if `evals/age-179-provider-docs/eval.md` contains a fenced code block with a language tag that imports testing libraries.
- **non-fire cases**: `evals/age-179-provider-docs/eval.md` contains only YAML frontmatter, markdown prose, and finding-contract entries; no pytest imports, fixture decorators, or `def test_*` functions appear in any file created by this WU; no file exists at `tests/test_*.py` or `tools/age-179-verify/*.py`.
- **suggested_action**: Remove all pytest-shaped constructs. Per `~/ai/conventions/evals.md` § Anti-scope, the canonical route for markdown/operator/workflow/convention/routing/anchor structural-verification is `WRITE`-state eval-spec authoring at `evals/<slug>/eval.md` — not pytest files, pytest-shaped assertions, or `tools/<wu>-verify/<anything>.py`. Lifecycle is `WRITE`; no runnable detector is required at this stage.
- **confidence**: HIGH
