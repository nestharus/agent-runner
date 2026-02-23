# Security Audit Report

**Date**: 2026-02-19
**Auditor**: Automated scan (Claude Opus 4.6)
**Repositories**:
1. `/home/nes/projects/agent-runner` (Tauri desktop app, Rust + TypeScript)
2. `/mnt/c/Users/xteam/IdeaProjects/ai-workflow` (Python project)

---

## Executive Summary

Both repositories are in generally good shape regarding secret management. No hardcoded
API keys, passwords, or private keys were found in any tracked files. However, there are
**three issues** that need attention:

| Severity | Issue | Repository |
|----------|-------|------------|
| CRITICAL | `.auto-claude/.env` contains real Linear API key and GitHub token on disk | ai-workflow |
| HIGH | `.npmrc` contains a Font Awesome auth token and is NOT gitignored | agent-runner |
| MEDIUM | `.gitignore` is missing coverage for several secret-bearing file types | agent-runner |

---

## 1. agent-runner Repository

### 1.1 Tracked Files (29 files) -- CLEAN

All 29 tracked files (Rust sources, Cargo manifests, CI workflows, README, LICENSE,
.gitignore) were scanned. No hardcoded secrets found.

- **Rust source files**: No API keys, tokens, passwords, or private keys embedded
  in any `.rs` file. Auth handling stores references (env var names, config paths)
  but never stores actual secret values.
- **Cargo.toml**: Contains only dependency declarations, no secret values.
- **CI workflows**: `ci.yml` and `release.yml` use no secret environment variables.
  The release workflow uses `GITHUB_TOKEN` implicitly via `softprops/action-gh-release@v2`
  (provided automatically by GitHub Actions). No custom secrets are required.
- **tauri.conf.json**: Contains only app metadata and CSP policy. No embedded keys.

### 1.2 .gitignore Coverage -- NEEDS IMPROVEMENT

Current `.gitignore` covers:
- `/src-tauri/target/` (Rust build artifacts)
- `node_modules/`, `dist/`, `.vite/` (frontend artifacts)
- `.idea/`, `.vscode/` (IDE files)
- `.DS_Store`, `Thumbs.db` (OS files)

**Missing entries that should be added:**

```gitignore
# Environment and secrets
.env
.env.*
*.env
.npmrc

# Secret/key files
*.key
*.pem
*.p12
*.pfx
*.jks
*.secret
*.keystore

# CLI config directories that may contain auth tokens
.claude/
.crush/
.factory/
.junie/
.agents/
.auto-claude/

# Tauri signing keys (if updater is enabled)
src-tauri/*.key
src-tauri/*.pem

# SQLite state database (contains invocation history)
*.db
*.db-wal
*.db-shm

# OS/editor extras
*.swp
*.swo
*~
```

### 1.3 Untracked Files on Disk -- TWO ISSUES

**ISSUE (HIGH): `.npmrc` contains real auth token**

File: `/home/nes/projects/agent-runner/.npmrc`

```
@fortawesome:registry=https://npm.fontawesome.com/
//npm.fontawesome.com/:_authToken=1619E852-300E-409B-B563-DF4BC372D935
```

This is a Font Awesome Pro registry auth token. It is NOT currently tracked by git,
but it is also NOT in `.gitignore`, meaning a careless `git add .` or `git add -A`
would commit it.

**Action required**: Add `.npmrc` to `.gitignore` immediately.

**ISSUE (LOW): Agent/CLI config directories present but safe**

The directories `.agents/`, `.claude/`, `.crush/`, `.factory/`, `.junie/` are present
but contain only `skills/` subdirectories with no secrets. However, these tool config
directories could acquire auth tokens in the future. They should be gitignored
preventatively.

### 1.4 CI Configuration -- CLEAN

- **ci.yml**: Runs frontend checks (bun) and Rust checks (cargo fmt, clippy, test).
  No secrets required.
- **release.yml**: Builds multi-platform artifacts and creates GitHub Releases.
  Uses `softprops/action-gh-release@v2` which uses the automatic `GITHUB_TOKEN`.
  No custom GitHub Secrets are needed for the current workflow.

**Note**: If Tauri auto-updater signing is enabled in the future, you will need:
- `TAURI_SIGNING_PRIVATE_KEY` as a GitHub Secret
- `TAURI_KEY_PASSWORD` as a GitHub Secret

### 1.5 TOML Model Configs -- N/A

No TOML model configs exist in this repository (models are loaded from
`~/.config/oulipoly-agent-runner/models/` at runtime).

---

## 2. ai-workflow Repository

### 2.1 Tracked Files -- CLEAN

Comprehensive grep scans for the following patterns found no real secrets in tracked files:
- `sk-` prefixed API keys (OpenAI/Anthropic format)
- `ANTHROPIC_API_KEY=`, `OPENAI_API_KEY=`, etc. with actual values
- Bearer tokens or auth tokens with real values
- Private key blocks (`BEGIN RSA PRIVATE KEY`, etc.)
- Hardcoded passwords

The only matches were:
- **Documentation examples** in `docs/usage/linear-client.yml` and
  `docs/development/general/` security guides -- contain placeholder values like
  `"lin_api_your_key_here"` and `"sk-1234567890abcdef"` with `# gitleaks:allow`
  annotations. These are correctly marked as intentional examples.
- **Test fixtures** in `scripts/tests/unit/dev/test_lint_config.py` -- contain
  `API_KEY = 'test'` with `# pragma: allowlist secret` annotations. Correct.

### 2.2 .gitignore Coverage -- GOOD

The `.gitignore` properly covers:
- `.env`, `.env.local`, `.env.*.local` (secrets)
- `.claude/settings.local.json` (local Claude config)
- `.auto-claude/` (entire directory, which contains the `.env` with real tokens)
- `__pycache__/`, `.venv*/`, `venv*/`, `env*/` (Python artifacts)
- `build/`, `dist/`, `*.egg-info/` (build artifacts)
- `.idea*/`, `.vscode/` (IDE)
- Various cache directories (`.pytest_cache/`, `.mypy_cache/`, etc.)
- `.repositories/` (external repo clones)

**Missing entries that should be added:**

```gitignore
# Secret/key files (defense in depth)
*.key
*.pem
!**/cacert.pem
*.p12
*.pfx
*.secret
*.keystore

# NPM auth tokens
.npmrc
```

### 2.3 Untracked Files on Disk -- ONE CRITICAL ISSUE

**ISSUE (CRITICAL): `.auto-claude/.env` contains real API tokens**

File: `/mnt/c/Users/xteam/IdeaProjects/ai-workflow/.auto-claude/.env`

Contains real secrets:
- `LINEAR_API_KEY=lin_api_HopbXqLwFF...` (Linear API key -- REAL)
- `GITHUB_TOKEN=gho_9aPVevisoe...` (GitHub OAuth token -- REAL)

**Mitigation status**: This file IS protected by `.gitignore` (the `.auto-claude/`
directory is gitignored). It was never committed to git history. The risk is
**mitigated but the secrets themselves should be rotated** as a precaution since
they were read during this audit.

**Recommendation**: Consider using a proper secrets manager (e.g., `pass`, 1Password
CLI, or OS keychain) rather than storing tokens in plaintext `.env` files, even if
gitignored.

### 2.4 .env.example -- CORRECT

The `.env.example` file contains only placeholder values (`your-firecrawl-api-key-here`,
`sk-...`, `csk-...`, etc.) and is correctly committed as a template for developers.

### 2.5 TOML Model Configs -- CLEAN

All 23 TOML model config files in `.agents/models/` were sampled. Every file contains
only:
- `command` -- the CLI tool name (e.g., `"codex"`, `"claude"`, `"gemini"`, `"opencode"`)
- `args` -- command-line arguments (model names, flags)
- `prompt_mode` -- either `"arg"` or `"stdin"`

No API keys, tokens, or secrets are embedded in any TOML config.

### 2.6 MCP Configuration -- CLEAN

- `.mcp.json`: Empty (`{"mcpServers": {}}`)
- `.mcp.yml`: References `${FIRECRAWL_API_KEY}` via environment variable substitution.
  No hardcoded values.

### 2.7 CI Configuration -- CLEAN

Three workflows exist:
- **lint.yml**: Installs and runs linting tools including **gitleaks** (secret scanner)
  and **trivy** (vulnerability scanner). No secrets needed.
- **cold-start-gate.yml**: Performance benchmark. No secrets needed.
- **rebase-check.yml**: Git history check. No secrets needed.

No custom GitHub Secrets are required for any current CI workflow.

### 2.8 .claude/settings.local.json -- CLEAN

Contains only UI preferences (`enableAllProjectMcpServers`, `outputStyle`, etc.).
No secrets. Correctly gitignored.

---

## 3. Recommended .gitignore Additions

### agent-runner `.gitignore` (HIGH PRIORITY)

Add these lines:

```gitignore
# Environment and secrets
.env
.env.*
*.env
.npmrc

# Secret/key files
*.key
*.pem
*.p12
*.pfx
*.secret
*.keystore

# CLI tool config directories (may contain auth tokens)
.claude/
.crush/
.factory/
.junie/
.auto-claude/

# SQLite databases
*.db
*.db-wal
*.db-shm

# Tauri signing keys
src-tauri/*.key
src-tauri/*.pem
```

### ai-workflow `.gitignore` (LOW PRIORITY, defense-in-depth)

Add these lines:

```gitignore
# Secret/key files (defense in depth)
*.key
!**/cacert.pem
*.p12
*.pfx
*.secret
*.keystore
.npmrc
```

---

## 4. GitHub Secrets Recommendations

### agent-runner

No GitHub Secrets are currently required. If you enable Tauri's auto-updater in the
future, you will need to configure:

| Secret Name | Purpose |
|------------|---------|
| `TAURI_SIGNING_PRIVATE_KEY` | Signs update bundles for Tauri updater |
| `TAURI_KEY_PASSWORD` | Password for the signing key |

### ai-workflow

No GitHub Secrets are currently required. All three CI workflows (lint, cold-start-gate,
rebase-check) run without external API access.

If you add workflows that require API calls (e.g., integration tests against
OpenAI/Anthropic), you would need:

| Secret Name | Purpose |
|------------|---------|
| `OPENAI_API_KEY` | OpenAI API access for integration tests |
| `ANTHROPIC_API_KEY` | Anthropic API access for integration tests |
| `FIRECRAWL_API_KEY` | Firecrawl API access for web scraping tests |

---

## 5. Git History Analysis

Both repositories were checked for secrets that may have been committed and later removed:

- **agent-runner**: No `.env`, `.npmrc`, `.key`, `.pem`, or `.secret` files were ever
  committed to any branch. Clean history.
- **ai-workflow**: Only `.env.example` was ever committed (correct -- it's a template).
  The `.auto-claude/.env` was never committed. Clean history.

No git history remediation (e.g., `git filter-branch` or BFG Repo Cleaner) is needed.

---

## 6. Summary of Actions Required

### Immediate (do now)

1. **Add `.npmrc` to agent-runner `.gitignore`** -- the Font Awesome auth token file
   is one `git add .` away from being committed.
2. **Add `.env` and `*.env` patterns to agent-runner `.gitignore`** -- currently no
   .env protection exists at all.

### Recommended (do soon)

3. **Add the full recommended `.gitignore` additions** listed in Section 3 to both repos.
4. **Rotate the Linear API key and GitHub token** in `.auto-claude/.env` as a precaution
   (they were read in plaintext during this audit).

### Optional (defense in depth)

5. **Add a pre-commit hook** using gitleaks or detect-secrets to both repos. The
   ai-workflow CI already runs gitleaks, but a pre-commit hook catches secrets before
   they reach the remote.
6. **Consider using OS keychain or a secrets manager** instead of plaintext `.env` files
   for local development tokens.
