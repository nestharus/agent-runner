# Oulipoly Plane — Agent Entry Point

## What This Is

**Oulipoly Plane** is a desktop control plane for AI coding agent CLIs
(Claude, Codex, Gemini, OpenCode). It handles discovery, configuration,
load balancing, and monitoring. Built with **Tauri v2** (Rust backend) +
**SolidJS** (frontend) + **Ark UI** components + **Tailwind CSS v4**.

Oulipoly derives from **Oulipo** — a movement combining mathematics and
art through constrained writing. The brand evokes precision, creativity
within structure, mathematical elegance, playful geometry.

## Tech Stack

| Layer | Tech | Notes |
|-------|------|-------|
| Backend | Rust + Tauri v2 | `src-tauri/` — state DB, CLI detection, setup agent, model discovery |
| Frontend | SolidJS + TypeScript | `src/` — reactive UI with signals |
| Components | Ark UI (headless) | Dialog, Field, Steps, Progress, Popover, Switch, Checkbox |
| Styling | Tailwind CSS v4 | `@theme` tokens in `src/app.css`, `tailwind-variants` for component styles |
| Icons | FontAwesome (sharp-solid) | Via `@fortawesome/sharp-solid-svg-icons` + custom SVG components |
| Testing | Vitest (unit) + Playwright (E2E) | `src/__tests__/` and `e2e/` |
| Build | Vite + Bun | `bun run dev`, `bun run build`, `bun run test` |

## Design System: Ollie

The app has a mascot character called **Ollie** — a geometric polyhedron
head built from pastel SVG shapes with a "boil filter" (feTurbulence +
feDisplacementMap) for a hand-drawn wobble aesthetic.

### Brand Colors (from Ollie's body)

```
--color-brand-blue:    #4fc3f7  (cyan facet, also --color-accent)
--color-brand-purple:  #b388ff  (purple facet)
--color-brand-yellow:  #ffd54f  (yellow facet)
--color-brand-orange:  #ff8a65  (orange facet)
```

### Integrated Components

| Component | File | Description |
|-----------|------|-------------|
| **OllieSvg** | `src/components/OllieSvg.tsx` | Portrait head with bob + blink animations, boil filter |
| **InlineSpinner** | `src/components/InlineSpinner.tsx` | Diamond-shaped spinner cycling through all 4 brand colors |

### CSS Animations (in `src/app.css`)

| Animation | Usage |
|-----------|-------|
| `ollie-bob` | 3s portrait head bob |
| `ollie-blink` | 4s eye blink cycle |
| `ollie-spin` | 1.5s diamond spin with color cycling |
| `fade-up` | 0.3s entrance animation, staggered on dashboard rows |
| `slide-in-right` | 0.2s panel slide-in |

### Color Token Migration: COMPLETE

All components use Tailwind theme tokens (`text-text`, `bg-accent`,
`text-error`, etc.) instead of hardcoded hex values. The only remaining
raw hex values are intentional blue-tinted surface variants (`#16213e`,
`#0f3460`, `#1e2a4a`, `#2a3a5e`) that don't have generic token equivalents.

## Current UI Architecture

```
App.tsx
├── SetupView (first-run wizard)
│   └── SetupSession (streaming event handler)
│       ├── StatusBar (with InlineSpinner)
│       ├── ProgressBar (Ark UI Progress)
│       ├── FormRenderer (dynamic forms)
│       ├── WizardStepper (Ark UI Steps)
│       ├── CliSelector (checkbox selection)
│       ├── OAuthFlow (auth instructions)
│       ├── ApiKeyEntry (key input)
│       ├── ConfirmDialog (yes/no)
│       └── ResultDisplay (detection summary, test results)
│
└── PoolsView (pool/model management — currently the home page)
    ├── PoolCard (per-pool row with command tags, model dropdown)
    │   └── Popover (Ark UI, grouped model list with facet chips)
    ├── ModelPanel (Ark UI Dialog, slide-in editor)
    ├── PoolSettingsPanel (flag toggles)
    └── SetupSession (inline for add-pool flow)
```

### Model Naming Convention

Models use `group~facet` format (e.g., `claude~high`, `codex~low`). The
`~` separator triggers grouping in the UI via `src/lib/grouping.ts`.
Standalone models without `~` render as single entries.

## Design Workflow

### External Design Directory

**Location**: `~/work/tmp/oulipoly-e2e/`

This directory contains all design work, organized as packages for an
external design model (LLM that generates SVGs, animations, CSS). It is
NOT inside the git repo — it's a working area for design iteration.

```
~/work/tmp/oulipoly-e2e/
├── design-audit.md          # What's answered vs open from design deliverables
├── dashboard-rethink.md     # Task-centric home screen UX concept
├── packages/                # Individual design packages for the designer
│   ├── 01-mascot/           # ANSWERED — Ollie exists
│   ├── 02-brand-identity/   # PARTIAL — palette yes, wordmark no
│   ├── 03-ollie-animations/ # MOSTLY ANSWERED — has design deliverables
│   │   └── designs/         # Designer's HTML mockups + notes
│   │       ├── design 1.html
│   │       ├── design 2.html
│   │       └── notes.md     # User's feedback on what's aligned/not
│   ├── 04-progress-bar/     # NEEDS DESIGN
│   ├── 06-empty-states/     # NEEDS DESIGN
│   ├── 09-dashboard/        # SUPERSEDED by UX rethink
│   ├── 10-oauth-flow/       # PARTIAL — lock SVG yes, step indicators no
│   ├── 11-indicators/       # NEEDS DESIGN
│   ├── 12-ollie-error/      # NEW — error pose animation
│   ├── 13-ollie-empty/      # NEW — empty state pose
│   ├── 14-home-screen/      # NEW — task-centric action cards
│   ├── 15-wordmark/         # NEW — logo/wordmark
│   ├── 16-inline-indicators/ # NEW — success/error/warning icons
│   └── 17-oauth-steps/      # NEW — OAuth flow step indicators
├── catalog/                 # Organized screenshots from E2E tests
└── results/                 # Raw test results
```

### How Design Packages Work

Each package is a focused ask for the design model (a separate LLM).
A package contains:
- `brief.md` — what's needed, constraints, references
- Screenshots of current UI state
- `robot-concept.jpg` — Ollie reference image
- Optional `reference-*.html` — existing animation code for context

**Key principle**: The designer loses detail on full-page redesigns.
Packages must target ONE specific piece. Give HTML for animations.

### Designer Tendencies to Steer Away From

1. **Chatlog/conversational UI** — app is NOT a chat interface
2. **Sidebar navigation** — app does too few things for persistent nav
3. **Full-page redesigns** — designer drops detail at page scale
4. **Conflicting elements** — sometimes generates elements that fight each other

### Design Deliverables Already Received

Two HTML mockups from the designer live in
`~/work/tmp/oulipoly-e2e/packages/03-ollie-animations/designs/`:

- `design 1.html` — Interactive flow, input required, inline message
- `design 2.html` — Auto-discovery flow, active task panel, input required
- `notes.md` — User's analysis of what's aligned and what's not

These contain the source SVGs and animations that were extracted into
`OllieSvg.tsx`, `InlineSpinner.tsx`, and the CSS keyframes.

## Remaining TODO(design) Comments

These mark places where the UI needs design work that hasn't been done yet:

| File | What's Needed |
|------|---------------|
| `src/views/PoolsView.tsx` | Should become "Configure" sub-screen, not the home page |
| `src/views/SetupView.tsx` | Searching/progress/error Ollie animations (full-body, needs design) |
| `src/components/ProgressBar.tsx` | Ollie walking companion alongside progress bar |
| `src/components/OAuthFlow.tsx` | Step indicators, lock SVG, confused Ollie pose |
| `src/components/ResultDisplay.tsx` | Custom animated indicator SVGs for success/error |
| `src/components/ModelPanel.tsx` | Empty space background treatment |
| `src/components/PoolCard.tsx` | Per-tool visual differentiation (lower priority) |
| `src/App.tsx` | Wordmark/logo branding in header |

## UX Direction: Task-Centric Home Screen

**Read**: `~/work/tmp/oulipoly-e2e/dashboard-rethink.md`

The current app opens to PoolsView (configuration management). The plan
is to replace this with a task-centric home screen:

- **Three action cards**: Run, Status, Configure
- **Ollie as status indicator**: expression reflects system health
- **PoolsView becomes "Configure"** sub-screen
- **New views needed**: HomeView, StatusView
- **New Tauri commands needed**: `get_system_health()`, `get_quota_status()`,
  `get_recent_invocations()`, `get_invocation_stats()`

## Commands

```bash
# Development
bun run dev              # Start Vite dev server (frontend only)
cargo tauri dev          # Start full Tauri app (frontend + backend)

# Testing
bun run test             # Vitest unit tests
bun run test:e2e         # Playwright E2E tests
bunx tsc --noEmit        # TypeScript type check

# Build
bun run build            # Production build
cargo tauri build        # Full app build

# E2E screenshots
bunx playwright test e2e/screenshots.spec.ts  # Generate screenshot catalog
```

## Tauri Backend Structure

```
src-tauri/src/
├── main.rs              # Entry point
├── lib.rs               # Tauri command handlers (IPC bridge)
├── config/              # Model + agent config (TOML files)
├── state/               # SQLite state DB (invocations, health, accounts)
├── setup/               # CLI detection, setup wizard agent, memory
├── discovery/           # Model discovery pipeline
├── balancer/            # Load balancing across providers
├── executor/            # CLI execution engine
└── diagnostics/         # Health checks, error reporting
```

## E2E Test Infrastructure

Tests use a Tauri mock builder that injects fake `window.__TAURI_INTERNALS__`
via `page.addInitScript()`. See `e2e/fixtures/tauri-mock.ts` for the mock
builder and `e2e/fixtures/scenarios.ts` for pre-built mock configurations.

Scenarios: `FRESH_USER`, `CONFIGURED_USER`, `SINGLE_CLI`, `ERROR_SETUP`,
`EMPTY_POOLS`.
