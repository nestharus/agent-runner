# WU-16-01 Audit Risk

## Verdict
LOW

## Findings

No blocking or non-blocking audit-risk findings.

The proposal carries every required Phase 4 audit-risk obligation for
presence and checklist completeness. No missing required section was found,
and no present section was thin enough to require a MEDIUM verdict.

## Observations

1. Anti-scope is present and concrete.

   Evidence: proposal lines 10-72 restate the ticket anti-scope and add
   proposal-derived exclusions for the WU-13 bare-binary suffix contract,
   Tauri bundle contents, adapter script bodies, non-adapter `scripts/`
   entries, frontend/Rust runtime surfaces, runtime version-skew detection,
   automatic PATH installation, and `scripts.tar.gz`.

   Cross-check: this aligns with the ticket boundaries at
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:81-107`
   and anti-scope at
   `/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-16-01/ticket.md:123-133`.

2. Supported-surface track is present and complete.

   Evidence: proposal lines 73-220 cover deployment mode, customer cohort,
   adjacent public/user-reachable paths, blast-radius for unchanged paths,
   release-CI surface, user-install surface, migration path, rollback path,
   and observability.

   Cross-check: this satisfies the implementation-pipeline checklist for a
   supported-surface track at
   `/home/nes/ai/workflows/implementation-pipeline.md:93-95`.

3. Migration path is explicitly declared.

   Evidence: proposal lines 191-196 state "None required" and explain why the
   release-asset and documentation changes are additive while the source-build
   install path remains valid.

   Audit note: "None required" is acceptable here because the proposal makes an
   explicit migration declaration instead of omitting the topic.

4. Rollback path is explicitly declared.

   Evidence: proposal lines 198-206 name the rollback as reverting the
   publish-step `files:` block, the structural-test extension, the README
   release-asset snippet, and optional `scripts/README.md` cross-reference.

   Cross-check: the current release workflow has the exact prior publish shape
   `files: artifacts/*` at `.github/workflows/release.yml:177-181`, so the
   rollback target is concrete.

5. Observability is declared.

   Evidence: proposal lines 208-219 distinguish runtime observability, which is
   not required because runtime is unchanged, from deployment/merge signals:
   GitHub release asset list, softprops upload output, and the structural test
   failing if required adapter paths disappear.

   Cross-check: the current structural test owns the release upload assertion at
   `src-tauri/tests/release_yml_contract.rs:253-262`.

6. Assumption register is present with evidence and falsification paths.

   Evidence: proposal lines 221-332 define A1 through A6. Each assumption has a
   statement, evidence, falsification path, and owner.

   Audit note: this is not just a draft carried forward from the problem map.
   The problem map's draft assumption register at
   `research/16-release-scripts-problem-map.md:367-424` is narrowed into an
   approved proposal register at proposal lines 221-332, satisfying the
   implementation-pipeline rule at
   `/home/nes/ai/workflows/implementation-pipeline.md:96`.

7. Test-intent track is present with one entry per required AC.

   Evidence: proposal lines 334-485 include entries for AC-1, AC-2, AC-3,
   AC-4, AC-5, and AC-6.

   Audit note: the user-required AC set for this audit is AC-1, AC-2, AC-4,
   AC-5, and AC-6. The proposal also includes AC-3, which is acceptable and
   useful because AC-3 is documentation-only and not test-encoded.

8. Each test-intent entry names change or verification risk.

   Evidence: AC-1 does so at proposal lines 338-340; AC-2 at lines 371-373;
   AC-3 at lines 403-405; AC-4 at lines 422-423; AC-5 at lines 440-442; AC-6
   at lines 458-460.

9. Each test-intent entry names intended behavior.

   Evidence: AC-1 does so at proposal lines 341-343; AC-2 at lines 374-377;
   AC-3 at lines 406-409; AC-4 at lines 424-425; AC-5 at lines 443-444; AC-6
   at lines 461-463.

10. Each test-intent entry names selected level.

    Evidence: AC-1 names a particular-integration structural test at proposal
    line 344; AC-2 names a particular-integration structural test plus manual
    release-CI evidence at lines 378-379; AC-3 names documentation review at
    line 410; AC-4 names unit/particular-integration at line 426; AC-5 names CI
    suite / unit and particular-integration at line 445; AC-6 names
    particular-integration structural test at line 464.

11. Each test-intent entry names fixture source or application point.

    Evidence: AC-1 names `.github/workflows/release.yml` parsed by
    `src-tauri/tests/release_yml_contract.rs` at proposal lines 345-349; AC-2
    names the same parsed workflow/test file at lines 380-383; AC-3 names the
    README insertion area at lines 411-412; AC-4 names the release test and
    workflow at lines 427-428; AC-5 names `src-tauri/tests/` and release
    workflow gates at lines 446-448; AC-6 names existing release contract
    assertions at lines 465-468.

12. Each test-intent entry links assumptions where applicable.

    Evidence: AC-1 links A1, A2, A3, and A4 at proposal line 350; AC-2 links
    A1, A2, and A4 at line 384; AC-3 links A5 at line 413; AC-4 links A1 and
    A4 at line 429; AC-5 links A4 at line 449; AC-6 links A1 and A4 at line
    469.

13. Each test-intent entry names expected observable signal.

    Evidence: AC-1 does so at proposal lines 351-362; AC-2 at lines 385-387;
    AC-3 at lines 414-415; AC-4 at lines 430-431; AC-5 at line 450; AC-6 at
    lines 470-472.

14. Each test-intent entry names residual risk or residual path.

    Evidence: AC-1 names the live-release residual and
    `risk/16-release-scripts-test-residuals.md` at proposal lines 363-366;
    AC-2 names the same residual path at lines 395-397; AC-3 names its
    documentation-command freshness residual at lines 416-418; AC-4 names the
    residual path at lines 432-435; AC-5 names the residual path at lines
    451-454; AC-6 names the residual path at lines 473-479.

    Audit note: the proposal also has a consolidated residual-risk artifact
    requirement at lines 481-485, which is consistent with the WU-13-01
    precedent that structural tests cannot prove live release asset publication:
    `risk/13-release-restore-test-residuals.md:25-29`.

15. Net-value statement is present and qualitative.

    Evidence: proposal lines 487-511 state the concrete current-state risk,
    the limited blast radius, the WU-13 precedent surface, and the positive
    supported-surface termination check.

16. Implementation outline is present and remains design-level.

    Evidence: proposal lines 513-608 give the publish-step extension,
    trade-offs, structural-test extension shape, README snippet placement,
    optional `scripts/README.md` cross-reference, and portability notes.

    Audit note: the outline references exact paths, assertion strategy, and the
    command shape already established in earlier proposal sections, but it does
    not include an inline target code patch. The Round 2 changelog at proposal
    lines 647-668 states that prior code-shaped YAML and set-literal blocks were
    removed to keep section 6 design-level only.

17. WU-13-01 release-flow precedent is carried forward.

    Evidence: proposal lines 124-126 preserve WU-13 structural assertions;
    lines 456-479 assign AC-6 to the existing WU-13 release workflow paths and
    residual live-release risk; lines 530-540 cite WU-13's explicit release
    asset pattern.

    Cross-check: WU-13's audit precedent accepted LOW with complete AC-to-
    artifact mappings in `risk/13-release-restore-audit.md:57-59`, and WU-13's
    residuals explicitly separated structural YAML coverage from live
    workflow-dispatch evidence in `risk/13-release-restore-test-residuals.md:25-29`.

18. Current workflow and structural test cross-check support the proposal's
    stated edit point.

    Evidence: `.github/workflows/release.yml:164-181` has a release job that
    checks out the repo, downloads artifacts, creates/pushes the tag, and calls
    `softprops/action-gh-release@v2` with `files: artifacts/*`.

    Evidence: `src-tauri/tests/release_yml_contract.rs:228-262` already locates
    the release job's download step and `softprops/action-gh-release@v2`
    publish step, and lines 264-273 preserve the bare-binary hit scan.

    Audit note: this confirms the proposal's test-intent plan is an additive
    extension point rather than a missing or purely speculative test surface.

## Status
LOW — Phase 4 audit-risk clears without revision.
