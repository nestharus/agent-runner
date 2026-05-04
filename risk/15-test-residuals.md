# Test residuals — WU-14-02 claude-path-hash

## No live Windows Claude Code probe

- Residual class: external-behavior probe
- Named risk: A1 invalidator / RC-2 Windows-shaped Claude Code project
  directory hashing.
- Technique attempted or considered: deterministic particular-integration
  harness using the authoritative encoder rule from
  `research/15-claude-path-hash-rca.md`, with a Windows-shaped path fixture
  asserted by
  `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs`.
- Scope: the test set verifies that Oulipoly accepts and encodes
  `C:\Users\foo.bar\work_tree\漢字` according to the documented rule, but it
  does not launch a real Claude Code binary on Windows.
- Budget or bound: this Step 6b invocation has no live Windows host or
  real-Claude probing surface. The supported-surface risk gate explicitly
  routes this residual here instead of blocking the WU.
- Result: not verified by Phase 6b runtime tests.
- Remaining residual: a future real-Claude probe on Windows could invalidate
  A1 if Claude Code uses a different project-directory rule than
  anthropics/claude-code#19972 for Windows hosts.
- Invalidating inputs: a successful live Windows Claude Code probe matching
  the rule closes this residual; a live probe showing different hashing
  reopens the WU's encoder assumption.
- Net-value impact: no change. RC-2 still gives deterministic regression
  coverage for Oulipoly's accepted cross-platform string rule, and the
  residual is explicitly named by the proposal.
