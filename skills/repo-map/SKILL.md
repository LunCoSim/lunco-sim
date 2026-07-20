---
name: repo-map
description: >
  Orientation for the LunCoSim workspace — how the repo is laid out, which
  binaries exist, and WHEN to run each. USE THIS SKILL whenever you need to get
  your bearings: "which app do I run for X?", "how do I launch the sim /
  workbench / server?", "where does <feature> live?", "what crate handles
  Y?", "is there a headless mode?", "how do I run it without a window?", or any
  moment you're about to grep the whole tree to find where something is. Also
  trigger when you catch yourself about to run a bare `cargo run` (ambiguous —
  needs a target), confusing `lunica` with the main simulator, reaching for
  `pkill`, or guessing an API port. It's project-specific: there is NO `apps/`
  dir (binaries live inside crates), `lunica` is the *Modelica* workbench (not
  the flagship sim), `sandbox` / `luncosim` / `sandbox-server` are three
  different entry points into overlapping stacks, and the canonical API port is
  4101. Authoritative indexes: docs/apps/README.md (every binary) and
  docs/crates-index.md (every library crate, grouped by domain).
---

# Repo map — layout, binaries, and when to use them

A Rust/Bevy Cargo workspace: **60+ library crates** + a handful of app binaries,
plus assets, docs, specs, and skills. This skill is the fast orientation; the two
**authoritative, always-current indexes** are:

- **[`docs/apps/README.md`](../../docs/apps/README.md)** — every runnable binary, full CLI flags, launch lines.
- **[`docs/crates-index.md`](../../docs/crates-index.md)** — every library crate, grouped by domain, with responsibilities.

When those disagree with anything here, they win.

## Top-level layout

| Dir | What's in it |
|---|---|
| `crates/` | All Rust code — libraries **and** the app binaries (there is **no `apps/` dir**). |
| `assets/` | Runtime data: `scenes/` (USD), `models/` (Modelica `.mo`), `scripting/` (rhai prelude/examples/tools), `tutorials/`, `vessels/`, `shaders/`, `props/`, `missions/`, `config/`. |
| `docs/` | `architecture/` (numbered design docs), `apps/`, `tutorials/`, `crates-index.md`, `scripting-guide.md`, `principles.md`. |
| `specs/` | Numbered feature specs (`NNN-name/spec.md`) — the *intent* behind subsystems. |
| `skills/` | Agent skills (this one, `author-scenario`, `authoring-vessel-controllers`, `run-modelica`, `test-via-api`, `lunco-ui`, `lunco-theme`). |
| `mcp/` | Node MCP server wrapping the HTTP API as tools for AI agents. |
| `scripts/` | `build*.sh`, `check_*.sh` (lints/wasm), `api/` (HTTP helpers), `deploy/`, `perf/`. |

## Binaries — which one do I run?

**Pick by task, not by habit:**

| I want to… | Run | Why |
|---|---|---|
| Full lunar mission (celestial + ephemeris + solar-scale + full vehicle stack) | **`luncosim`** | The flagship windowed simulator. |
| Ground physics / rovers / USD scenes / edit tools | **`sandbox`** | Physics test bed — windowed, or headless with `--no-ui`. |
| Multiplayer host / CI automation (no GUI ever linked) | **`sandbox-server`** | Same sim as `sandbox` via `run_headless()`, GUI stack never compiled in. |
| Author / compile / simulate Modelica models, browse MSL | **`lunica`** | The **Modelica** workbench (⚠️ NOT the main sim). |
| Download / verify / process external assets | **`lunco-assets`** | `-- download\|list\|process`. |

Launch (workspace `default-members` make a bare `cargo run` ambiguous — **always pass a target**):

```bash
cargo run -p luncosim
cargo run --release -p lunco-sandbox --bin sandbox
cargo run -p lunco-sandbox-server
cargo run --bin lunica
```

**Utility / dev bins** (all in `lunco-modelica` unless noted): `modelica_run`
(headless Modelica CLI → CSV), `msl_indexer` (rebuild the MSL search index — re-run
after an MSL change), `lunica_worker` (wasm compile worker, bundled not run),
`build_msl_assets` (`lunco-assets`), `net_smoke` (`lunco-networking`, transport smoke
test), `joint_minimal` (`lunco-sandbox`, single-joint physics repro). Details:
[`docs/apps/README.md`](../../docs/apps/README.md).

## Talking to a running app (agents)

The windowed apps that embed the API bridge (`sandbox`, `lunica`, and anything with
`LunCoApiPlugin`) honor:

- `--api [PORT]` — enable the HTTP automation API. Default port **4101**
  (`lunco_core::session::DEFAULT_API_PORT`); the MCP config points here via
  `LUNCO_API_PORT`. Without `--api`, no network surface.
- `--no-ui` — headless (skip winit/egui, run the shared sim loop).
- `--scene <path>` — (`sandbox`) load a USD scene on boot; path is relative to the
  `assets/` root (do **not** prefix with `assets/`).

Drive it: `POST /api/commands` with `{"command":"<Name>","params":{...}}`; discover
the live command set with `DiscoverSchema` (it's introspected, never hard-coded). Full
recipe in the [`run-modelica`](../run-modelica/SKILL.md) / [`test-via-api`](../test-via-api/SKILL.md) skills.

## Crate domains at a glance

Crates are grouped into 8 domains in [`docs/crates-index.md`](../../docs/crates-index.md).
Use this to jump to the right one; read the index for the full responsibility.

| Domain | Crates own | Key crates |
|---|---|---|
| **Core foundation** | primitives, docs/journal, time, storage, hashing, cache, settings, theme | `lunco-core`, `lunco-doc`, `lunco-twin-journal`, `lunco-time`, `lunco-storage`, `lunco-hash` |
| **Simulation engine** | celestial, environment, terrain, experiments, cosim | `lunco-celestial`, `lunco-cosim`, `lunco-experiments`, `lunco-terrain-*` |
| **Vessel control & hardware** | mobility, robotics, avatar, FSW/OBC/hardware, controller, autopilot | `lunco-mobility`, `lunco-autopilot`, `lunco-controller` |
| **USD integration** | OpenUSD↔Bevy: visuals, physics, sim schemas, materials | `lunco-usd`, `lunco-usd-bevy`, `lunco-usd-avian`, `lunco-materials` |
| **Networking & API** | replication, HTTP API, telemetry, attributes | `lunco-networking`, `lunco-api`, `lunco-telemetry` |
| **Workbench & UI** | IDE shell, widgets, viz, 2D canvas, edit tools, render, web boot | `lunco-workbench`, `lunco-ui`, `lunco-viz`, `lunco-canvas`, `lunco-sandbox-edit` |
| **Scripting & modeling** | Modelica, rhai bridge, tools, hooks, behavior trees, tutorials | `lunco-modelica`, `lunco-scripting`, `lunco-tools`, `lunco-hooks`, `lunco-behavior`, `lunco-tutorial` |
| **Applications** | the entry-point binaries above | `luncosim`, `lunco-sandbox`, `lunco-sandbox-server`, `lunco-modelica` |

## Where does X live? (routing)

| Looking for… | Go to |
|---|---|
| A subsystem's design/intent | `docs/architecture/NN-*.md` (numbered) or `specs/NNN-*/spec.md` |
| Which crate owns a responsibility | `docs/crates-index.md` |
| How to run/launch anything | `docs/apps/README.md` |
| Writing rover/vehicle behavior (rhai) | skill `author-scenario` + `docs/scripting-guide.md` |
| A self-driving vessel / GNC / autopilot | skill `authoring-vessel-controllers` |
| Running Modelica / experiments over the API | skill `run-modelica` |
| Verifying a change end-to-end via the API | skill `test-via-api` |
| Runtime data (scenes, models, scripts) | `assets/` (see layout table) |
| Build/lint/deploy helpers | `scripts/` |

## Gotchas / naming traps

- **No `apps/` directory** — every binary lives in a `crates/<crate>/src/{main.rs,bin/}`.
- **`lunica` ≠ the main sim.** It's the Modelica workbench (crate `lunco-modelica`). The flagship is `luncosim`; the physics bed is `sandbox`.
- **`cargo run` alone is ambiguous** — `default-members` are `luncosim`, `lunco-sandbox`, `lunco-modelica`. Always `-p <crate>` and/or `--bin <name>`.
- **`lunco-sandbox` produces the `sandbox` binary** (crate name ≠ binary name); `sandbox-server` is a *separate crate* (`lunco-sandbox-server`) that exists only to default to headless.
- **API port is 4101** everywhere — not 3000 (a stale MCP default) and not 3001.
- **Don't `pkill`** a running app to restart — use the API `Exit` command (see `test-via-api`).
- Composition roots: `lunco-sandbox` = `SandboxCorePlugin` (+ optional UI/headless plugin), shared by both `sandbox` and `sandbox-server`. There is **no** `lunco-usd-composer` crate — composition lives in `lunco-usd-bevy` (`flatten_stage`).
