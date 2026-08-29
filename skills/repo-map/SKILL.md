---
name: repo-map
description: >
  Orient within the LunCoSim workspace: repository layout, crate ownership,
  runnable binaries, API launch modes, and the right evidence path for a task.
  Use when choosing an app, locating a feature or crate, launching the simulator,
  workbench, or server, using headless mode, or avoiding an ambiguous bare
  `cargo run`. It distinguishes production `luncosim` scene and visual evidence
  from numeric headless execution, identifies `lunica` as the Modelica
  workbench, explains the canonical API port, and points to the authoritative
  application and crate indexes.
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
| `assets/` | Runtime data: `scenes/` (USD), `models/` (Modelica `.mo`), `scripting/` (rhai prelude/examples/tools), `tutorials/`, `ui/` (runtime-authored HTML/CSS-like surfaces), `vessels/`, `shaders/`, `props/`, `missions/`, `config/`. |
| `docs/` | `architecture/` (numbered design docs), `apps/`, `tutorials/`, `crates-index.md`, `scripting-guide.md`, `principles.md`. |
| `specs/` | Numbered feature specs (`NNN-name/spec.md`) — the *intent* behind subsystems. |
| `skills/` | Agent skills (this one, `author-scenario`, `authoring-vessel-controllers`, `run-modelica`, `test-via-api`, `lunco-ui`, `runtime-ui`, `lunco-theme`). |
| `mcp/` | Node MCP server wrapping the HTTP API as tools for AI agents. |
| `scripts/` | `build*.sh`, `check_*.sh` (lints/wasm), `api/` (HTTP helpers), `deploy/`, `perf/`. |

## Binaries — which one do I run?

**Pick by task, not by habit:**

| I want to… | Run | Why |
|---|---|---|
| Ground physics / rovers / USD scenes / Modelica / visual evidence | **`luncosim`** | The production scene/runtime binary; use it for scene tests, screenshots, and visual acceptance. |
| Numeric headless simulation / CI automation | **`luncosim-server`** | The same simulation through `run_headless()`, with no GUI evidence; use it for numeric/API automation. |
| Author / compile / simulate Modelica models, browse MSL | **`lunica`** | The **Modelica** workbench (⚠️ NOT the main sim). |
| Download / verify / process external assets | **`lunco-assets`** | `-- download\|list\|process`. |

Launch (workspace `default-members` make a bare `cargo run` ambiguous — **always pass a target**):

```bash
cargo build -p lunco-luncosim --bin luncosim
target/debug/luncosim
target/debug/luncosim --api 4101
cargo build -p lunco-luncosim-server --bin luncosim-server
target/debug/luncosim-server --api 4101
cargo build -p lunco-modelica --bin lunica
target/debug/lunica --api 4101
```

**Utility / dev bins** (all in `lunco-modelica` unless noted): `modelica_run`
(headless Modelica CLI → CSV), `msl_indexer` (rebuild the MSL search index — re-run
after an MSL change), `lunica_worker` (wasm compile worker, bundled not run),
`build_msl_assets` (`lunco-assets`), `net_smoke` (`lunco-networking`, transport smoke
test). Authored luncosim behavior tests run through `luncosim test` plus their Rhai scenarios.
Details:
[`docs/apps/README.md`](../../docs/apps/README.md).

## Talking to a running app (agents)

The windowed apps that embed the API bridge (`luncosim`, `lunica`, and anything with
`LunCoApiPlugin`) honor:

- `--api [PORT]` — enable the HTTP automation API. Default port **4101**. This is
  mandatory for luncosim visual/runtime validation; use an explicit free port.
  (`lunco_core::session::DEFAULT_API_PORT`); the MCP config points here via
  `LUNCO_API_PORT`. Without `--api`, no network surface.

- `--no-ui` — headless (skip winit/egui, run the shared sim loop).
- `--scene <path>` — (`luncosim`) load a USD scene on boot; path is relative to the
  `assets/` root (do **not** prefix with `assets/`).

Use production `luncosim` for scene-test and visual evidence. Use
`luncosim-server` (or `luncosim --no-ui`) for numeric/headless evidence only;
headless runs cannot prove screenshots, rendering, or visual acceptance. Keep
the binary, revision, readiness state, and evidence type explicit in reports;
these evidence paths are complementary, not interchangeable.

Drive it: `POST /api/commands` with `{"type":"ExecuteCommand","command":"<Name>","params":{...}}`; discover
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
| **Workbench & UI** | IDE shell, widgets, viz, 2D canvas, edit tools, render, web boot | `lunco-workbench`, `lunco-ui`, `lunco-viz`, `lunco-canvas`, `lunco-luncosim-edit` |
| **Scripting & modeling** | Modelica, event-driven Rhai, tools, hooks, behavior trees, tutorials | `lunco-modelica`, `lunco-scripting`, `lunco-tools`, `lunco-hooks`, `lunco-behavior`, `lunco-tutorial` |
| **Applications** | the entry-point binaries above | `luncosim`, `luncosim-server`, `lunica` |

## Where does X live? (routing)

| Looking for… | Go to |
|---|---|
| A subsystem's design/intent | `docs/architecture/NN-*.md` (numbered) or `specs/NNN-*/spec.md` |
| Which crate owns a responsibility | `docs/crates-index.md` |
| How to run/launch anything | `docs/apps/README.md` |
| Writing rover/vehicle behavior (rhai) | skill `author-scenario` + `docs/scripting-guide.md` |
| Authoring a reloadable Twin-facing UI | skill `runtime-ui` + `docs/architecture/runtime-authored-ui.md` |
| A self-driving vessel / GNC / autopilot | skill `authoring-vessel-controllers` |
| Running Modelica / experiments over the API | skill `run-modelica` |
| Verifying a change end-to-end via the API | skill `test-via-api` |
| Runtime data (scenes, models, scripts) | `assets/` (see layout table) |
| Build/lint/deploy helpers | `scripts/` |

## Gotchas / naming traps

- **No `apps/` directory** — every binary lives in a `crates/<crate>/src/{main.rs,bin/}`.
- **`lunica` ≠ the main sim.** It is the Modelica workbench (crate `lunco-modelica`); `luncosim` is the ground-physics simulator and `luncosim-server` is its headless launcher.
- **Do not launch LunCoSim through `cargo run`.** Build the named package/bin,
  then execute `target/debug/luncosim` directly. Bare `cargo run` is also
  ambiguous because the default members are `lunco-luncosim` and `lunco-modelica`.
- **`lunco-luncosim` produces the `luncosim` binary** (crate name ≠ binary name); `luncosim-server` is a *separate crate* (`lunco-luncosim-server`) that exists only to default to headless.
- **API port is 4101** by default; always pass an explicit free port when another
  session owns it.
- **Don't `pkill`** a running app to restart — use the API `Exit` command (see `test-via-api`).
- Composition roots: `lunco-luncosim` = `SandboxCorePlugin` (+ optional UI/headless plugin), shared by both `luncosim` and `luncosim-server`. There is **no** `lunco-usd-composer` crate — composition lives in `lunco-usd-bevy` (`flatten_stage`).
