# Phase 4 — Shortcut Risk Gate: WU-14-02 Claude Path Hash

Judge: `claude-opus`
Inputs reviewed:
- `proposals/15-claude-path-hash.md`
- `research/15-claude-path-hash-rca.md`
- `tickets/phase-14:plans/tickets/phase-14/WU-14-02.md` (via `git show`)
- Authoritative rule: anthropics/claude-code#19972 (quoted in RCA)

WU purpose, restated for the gate: replace the slash-only encoder with the
authoritative #19972 string rule, canonicalize cwd before hashing, and keep
empty cwd rejection as the only `SpawnCwdUnsupported` trigger so migrated
JSONL lands at the path Claude Code actually searches for Windows-shape
paths, filtered-character paths, and symlinked workspaces.

## 1. Encoder rule, byte-for-byte vs. #19972

Authoritative Python (RCA, lines 32–36):

```python
def encode_path(path):
    result = path.replace('/', '-').replace('\\', '-')
    result = ''.join([c if (c.isascii() and c.isalnum()) or c == '-' else '-' for c in result])
    return result
```

Proposal pseudocode (`proposals/15-claude-path-hash.md:93-101`):

```text
input = path_for_hash.to_string_lossy()
replaced = input.replace('/', '-').replace('\\', '-')
encoded = ""
for char in replaced.chars():
    if (char.is_ascii() and char.is_alphanumeric()) or char == '-':
        encoded.push(char)
    else:
        encoded.push('-')
return Ok(encoded)
```

Rule comparison:

| Step | #19972 | Proposal | Match |
|------|--------|----------|-------|
| 1. `/` → `-` | `replace('/', '-')` | `replace('/', '-')` | yes |
| 2. `\` → `-` | `.replace('\\', '-')` | `.replace('\\', '-')` | yes |
| 3. survive predicate | `(c.isascii() and c.isalnum()) or c == '-'` | `(char.is_ascii() and char.is_alphanumeric()) or char == '-'` | yes |
| 4. else | `'-'` | `'-'` | yes |

No divergence. The string-input shape is `path.to_string_lossy()`, which
the ticket's Phase 2.5 notes explicitly authorize because non-UTF8 bytes
already become `-` under the filter. The proposal cites that note and
treats it as a justified design commitment, not a corner-cut.

Verdict on encoder rule: full commit, no shortcut.

## 2. Canonicalize-failure fallback (AC-3)

AC-3 mandate (ticket): "If canonicalization fails (path doesn't exist on
disk), fall back to the literal path with a documented warning behavior —
do NOT silently mis-encode."

Proposal (`proposals/15-claude-path-hash.md:121-139`) defines a four-part
contract:

1. `std::fs::canonicalize(cwd)` runs first.
2. On `Ok(resolved)`: hash resolved path (matches RCA's real-Claude
   Probe B which found the session under the resolved-symlink encoded
   directory).
3. On `Err(error)`: hash the literal `cwd` and emit a warning.
4. Warning channel: the existing `stderr: &mut dyn Write` already
   threaded through `migrate_chain_segment` — not a new logging
   dependency. Keeps warnings test-capturable and on the same stream
   as `[migrate]`.

Warning shape is fully specified, not hand-waved:

```text
Warning: Claude project-dir canonicalize failed for provider=<provider> cwd=<cwd>: <error>; falling back to literal cwd
```

Test plan for the fallback exists explicitly (`proposals/15-claude-path-hash.md:337-351`,
"AC-3 canonicalize failure fallback"): inline migration test using a
non-existing path or Windows-shaped path on Unix and a captured `Vec<u8>`
stderr sink, asserting both the encoded output and the warning string
prefix. This means the warning shape is contract-tested, not just
documented.

Verdict on canonicalize fallback: explicitly defined and test-bound.

## 3. `SpawnCwdUnsupported` trigger consistency

Proposal posture (`proposals/15-claude-path-hash.md:108-119`): keep the
variant; narrow the trigger to empty cwd only; remove the
`cwd.is_absolute()` rejection from the encoder.

Internal consistency check:

- The proposal's pseudocode applies the empty check BEFORE canonicalize:
  `if cwd.as_os_str().is_empty(): return Err(SpawnCwdUnsupported {...})`.
- The `is_absolute()` check is explicitly listed as removed, with the
  exact justification needed for AC-2: "Windows-shaped paths such as
  `C:\Users\foo.bar\work_tree\漢字` are string-encoded instead of
  rejected on Unix."
- The proposal's anti-scope (`proposals/15-claude-path-hash.md:21-23`)
  explicitly forbids unrelated `MigrationError` variant changes — so
  the variant is preserved, only its trigger condition tightens.
- The RCA's "Files Involved" lists `claude_project_dir_for` at line 256
  as the single production encoder, called from
  `src-tauri/src/migration/mod.rs:161`. The proposal restates this as
  the only production caller. No callers outside that module are
  expected to construct or match `SpawnCwdUnsupported`, so narrowing
  the trigger does not require cascading caller updates.

Verdict on variant posture: trigger condition is internally consistent
with the encoder accepting Windows shapes; no contradictory state.

## 4. Test plan per RC, exercising the reproduced failure mode

| RC | Reproduced failure | Test fixture | Test exercises the failure? |
|----|--------------------|---------------|-----------------------------|
| RC-1 | non-alnum (`.`, `_`, CJK) preserved instead of filtered to `-` | `tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs` + `path_with_non_alnum` fixture, plus inline migration unit test with filtered chars | yes — RCA shows pre-fix `left` keeps `.`, `_`, `漢字`; post-fix asserts the fully filtered form |
| RC-2 | `is_absolute()` rejects Windows shapes; backslash + drive punctuation not encoded | `tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs`, `windows_shape_path()`, plus inline empty-cwd rejection and Windows-shape literal-fallback unit | yes — asserts encoding to `C--Users-foo-bar-work-tree---` and that empty cwd remains the only `SpawnCwdUnsupported` |
| RC-3 | encoder hashes literal symlink path; real Claude hashes resolved | `tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs`, `symlinked_workspace(base)` under `#[cfg(unix)]` | yes — asserts the resolved-target encoded path differs from the literal-link encoded path (matches the RCA's Probe A vs. Probe B distinction) |

Each RC's selected level is "particular-integration for the RCA harness,
plus unit." Each fixture aligns to the harness file the RCA's Reproduction
section already enumerated as RED. The proposal commits to flipping all
three RED → GREEN as regression tests, not deleting or weakening them
(`proposals/15-claude-path-hash.md:324-333`).

The AC-3 warning shape gets its own contract test (Section 2 above).

Verdict on test plan: every RC has a fixture exercising the reproduced
failure mode plus the inline unit lift.

## 5. Inline-test update (ticket-mandated)

Ticket (`Notes for Phase 2.5+` and `Anti-scope`):

> Do NOT remove the existing `claude_project_dir_for_encodes_absolute_unix_path`
> test; update it to assert the FULL rule (include a `_` or `.` in the
> test fixture so the new filtering is exercised).

Proposal commitments:

- `proposals/15-claude-path-hash.md:42-44` (anti-scope): "Do NOT remove
  the existing `claude_project_dir_for_encodes_absolute_unix_path`
  inline test; update it to assert the FULL rule by including filtered
  characters."
- `proposals/15-claude-path-hash.md:165-167` (AC mapping): "AC-4: inline
  migration tests are updated, including the existing
  `claude_project_dir_for_encodes_absolute_unix_path`, to assert the
  full rule."
- `proposals/15-claude-path-hash.md:354-365` (AC-4 test-intent entry):
  selected level unit; expected observable signal is "inline unit tests
  pass and **include filtered characters in the Unix fixture**."

The ticket's "include a `_` or `.`" minimum is mirrored explicitly by
"by including filtered characters" plus the test-intent entry's
"include filtered characters in the Unix fixture." Verdict on inline
test update: ticket instruction is restated and tied to AC-4.

## 6. "We'll figure it out in Phase 6" hand-waves

Skim for deferrals that would defeat the WU's purpose:

- `proposals/15-claude-path-hash.md:312-315` (RC-3 test-intent): "plus
  unit or helper-level coverage **if Phase 6b needs a focused
  canonicalize fallback fixture**." This is not a defeat — AC-3's
  fallback contract has its own explicit unit test entry
  (`proposals/15-claude-path-hash.md:337-351`); the conditional only
  applies to whether RC-3 itself adds an additional helper-level
  fixture, not to whether the canonicalize fallback is verified at all.

- `proposals/15-claude-path-hash.md:447-453` (Test Residuals): the lack
  of a live Windows Claude Code probe is named explicitly as a residual
  to be recorded in `risk/15-test-residuals.md`. This is the right
  shape: the WU's purpose is the encoder rule, and the rule comes
  from #19972 plus a deterministic in-repo fixture; deferring a live
  Windows binary probe to a test residual is correct, not a shortcut.

- D-010 / D-011 are marked as resolved by this WU rather than rewritten
  or deleted (AC-7), which is the policy-correct posture.

No purpose-defeating deferrals identified.

## Summary against the verdict rule

- Full commit to #19972 rule: yes (Section 1).
- Canonicalize-failure fallback explicitly defined, including warning
  shape: yes (Section 2).
- Test-intent entry per RC actually exercises the RCA failure mode:
  yes (Section 4), plus a separate AC-3 fallback-warning contract test.
- Inline-test update commits to filtered chars per ticket: yes
  (Section 5).
- `SpawnCwdUnsupported` posture internally consistent: yes (Section 3).
- No purpose-defeating Phase-6 hand-waves: yes (Section 6).

The proposal does not implement only RC-1+RC-2; RC-3 has a fixture and
canonicalize is wired into the encoder pseudocode itself. It does not
half-bake the symlink resolver — it uses `std::fs::canonicalize` with
a defined error path. It does not hard-code drive letters; the
generic non-alnum filter handles `:` (the proposal's worked example
even shows `C:\Users\foo.bar\work_tree\漢字 → C--Users-foo-bar-work-tree---`,
matching #19972). It does not skip the inline-test update, and it does
not mark AC-3 satisfied by the canonicalize call alone — it defines
the failure fallback, the warning channel, and the warning shape.

Verdict: LOW
