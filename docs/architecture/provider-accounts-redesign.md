# Provider & Accounts Architecture Redesign

## Problem Statement

The current architecture treats model configuration as a manual, flat process:
users create TOML files with raw CLI flags, organize them into "pools" by
command set, and manage parameters by hand. This has several problems:

1. **Users see implementation details** — raw flags like `--dangerously-bypass-approvals`,
   `-c model_reasoning_effort=high`, `-m gpt-5.3-codex` are exposed directly
2. **No model discovery** — users must know what models exist and how to configure them
3. **No account management** — "pools" conflate CLI tools with auth profiles
4. **No parameter intelligence** — the system doesn't know what parameters are legal
   for a given model or how to present them
5. **No version tracking** — CLI updates can silently break configurations
6. **No quotas** — users can't see their remaining usage

## Core Insight

What we call "pools" are really **accounts** — different authentication profiles
for the same provider CLI tool. A user doesn't think "I have a claude pool with
codex commands." They think "I have two Anthropic accounts and three OpenAI
accounts, and I want to run claude-sonnet-4 load-balanced across all of them."

## New Mental Model

```
Provider (CLI tool: claude, codex, gemini, opencode)
  └── Account (auth profile within that CLI)
       └── Model (discovered from CLI, with typed parameters)
```

### Example

```
Anthropic (claude CLI v1.2.3)
  ├── Account: "work"     (OAuth via claude CLI)
  ├── Account: "personal" (API key: ANTHROPIC_API_KEY)
  └── Available Models:
       ├── claude-sonnet-4  (params: extended_thinking, max_tokens)
       └── claude-haiku-4   (params: max_tokens)

OpenAI (codex CLI v0.9.1)
  ├── Account: "team-a"   (API key: OPENAI_API_KEY)
  ├── Account: "team-b"   (API key via codex config)
  └── Available Models:
       ├── gpt-5.3-codex   (params: reasoning_effort)
       └── o3               (params: reasoning_effort, temperature)

Google (gemini CLI v2.0.0)
  ├── Account: "default"  (OAuth via gemini CLI)
  └── Available Models:
       ├── gemini-3-pro     (params: thinking_budget)
       └── gemini-3-flash   (params: thinking_budget)
```

### Load Balancing Across Everything

When the user asks for `claude-sonnet-4`, we can load-balance across ALL
accounts from ALL providers that offer it. If both Anthropic and a hypothetical
third-party provider support the same model, we balance across all of them.

```
Target: claude-sonnet-4
  ├── Anthropic/work     → claude --profile work --model sonnet-4
  ├── Anthropic/personal → claude --api-key $KEY --model sonnet-4
  └── ThirdParty/main    → thirdparty exec -m claude-sonnet-4
```

The existing balancer (round-robin + error avoidance via SQLite) already
supports this — we just need to map accounts to providers properly.

---

## Data Model

### Current → New Mapping

| Current | New | Notes |
|---------|-----|-------|
| Model TOML file | Model (discovered) | No longer manually created |
| Pool (command set) | Provider + Accounts | Pools were really auth profiles |
| Provider args | Parameters (typed) | Friendly names, AI-discovered |
| `model_names[]` | Available models per provider | Discovered from CLI |

### New Entities

#### Provider

A CLI tool that can execute AI model requests.

```rust
struct Provider {
    cli_name: String,         // "claude", "codex", "gemini", "opencode"
    display_name: String,     // "Anthropic", "OpenAI", "Google"
    installed: bool,
    version: Option<String>,  // detected CLI version
    config_dir: Option<String>,
    auth_methods: Vec<AuthMethod>,  // what auth this CLI supports
    models: Vec<DiscoveredModel>,   // what models this CLI knows about
    last_synced: Option<DateTime>,  // when we last queried the CLI
}

enum AuthMethod {
    OAuth,                    // CLI handles the flow
    ApiKey {
        env_var: String,      // e.g., "ANTHROPIC_API_KEY"
        config_path: Option<String>,  // alternative file-based location
    },
    ConfigFile {
        path: String,         // e.g., "~/.codex/config.toml"
    },
}
```

#### Account

An authenticated profile within a provider CLI.

```rust
struct Account {
    id: String,               // user-chosen label: "work", "personal", "team-a"
    provider: String,         // which CLI
    profile_name: String,     // CLI-specific profile identifier
    auth_method: AuthMethod,  // how this account authenticates
    auth_status: AuthStatus,  // valid, expired, unknown
    quotas: Option<QuotaInfo>,
    created_at: DateTime,
}

enum AuthStatus {
    Valid,
    Expired,
    Unknown,     // haven't checked yet
    NoAuth,      // CLI doesn't require auth for this profile
}

struct QuotaInfo {
    requests_remaining: Option<u64>,
    tokens_remaining: Option<u64>,
    reset_at: Option<DateTime>,
    raw: serde_json::Value,   // provider-specific quota data
    fetched_at: DateTime,
}
```

#### Discovered Model

A model that a provider CLI knows about, with typed parameters.

```rust
struct DiscoveredModel {
    canonical_name: String,   // "claude-sonnet-4", "gpt-5.3-codex"
    provider: String,         // which CLI discovered this
    parameters: Vec<Parameter>,
    discovered_at: DateTime,
    cli_version: String,      // CLI version when discovered
}

struct Parameter {
    name: String,             // friendly: "reasoning_effort", "max_tokens"
    param_type: ParamType,
    description: String,      // AI-generated description
    cli_mapping: CliMapping,  // hidden from user
}

enum ParamType {
    Enum { options: Vec<String> },  // e.g., ["low", "medium", "high", "xhigh"]
    String,
    Number { min: Option<f64>, max: Option<f64> },
    Boolean,
}

/// How a friendly parameter maps to actual CLI arguments.
/// Hidden from the user entirely — they never see dashes or flags.
struct CliMapping {
    flag: String,             // e.g., "-c", "--model", "--reasoning-effort"
    value_template: String,   // e.g., "{value}", "model_reasoning_effort={value}"
}
```

#### Model Selection (replaces current ModelConfig)

What the user actually configures: "I want to use this model with these
parameter values, load-balanced across these accounts."

```rust
struct ModelSelection {
    name: String,                    // user-facing name for this config
    canonical_model: String,         // "claude-sonnet-4"
    parameter_values: HashMap<String, String>,  // friendly_name → value
    accounts: Vec<AccountRef>,       // which accounts to use
    prompt_mode: PromptMode,         // stdin or arg
}

struct AccountRef {
    provider: String,        // CLI name
    account_id: String,      // account label
}
```

---

## Security Model

### OAuth Token Handling

**Rule: OAuth tokens are NEVER used outside the provider's own CLI unless
the user gives EXPLICIT permission.**

OAuth tokens obtained through a provider's CLI belong to that provider's
ecosystem. Using them outside the CLI (e.g., direct API calls) may violate
ToS and result in account bans.

```
OAuth Token Flow:
  1. User initiates auth → we call provider CLI's auth command
  2. CLI handles OAuth flow (browser redirect, token exchange)
  3. CLI stores token in its own config
  4. We ONLY invoke the CLI to use the token
  5. We NEVER read, extract, or reuse the OAuth token directly

API Key Flow:
  1. User provides API key → we store in env var or config
  2. We can use API key for direct API calls (quota checks, etc.)
  3. We can pass API key to any compatible CLI tool
```

### Permission Levels

```
              OAuth Token        API Key
              ───────────        ───────
Use via CLI:  ALWAYS OK          ALWAYS OK
Direct API:   EXPLICIT ONLY      OK
Cross-CLI:    EXPLICIT ONLY      OK (if compatible)
Quota check:  VIA CLI ONLY       DIRECT API OK
```

### Secret Storage

Secrets (API keys, tokens) are managed by the provider CLIs themselves.
We do NOT store secrets. We store:

- Which accounts exist and how they authenticate
- Account labels and metadata
- Auth status (valid/expired) — checked by probing the CLI

For CI/CD, users configure secrets via GitHub Secrets or equivalent.
Our config files contain NO secrets — only references (env var names,
profile names).

---

## AI Agent Architecture

### Agent Hierarchy

```
┌─────────────────────────────────────────────┐
│                   Opus                       │
│  CLI version research, contract discovery,   │
│  parameter introspection, integration        │
│  script generation                           │
│  (background, triggered by version changes)  │
└──────────────────┬──────────────────────────┘
                   │ creates/updates
┌──────────────────▼──────────────────────────┐
│                  Sonnet                      │
│  Complex tasks, tool generation, agent       │
│  creation, multi-step configuration          │
│  (background, escalated from Haiku)          │
└──────────────────┬──────────────────────────┘
                   │ creates/updates
┌──────────────────▼──────────────────────────┐
│                  Haiku                       │
│  User-facing chat, simple config changes,    │
│  parameter explanations, quick actions       │
│  (interactive, on every panel + main page)   │
└─────────────────────────────────────────────┘
```

### Haiku: User-Facing Assistant

Every panel and the main page has a "What would you like to do?" chatbox
powered by Haiku. Haiku handles:

- Explaining what parameters do
- Simple configuration changes ("set reasoning effort to high")
- Answering questions ("what models does my codex account support?")
- Navigating the UI ("show me my OpenAI accounts")

Haiku has access to:
- Current provider/account/model state
- Parameter documentation (AI-generated, cached)
- The user's current panel context

When Haiku can't handle something, it escalates to Sonnet in the background
and tells the user it's working on it.

### Sonnet: Complex Task Handler

Sonnet handles tasks that require multi-step reasoning or tool creation:

- Creating new integration scripts for a provider
- Configuring complex model setups
- Building specialized agents for a provider pool
- Resolving configuration conflicts

Sonnet creates:
- **Tools**: Provider-specific utilities (auth checkers, model listers)
- **Agents**: Specialized assistants for each provider/panel

### Opus: Research & Discovery

Opus handles broad research tasks triggered by system events:

- CLI version change detected → research new capabilities, parameters,
  breaking changes
- New provider added → discover auth methods, model catalog, parameter
  schemas
- Contract changes → update internal parameter mappings, regenerate UIs

Opus output feeds into the parameter/model discovery system, updating
what Haiku and Sonnet know about.

### Specialized Agents

Each provider pool gets its own specialized agent, created by Sonnet:

```
agents/
  ├── provider-anthropic.md     # knows claude CLI specifics
  ├── provider-openai.md        # knows codex CLI specifics
  ├── provider-google.md        # knows gemini CLI specifics
  └── panel-model-config.md     # knows model parameter mapping
```

These agents are regenerated when CLI versions change.

---

## Provider Integration Scripts

Each provider needs an integration layer that knows how to:

1. **Detect** — is the CLI installed? what version?
2. **Authenticate** — initiate auth flow through CLI
3. **List profiles** — enumerate existing accounts/profiles
4. **Create profile** — set up a new account/profile
5. **List models** — query what models are available
6. **Discover parameters** — for each model, what parameters exist
7. **Check quotas** — fetch remaining usage (respecting auth type)
8. **Execute** — run a prompt against a specific account+model+params

These are written per-provider pool (not per-provider), because pools share
the same CLI tool and differ only in auth context.

### Example: Claude CLI Integration

```
detect:
  which claude → path
  claude --version → version string

authenticate:
  claude auth login --profile {profile_name}
  # CLI handles OAuth browser flow

list_profiles:
  claude auth list
  # or scan ~/.claude/profiles/

list_models:
  claude models list
  # parse output for model names

discover_parameters:
  # AI-driven: read claude --help, claude models info {model}
  # extract parameter names, types, valid values

check_quotas:
  # OAuth: claude usage --profile {profile} (must go through CLI)
  # API key: direct API call to /v1/usage

execute:
  claude --profile {profile} --model {model} {param_flags} {prompt}
```

For interactive PTY MCP replacement launches against Claude Code, the `execute` step above is illustrative only — the load-bearing constraint on tool-filter flags lives in [`docs/architecture/claude-proxy-mcp-launch-shape.md`](./claude-proxy-mcp-launch-shape.md), which mandates `--allowedTools mcp__<server>__<tool>,...` (or no filter) and forbids `--tools mcp__<server>__<tool>,...` for that mode. Read that runbook before changing how this integration constructs Claude launch argv.

### Version-Aware Integration

Integration scripts are tagged with the CLI version they were written for:

```
integration:
  provider: claude
  cli_version: "1.2.3"
  last_verified: "2026-02-19"
```

When a version change is detected:
1. System flags the integration as potentially stale
2. Opus researches the changelog / new --help output
3. Opus updates parameter mappings and integration scripts
4. Sonnet regenerates specialized agents
5. Haiku's knowledge is refreshed

For Claude Code specifically, the version-bump procedure is anchored in [`docs/architecture/claude-proxy-mcp-launch-shape.md`](./claude-proxy-mcp-launch-shape.md) — see that runbook's `Version-Bump Runbook` and `Affected Claude Code Versions` sections before marking a Claude integration as re-verified.

---

## CLI Version Detection

### Continuous Monitoring

On app startup and periodically:

```
1. For each known provider:
   a. Run `which {cli}` → check if still installed
   b. Run `{cli} --version` → compare to stored version
   c. If version changed:
      - Flag integration as stale
      - Queue Opus research task
      - Notify user: "codex updated to v0.10.0, re-syncing..."
   d. If newly installed:
      - Queue full discovery (auth methods, models, params)
```

### Version Change Response

```
Version Change Detected
  │
  ├─► Opus: Research new version
  │    ├─ Read changelog / release notes
  │    ├─ Run {cli} --help, {cli} models --help, etc.
  │    ├─ Compare old vs new parameter schemas
  │    └─ Output: updated DiscoveredModel[] + breaking changes
  │
  ├─► Sonnet: Update integrations
  │    ├─ Regenerate integration scripts
  │    ├─ Update specialized agents
  │    └─ Fix any broken configurations
  │
  └─► Haiku: Inform user
       └─ "codex updated: 2 new models, reasoning_effort now supports 'ultra'"
```

---

## UI Design

### Main Page

```
┌─────────────────────────────────────────────────────────┐
│  Agent Runner                                    [+] [⚙] │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  ┌─ Anthropic (claude v1.2.3) ───────────────────────┐  │
│  │  Accounts: [work] [personal]              [+ Add]  │  │
│  │  Models:   claude-sonnet-4  claude-haiku-4          │  │
│  └────────────────────────────────────────────────────┘  │
│                                                          │
│  ┌─ OpenAI (codex v0.9.1) ───────────────────────────┐  │
│  │  Accounts: [team-a] [team-b]              [+ Add]  │  │
│  │  Models:   gpt-5.3-codex  o3                        │  │
│  └────────────────────────────────────────────────────┘  │
│                                                          │
│  ┌─ Google (gemini v2.0.0) ──────────────────────────┐  │
│  │  Accounts: [default]                      [+ Add]  │  │
│  │  Models:   gemini-3-pro  gemini-3-flash             │  │
│  └────────────────────────────────────────────────────┘  │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │  💬 What would you like to do?                    │   │
│  │  ┌──────────────────────────────────────── [Send]│   │
│  │  │ "Add a new OpenAI account for my team"        │   │
│  │  └──────────────────────────────────────────────┘│   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

### Provider Panel (slide-in)

```
┌─────────────────────────────────────┐
│  ◀ Anthropic                        │
│  claude CLI v1.2.3                   │
├─────────────────────────────────────┤
│                                      │
│  Accounts                            │
│  ┌──────────────────────────────┐   │
│  │ work         OAuth    [✓]    │   │
│  │              Quota: 89%      │   │
│  ├──────────────────────────────┤   │
│  │ personal     API Key  [✓]    │   │
│  │              Quota: 45%      │   │
│  └──────────────────────────────┘   │
│  [+ Add Account]                     │
│                                      │
│  Models                              │
│  ┌──────────────────────────────┐   │
│  │ claude-sonnet-4               │   │
│  │   extended_thinking: [on/off] │   │
│  │   max_tokens: [4096      ]    │   │
│  ├──────────────────────────────┤   │
│  │ claude-haiku-4                │   │
│  │   max_tokens: [4096      ]    │   │
│  └──────────────────────────────┘   │
│                                      │
│  ┌────────────────────────────┐     │
│  │ 💬 Ask about this provider  │     │
│  └────────────────────────────┘     │
├─────────────────────────────────────┤
│  CLI: /usr/local/bin/claude          │
│  Config: ~/.claude                   │
│  Last synced: 2 min ago  [↻ Sync]   │
└─────────────────────────────────────┘
```

### Model Configuration Panel (slide-in)

When clicking a model, shows friendly parameter names — never raw flags:

```
┌─────────────────────────────────────┐
│  ◀ claude-sonnet-4                   │
├─────────────────────────────────────┤
│                                      │
│  Parameters                          │
│                                      │
│  Reasoning Effort                    │
│  ○ low  ○ medium  ● high  ○ xhigh   │
│                                      │
│  Max Tokens                          │
│  [4096                          ]    │
│                                      │
│  Extended Thinking                   │
│  [✓] Enabled                         │
│                                      │
│  Load Balance Across                 │
│  [✓] Anthropic / work               │
│  [✓] Anthropic / personal           │
│  [ ] ThirdParty / main              │
│                                      │
│  ┌────────────────────────────┐     │
│  │ 💬 Ask about this model     │     │
│  └────────────────────────────┘     │
├─────────────────────────────────────┤
│  [Save & Test]                       │
└─────────────────────────────────────┘
```

Parameters are rendered dynamically based on `ParamType`:
- `Enum` → radio buttons or chips
- `Boolean` → toggle/checkbox
- `Number` → number input with min/max
- `String` → text input

The user never sees `-c model_reasoning_effort=high`. They see
"Reasoning Effort: high". The `CliMapping` handles translation internally.

---

## Faceted Grouping (Retained)

The `~` separator in filenames is still useful for our internal config layer.
When we create a `ModelSelection`, the filename encodes the model + parameter
variant:

```
claude-sonnet-4~high.toml     → model: claude-sonnet-4, reasoning_effort: high
claude-sonnet-4~low.toml      → model: claude-sonnet-4, reasoning_effort: low
```

This enables the faceted chip UI in the model list — users see grouped
variants rather than a flat list. The grouping is purely a UI concern and
doesn't affect the backend data model.

The difference from the current approach: facets are now derived from
discovered parameters rather than manually named. The system knows that
"high" and "low" are values of the "reasoning_effort" parameter because
it discovered that from the CLI.

---

## Migration Path

### Phase 1: Provider & Account Layer (Backend)

1. Add `Provider` entity to SQLite (replaces implicit pool detection)
2. Add `Account` entity to SQLite (replaces pool command grouping)
3. Add provider integration trait/interface
4. Implement claude CLI integration (detect, auth, list models)
5. Implement codex CLI integration
6. Implement gemini CLI integration

### Phase 2: Model Discovery (Backend + AI)

1. Build parameter discovery pipeline (AI-driven, reads CLI help)
2. Store `DiscoveredModel` + `Parameter` in SQLite
3. Build `CliMapping` translator (friendly name → raw flags)
4. Add CLI version tracking + staleness detection
5. Implement quota fetching (per auth type)

### Phase 3: UI Redesign (Frontend)

1. Replace PoolCard with ProviderCard (accounts + models)
2. Build dynamic parameter UI (rendered from ParamType)
3. Add chatbox component (Haiku-powered)
4. Build account management panel (add/remove accounts, auth flows)
5. Build model configuration panel (friendly parameters, load balancing)

### Phase 4: Agent Infrastructure

1. Build Haiku chat integration (per-panel + main page)
2. Build Sonnet escalation pipeline
3. Build Opus research pipeline (CLI version change → discovery)
4. Generate specialized provider agents
5. Build tool creation system (Sonnet creates provider-specific tools)

### Phase 5: Advanced Features

1. Cross-provider load balancing for same model
2. Quota-aware routing (prefer accounts with remaining quota)
3. Real-time quota display
4. Auto-update on CLI version changes
5. Integration script marketplace / sharing

---

## Storage Layout (New)

```
~/.config/oulipoly-agent-runner/
  ├── config.toml                    # global settings
  ├── models/                        # ModelSelection TOML files (faceted)
  │   ├── claude-sonnet-4~high.toml
  │   ├── claude-sonnet-4~low.toml
  │   └── gpt-5.3-codex~medium.toml
  └── agents/                        # agent configs (some auto-generated)
       ├── provider-anthropic.md
       ├── provider-openai.md
       └── user-custom.md

~/.local/share/oulipoly-agent-runner/
  └── state.db                       # SQLite (extended schema)
       ├── providers                 # installed CLIs + versions
       ├── accounts                  # auth profiles per provider
       ├── discovered_models         # models found in CLIs
       ├── parameters                # typed params per model
       ├── cli_mappings              # friendly name → raw flag
       ├── provider_states           # invocation counts, errors
       ├── invocations               # execution history
       └── memory_*                  # graph + sessions (existing)
```

---

## Open Questions

1. **Profile enumeration**: How does each CLI expose its profiles? Do they
   all support `--profile`? Or is it env vars, config files, etc.? Needs
   per-provider research.

2. **Model catalog**: Do CLIs expose their full model catalog programmatically,
   or do we need to scrape `--help` output and documentation? Likely varies
   per provider.

3. **Parameter schema**: Is there a machine-readable way to get parameter
   schemas from CLIs, or is this always AI-driven discovery? Some CLIs may
   have `--help --json` or similar.

4. **Quota APIs**: Which providers expose quota/usage APIs? What are the
   endpoints? Are there rate limits on checking quotas?

5. **Cross-provider model identity**: How do we determine that "claude-sonnet-4"
   on Anthropic's CLI is the same model as "claude-sonnet-4" on a third-party
   CLI? Canonical model name registry?

6. **Offline mode**: What happens when a CLI is unavailable? Cache last-known
   model catalog and parameters? Allow execution attempts anyway?

7. **Multi-user**: If multiple users share a machine, how do we handle
   conflicting CLI configs? Probably out of scope — each user has their own
   `~/.config`.
