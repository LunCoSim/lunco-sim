# LunCoSim AI Agent Guidelines

This document provides specific instructions and context for AI agents (Claude, Gemini, Antigravity, etc.) working on the LunCoSim codebase. Adherence to these guidelines is mandatory for maintaining simulation integrity and modularity.

## Repository Navigation

Start here, in order (new to the codebase? the canonical narrative path is **[docs/README.md → Reading order for newcomers](docs/README.md#reading-order-for-newcomers)**; the list below is the agent-oriented quick map):

1. **[docs/crates-index.md](docs/crates-index.md)** — the map of the ~50-crate workspace and each crate's responsibility. **First stop for "which crate does X".**
2. **[docs/principles.md](docs/principles.md)** — the non-negotiable design principles. Verify every plan against these.
3. **[docs/architecture/](docs/architecture/)** — numbered design docs. The ranges are a legend: **00s** overview/ontology, **10s** systems (document, workbench, API, twin, sim layers), **20s** domains (modelica, usd, cosim, environment, sysml, experiments), **30s** platform (wasm/web), **40s** cross-cutting (asset-io, axes-units). Start at `00-overview.md`.
4. **[specs/README.md](specs/README.md)** — feature-spec status index (Implemented / Partial / Not-built / Superseded).
5. **This file (AGENTS.md)** — the rules below.

## Agent Mandates
- **Crate Maintenance**: Whenever a new crate is added to the workspace, the agent MUST update `docs/crates-index.md` to include the new crate in the appropriate category with a concise responsibility summary.
- **Doc accuracy**: when you rename/remove a crate, type, or binary, grep the docs (`*.md`) for the old name and fix references in the same change — don't leave dangling docs for a later audit.

## 1. Project Context
LunCoSim is a digital twin of the solar system built with the Bevy engine. It follows a modular, hotswappable plugin architecture and mandates Test-Driven Development (TDD).

## 2. Core Technologies
- **Bevy Engine**: We are using **Bevy 0.18**. (Bevy 0.18 renamed `Event`→`Message` for buffered events: `MessageReader<AssetEvent<T>>`, `is_loaded_with_dependencies`.)
- **Physics**: Avian3D (0.6.1) — use its `xpbd_joints` for joints.
- **Large-scale space**: big_space (0.12.0) — f64 floating-origin.
- **Input Management**: leafwing-input-manager (0.20.0)
- **Modelica**: `rumoca` (consumed from its `main` branch) compiles `.mo` → DAE; runtime in `lunco-modelica`, Bevy cosim bridge in `lunco-cosim`.
- **Scripting**: **rhai** is the canonical embedded language (`lunco-scripting`; tool layer `lunco-tools` + `lunco-tools-rhai`). Python is **one-shot eval only** (`RunPython`); Lua/Luau is a *reserved, unimplemented* language id — do not write docs/code implying it works.
- **Networking**: **lightyear** (WebTransport) in `lunco-networking` — shipped: server-authoritative sync, client prediction + Hermite smoothing + reconciliation, RBAC relay gating, headless `--no-ui --host` server.
- **3D/USD**: `openusd` (consumed from `main`); native USD mesh + trimesh colliders via `lunco-usd*` crates.

## 3. The Tunability Mandate
As per Article X of the Project Constitution, **hardcoded magic numbers are forbidden**. 

*   **Visuals**: Colors, line widths, fade ranges, and subdivisions must be stored in Bevy `Resources` (for global settings) or `Components` (for entity-specific settings).
*   **Physics**: Gravity constants, SOI thresholds, and orbital sampling rates must be exposed as configurable parameters.
*   **UI**: Padding, margins, transition speeds, and every color must come from the `lunco-theme` crate — never hard-coded in a panel.
*   **Persisted user preferences** must go through `lunco-settings` (one shared `~/.lunco/settings.json`, namespaced keys). Implement `SettingsSection` on a typed resource and call `app.register_settings_section::<MySection>()`. Do **not** invent new per-feature JSON files. The intentional exceptions — each documented in `docs/architecture/11-workbench.md` § 9/§9b — are `layouts.toml`, `recents.json`, and per-project `workspace-state/<hash>.json` (VS Code `workspaceStorage`-style, path-keyed volatile UI state: active perspective + open docs). Global window geometry still goes through `lunco-settings` (the `"window"` section), **not** a new file.

### 3.1 Theme binding (`lunco-theme`)

All UI colours/spacing/rounding come from the `Theme` resource — **no
`Color32::from_rgb` / hex literals outside `lunco-theme`**. Pick the **highest
tier that fits**: (1) `theme.tokens.*` generic semantic; (2) `theme.schematic.*`
block-diagram; (3) a domain extension trait on `Theme` (e.g. `ModelicaThemeExt`)
mapping domain names to tier-2 fields — **no palette picks in the trait body**;
(4) `register_override` for user-pinned values that must not track the palette.
Palette reads (`theme.colors.*`) are legitimate **only** inside `from_palette`
builders. Read via `Res<lunco_theme::Theme>` (clone the `Theme` out before
touching `ui` in `&mut World` widgets); `lunco-workbench`'s layout loop pushes
visuals (`ctx.set_visuals`) and auto-adds `ThemePlugin` (add it explicitly in
headless UI tests); dark/light via `theme.toggle_mode()`.

**Full decision rules + API:** the `lunco-theme` skill and
[`crates/lunco-theme/README.md`](crates/lunco-theme/README.md).

## 4. Key Constraints
- **Hotswappable Plugins**: Everything must be a plugin.
- **TDD-First**: Write tests before feature code.
- **Headless-First**: Simulation core must run without a GPU.
- **SysML v2**: Used for high-level system models and "source of truth".
- **Double Precision (f64)**: For all spatial math, physics, ephemeris calculations, and physical properties (mass, dimensions, forces, spring constants, axes), use `f64` or `DVec3`. Single precision (`f32`) is only acceptable for final rendering offsets, UI-level logic, or non-physics signals.
- **Non-Blocking UI (Responsive Mandate)**: Performance-intensive tasks (mesh generation, large-scale ephemeris lookups, physics collider building) MUST be offloaded to `AsyncComputeTaskPool`. Synchronous execution of heavy math in the main thread is forbidden to prevent UI stuttering.
- **File I/O through `lunco-storage`**: persist via `lunco_storage::write_file_sync(path, bytes)` (one API, native + wasm) — never raw `std::fs::write`. `lunco-storage` is **I/O only** (no business logic).
- **No internal JSON for logic/change-detection**: JSON is for the API wire and persisted user files, not internal control flow. For change detection fold a `Hasher` instead of serialising to JSON and comparing strings.

## 4.1. Four-Layer Plugin Architecture

LunCoSim follows a standard simulation software pattern with independent plugin layers. Every feature you implement must fit into one of these layers:

```
Layer 4: UIPlugins (optional)     — lunco-workbench, lunco-ui, domain ui/ panels
Layer 3: SimulationPlugins (opt)  — Rendering, Cameras, Lighting, 3D viewport, Gizmos
Layer 2: DomainPlugins (always)   — Celestial, Avatar, Mobility, Robotics, OBC, FSW
Layer 1: SimCore (always)         — MinimalPlugins, ScheduleRunner, big_space, Avian3D
```

**Rules for agents**:
1. **Never mix layers in a single plugin**. A plugin is either domain logic (Layer 2) OR UI (Layer 4), never both.
2. **UI lives in `ui/` subdirectory**. Domain crates have `src/ui/mod.rs` that exports a `*UiPlugin`. UI code stays in `ui/`.
3. **UI never mutates state directly**. UI interactions dispatch typed `#[Command]` events (`ctx.trigger(...)` / `commands.trigger(...)`); observers in domain code do the work — see §4.2. (The obsolete `CommandMessage` has been removed — always use typed commands.)
4. **Headless must work**. Removing Layer 3 and Layer 4 plugins must leave a functioning simulation. Tests use `MinimalPlugins` only.
5. **Domain plugins are self-contained**. `SandboxEditPlugin` provides logic (spawn, selection, undo). `SandboxEditUiPlugin` provides panels. They are independent.

**Example — correct layering**:
```rust
// crates/lunco-sandbox-edit/src/lib.rs     ← Layer 2: Domain logic
pub struct SandboxEditPlugin;  // spawn, selection, undo — NO UI

// crates/lunco-sandbox-edit/src/ui/mod.rs  ← Layer 4: UI
pub struct SandboxEditUiPlugin;  // registers panels with lunco-workbench
```

**Example — correct composition**:
```rust
// Full sim: all four layers
app.add_plugins(DefaultPlugins)           // Layer 1 + 3
   .add_plugins(LunCoAvatarPlugin)        // Layer 2
   .add_plugins(SandboxEditPlugin)        // Layer 2
   .add_plugins(WorkbenchPlugin)          // Layer 4
   .add_plugins(LuncoUiPlugin)            // Layer 4
   .add_plugins(SandboxEditUiPlugin)      // Layer 4

// Headless: only layers 1 + 2
app.add_plugins((MinimalPlugins, ScheduleRunnerPlugin::run_loop(...)))  // Layer 1
   .add_plugins(LunCoAvatarPlugin)        // Layer 2
   .add_plugins(SandboxEditPlugin)        // Layer 2
   // No Layer 3, no Layer 4
```

## 4.2 Typed Commands — `#[Command]` / `#[on_command]` / `register_commands!()`

**Every user-facing intent is a typed `Command`.** UI clicks, HTTP API calls, MCP tool invocations, scripts, and AI agents all dispatch the *same* typed event; observers in domain code do the work. One input shape, one log line, one place to find every entry point.

Three macros from `lunco_core` (re-exporting `lunco-command-macro`): `#[Command(default)]` on the struct, `#[on_command(T)]` on the observer fn, and one `register_commands!(…)` list applied via `register_all_commands(app)` in `Plugin::build`.

```rust
#[Command(default)]                      // = #[derive(Event,Reflect,Clone,Debug,Default)] + #[reflect(Event,Default)]
pub struct OpenFile { pub path: String }

#[on_command(OpenFile)]                  // `cmd = trigger.event()` is bound for you
fn on_open_file(trigger: On<OpenFile>, mut commands: Commands) { /* … */ }

register_commands!(on_open_file, /* … alphabetical */);   // never hand-roll register_type + add_observer
```

**Essentials:** result-returning commands return `Result<Ack, String>` (`Ok`→Succeeded, `Err`→Failed), pollable by id via `QueryCommandResult`. Use the typed `DocumentId` in fields — **never `u64` shims** (the wire `{"doc":1}` auto-converts via reflection). Never hand-roll the derive or the `register_type().add_observer()` pair.

**Full authoring guide** (defining, observers, result-returning, registering, field types, anti-patterns): [`docs/architecture/12-api.md` → *Authoring a typed command*](docs/architecture/12-api.md#authoring-a-typed-command).

### When NOT to use `#[Command]`

- **Notifications** (system tells the world "X happened"): `DocumentChanged`, `DocumentSaved`, lifecycle events. These are observed *by* domain crates, not invoked by users — hand-rolled `#[derive(Event, Clone, Debug)]` is fine.
- **High-frequency continuous signals** (joystick, drag deltas, telemetry): use the `ControlStream` channel in [`docs/architecture/01-ontology.md`](docs/architecture/01-ontology.md#controlstream), not the Command Bus.

### Command policy / RBAC

Transport-dispatched commands (HTTP API, MCP, networking relays) pass through `CommandPolicyRegistry` (`lunco-core/session.rs`) — **open-by-default** today, but the gate is the RBAC seam. Authority roles are `Owner`/`Operator`/`Observer`. When adding a command that should be permission-gated, register its policy there rather than inventing a bespoke check. In-process UI triggers bypass the registry (local user is trusted).

### Same command, every surface — and how to test it

One typed command is reachable from the UI, the HTTP API (`--api PORT`, `{"command":"<Name>","params":{…}}` → `/api/commands`), MCP tools, scripts, and networked peers. To verify a change end-to-end **without** asking the user to click, drive the running app over its HTTP API — see the **`test-via-api`** skill (runbook) and [`docs/architecture/12-api.md`](docs/architecture/12-api.md). Two more project skills exist: **`lunco-theme`** (theming rules) and **`lunco-ui`** (panel patterns) — consult them when touching UI/theme code.

## 5. Implementation Patterns
### Dynamic Update Pattern
When adding a new tunable parameter:
1.  Define/Update a Bevy `Resource` to hold the data.
2.  Use that resource in your `System` queries.
3.  **Prefer reactive dispatch** (change detection, events, cursors) **over per-frame recomputation**. See §7 / [`42-ui-frame-discipline.md`](docs/architecture/42-ui-frame-discipline.md) — per-frame work is the path of least resistance in Bevy, but almost never the right default for UI state that's "stable most of the time".

### Principle Hierarchy
Always verify your implementation plan against `docs/principles.md`. If a feature request conflicts with the project's principles (e.g., suggesting a non-plugin-based architecture), you must flag this to the user and prioritize principle integrity.

## 6. Tooling & Workflow
- **Search Tools**: Always skip the `target/` directory when using `grep` or other search tools to avoid searching generated artifacts.

## 7. UI Responsiveness & Frame Discipline

The frame budget is shared by the 3D scene, the Avian physics step, the
Modelica simulator, and a heavyweight egui UI. Core rules:

- **Per-frame work is the anti-default.** A system that runs every tick
  for state that changes once a minute is a bug. Prefer, in order: an
  **observer** on the event; a **change-detection gate**
  (`Res::is_changed()`, `Changed<T>`); a **fingerprint** `Local<Cursor>`
  early-return; a **generation counter**. Reserve unconditional
  every-frame systems for genuinely continuous work (render, physics,
  animation, input).
- **Never block the UI thread.** No synchronous I/O or heavy parse/index
  on `Update` — offload to `AsyncComputeTaskPool` + `future::poll_once`,
  or cache behind a keyed `OnceLock<Mutex<HashMap>>`. Keep `Update`
  systems short and allocation-free on the no-op path.
- **Frame-rate-independent timing** — take `dt` from `Time::delta` or
  egui `unstable_dt`, never assume 60 Hz.
- **Profile, don't guess.** When FPS drops, run `scripts/perf/profile.sh`
  and A/B-disable to confirm before fixing. Two recurring regressions:
  never `(*arc).clone()` a heavy shared read-only container (borrow
  `&*arc`); do once-per-entity setup in an `OnAdd<T>` observer, not a
  `run_if(Without<Marker>)` poll.

**Full guide** (the four reactive patterns, off-thread/cache recipes,
decision checklist, profiling workflow):
[`docs/architecture/42-ui-frame-discipline.md`](docs/architecture/42-ui-frame-discipline.md).

## 8. Documentation Standards
- **MANDATORY Documentation**: All produced code MUST be documented using Rust's built-in doc comments (`///` for functions/structs/enums and `//!` for modules).
- **Maintenance Focus**: Comments should primarily aid in **system maintenance** for both **human developers and AI agents**.
- **The "Why" Over "How"**: Prioritize explaining the design intent, dependencies, and "why" a particular approach was chosen, rather than just restating what the code does. 
- **Conciseness**: Aim for "the right amount" of documentation—clear, helpful, and never redundant.

## 9. Numeric Experiments & Solver Tuning

When a model won't integrate or solver behaviour needs investigation, record
the diagnosis under `docs/numeric-experiments/` (report template in its
[README](docs/numeric-experiments/README.md)). **Read existing reports before
re-deriving** — most stiff-DAE failures fall into a few already-diagnosed buckets.

The [numeric-experiments README](docs/numeric-experiments/README.md) is the
**solver-tuning reference**: known-working configs (e.g. stiff radiative
thermal → `tr_bdf2`, `tol=1e-3`, `dt=3600`), the **known-failing models** table
(don't tune solvers for structural rumoca gaps), and the ranked
rumoca/lunco-modelica backlog. Shortcut: a bit-identical `fail_t` across
tolerance sweeps is an IC-solve degeneracy, not a tunable.
