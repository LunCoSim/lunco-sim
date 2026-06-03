# LunCoSim AI Agent Guidelines

This document provides specific instructions and context for AI agents (Claude, Gemini, Antigravity, etc.) working on the LunCoSim codebase. Adherence to these guidelines is mandatory for maintaining simulation integrity and modularity.

## Repository Navigation
AI agents should use [docs/crates_index.md](docs/crates_index.md) as the primary guide for understanding the workspace structure and crate responsibilities.

## Agent Mandates
- **Crate Maintenance**: Whenever a new crate is added to the workspace, the agent MUST update `docs/crates_index.md` to include the new crate in the appropriate category with a concise responsibility summary.

## 1. Project Context
LunCoSim is a digital twin of the solar system built with the Bevy engine. It follows a modular, hotswappable plugin architecture and mandates Test-Driven Development (TDD).

## 2. Core Technologies
- **Bevy Engine**: We are using **Bevy 0.18**.
- **Physics**: Avian3D (0.6.1)
- **Large-scale space**: big_space (0.12.0)
- **Input Management**: leafwing-input-manager (0.20.0)

## 3. The Tunability Mandate
As per Article X of the Project Constitution, **hardcoded magic numbers are forbidden**. 

*   **Visuals**: Colors, line widths, fade ranges, and subdivisions must be stored in Bevy `Resources` (for global settings) or `Components` (for entity-specific settings).
*   **Physics**: Gravity constants, SOI thresholds, and orbital sampling rates must be exposed as configurable parameters.
*   **UI**: Padding, margins, transition speeds, and every color must come from the `lunco-theme` crate — never hard-coded in a panel.
*   **Persisted user preferences** must go through `lunco-settings` (one shared `~/.lunco/settings.json`, namespaced keys). Implement `SettingsSection` on a typed resource and call `app.register_settings_section::<MySection>()`. Do **not** invent new per-feature JSON files. The intentional exceptions — each documented in `docs/architecture/11-workbench.md` § 9/§9b — are `layouts.toml`, `recents.json`, and per-project `workspace-state/<hash>.json` (VS Code `workspaceStorage`-style, path-keyed volatile UI state: active perspective + open docs). Global window geometry still goes through `lunco-settings` (the `"window"` section), **not** a new file.

### 3.1 Theme binding (`lunco-theme`)

All UI colors, spacing, and rounding come from the `Theme` resource in
`lunco-theme` (see `crates/lunco-theme/README.md` + the `lunco-theme`
skill for the full API and decision rules):

- **No `Color32::from_rgb` / hex literals outside `lunco-theme`.**
- **Four tiers, pick the highest that fits:**
  1. `theme.tokens.*` (`DesignTokens`) — generic semantic colours for
     any UI (`accent`, `success`, `warning`, `error`, `text`, …).
  2. `theme.schematic.*` (`SchematicTokens`) — schematic-editor
     colours for any block-diagram domain (wire colours by domain,
     class-kind badges, diagram text).
  3. **Domain extension trait on `Theme`** (e.g. `ModelicaThemeExt`
     in `lunco-modelica/src/ui/theme.rs`) — maps domain type names
     (`"Pin"`, `ClassType::Model`) to tier-2 fields. **No palette
     picks in the trait body**; if the intent isn't in tier 2 yet,
     add it there first.
  4. `theme.register_override(...)` + `theme.get_token(...)` — only
     for user-pinned values that must *not* track the palette.
- **Palette reads (`theme.colors.*`) are only legitimate inside
  `from_palette` builders.** Never in a panel, overlay, or domain
  trait default.
- Read via `Res<lunco_theme::Theme>`; in `&mut World` widgets, clone
  the whole `Theme` out of `World` before touching `ui`.
- Don't call `ctx.set_visuals` — `lunco-ui`'s `sync_theme_system`
  handles it. Dark/light flips via `theme.toggle_mode()`.
- `lunco-workbench` auto-adds `ThemePlugin`; add it explicitly in
  headless UI tests.

## 4. Key Constraints
- **Hotswappable Plugins**: Everything must be a plugin.
- **TDD-First**: Write tests before feature code.
- **Headless-First**: Simulation core must run without a GPU.
- **SysML v2**: Used for high-level system models and "source of truth".
- **Double Precision (f64)**: For all spatial math, physics, ephemeris calculations, and physical properties (mass, dimensions, forces, spring constants, axes), use `f64` or `DVec3`. Single precision (`f32`) is only acceptable for final rendering offsets, UI-level logic, or non-physics signals.
- **Non-Blocking UI (Responsive Mandate)**: Performance-intensive tasks (mesh generation, large-scale ephemeris lookups, physics collider building) MUST be offloaded to `AsyncComputeTaskPool`. Synchronous execution of heavy math in the main thread is forbidden to prevent UI stuttering.

## 4.1. Four-Layer Plugin Architecture

LunCoSim follows a standard simulation software pattern with independent plugin layers. Every feature you implement must fit into one of these layers:

```
Layer 4: UIPlugins (optional)     — bevy_workbench, lunco-ui, domain ui/ panels
Layer 3: SimulationPlugins (opt)  — Rendering, Cameras, Lighting, 3D viewport, Gizmos
Layer 2: DomainPlugins (always)   — Celestial, Avatar, Mobility, Robotics, OBC, FSW
Layer 1: SimCore (always)         — MinimalPlugins, ScheduleRunner, big_space, Avian3D
```

**Rules for agents**:
1. **Never mix layers in a single plugin**. A plugin is either domain logic (Layer 2) OR UI (Layer 4), never both.
2. **UI lives in `ui/` subdirectory**. Domain crates have `src/ui/mod.rs` that exports a `*UiPlugin`. UI code stays in `ui/`.
3. **UI never mutates state directly**. All UI interactions emit `CommandMessage` events. Observers in domain code handle the logic.
4. **Headless must work**. Removing Layer 3 and Layer 4 plugins must leave a functioning simulation. Tests use `MinimalPlugins` only.
5. **Domain plugins are self-contained**. `SandboxEditPlugin` provides logic (spawn, selection, undo). `SandboxEditUiPlugin` provides panels. They are independent.

**Example — correct layering**:
```rust
// crates/lunco-sandbox-edit/src/lib.rs     ← Layer 2: Domain logic
pub struct SandboxEditPlugin;  // spawn, selection, undo — NO UI

// crates/lunco-sandbox-edit/src/ui/mod.rs  ← Layer 4: UI
pub struct SandboxEditUiPlugin;  // registers panels with bevy_workbench
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

The pattern is three macros from `lunco_core` (re-exporting `lunco-command-macro`):

### Defining a command

```rust
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::DocumentId;

/// Open a Modelica file and create a tab for it.
#[Command(default)]                         // ← expands to:
pub struct OpenFile {                       //   #[derive(Event, Reflect, Clone, Debug, Default)]
    pub path: String,                       //   #[reflect(Event, Default)]
}
```

`#[Command]` (no `default`) when the struct can't sensibly default. Use `#[Command(default)]` (the common case) so the HTTP API can fill in omitted fields. Empty unit-style commands take an empty named-fields body: `pub struct Ping {}`.

### Defining the observer

```rust
#[on_command(OpenFile)]                     // ← emits an internal register helper
fn on_open_file(trigger: On<OpenFile>, mut commands: Commands) {
    let path = trigger.event().path.clone();
    /* … */
}
```

The macro keeps `trigger: On<X>` as the synthetic first parameter and binds `cmd = trigger.event()` automatically — bodies that already use `trigger.event()` work unchanged. New observer bodies should prefer `cmd.field`. The generated `__register_*` helper is an internal detail — never call it by hand; list the observer in `register_commands!` (below).

### Result-returning commands (`-> Result<Ack, String>`)

Most commands are fire-and-forget (return `()`). A command whose caller needs a **result** (script stdout, a computed value, a hard pass/fail) instead returns `Result<Ack, String>`:

```rust
#[on_command(RunPython)]
fn on_run_python(_t: On<RunPython>, backends: Res<ScriptBackends>) -> Result<Ack, String> {
    let out = backends.get(ScriptLanguage::Python)
        .ok_or("python backend not registered")?
        .eval(&cmd.code)?;
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "stdout": out });
    Ok(ack)          // Ok → Succeeded, Err → Failed
}
```

The macro records the outcome in `CommandResults` (`lunco-core`) under the request id the transport minted. The caller gets that id back as `command_id` and polls `QueryCommandResult{id}` → `Succeeded(Ack) | Failed(msg) | Pending | Rejected`. In-process triggers (UI `commands.trigger`) set no active id, so nothing is recorded — only transport-dispatched calls are pollable. This is deliberately minimal (one result + states), matching F′/MAVLink/behaviour-tree practice rather than XTCE's multi-stage verifier pipeline; rich long-running lifecycles (queued/progress/cancel) stay as per-domain state (e.g. experiments' `RunStatus`).

### Registering inside `Plugin::build`

```rust
// One source-of-truth list at module scope. Alphabetical for diff hygiene.
// Entries may be bare idents or `module::fn` paths — the path form lets
// observers live in split submodules without per-fn `use` shims.
register_commands!(
    on_open_file,
    on_compile_model,
    nav::on_set_view_mode,      // observer in a submodule → path form
    /* … */
);

impl Plugin for ModelicaCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CloseDialogState>();   // resources
        register_all_commands(app);                // observers + reflect-types in one shot
    }
}
```

`register_commands!()` collapses a per-observer `__register_on_X(app)` boilerplate cascade into a single function call. Adding a new typed command is a three-line change: struct + observer + one entry in the list. (A future change to make commands self-register and drop the list entirely is tracked in `docs/api.md` → "TBD: grouped self-submitting command registration".)

### Field types

- **`DocumentId`** is `Reflect`-derived in `lunco-doc` — use the typed `DocumentId` directly in command fields. **Never `u64` shims.** The HTTP-wire `{"doc": 1}` auto-converts via reflection.
- New domain identifier types should derive `Reflect` for the same reason. Adding `bevy_reflect = "0.18"` to a leaf crate is cheap (no renderer / ECS deps).

### Anti-patterns (do not do this)

```rust
// ✗ Hand-rolled equivalent of #[Command(default)] — verbose, drifts from canonical form
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct Foo { … }

// ✗ Hand-rolled registration — easy to forget either half, no auto-discovery
app.register_type::<Foo>().add_observer(on_foo);

// ✗ Threading u64 doc-ids through commands to dodge a Reflect requirement
pub struct Foo { pub doc: u64 }   // use DocumentId
```

### When NOT to use `#[Command]`

- **Notifications** (system tells the world "X happened"): `DocumentChanged`, `DocumentSaved`, lifecycle events. These are observed *by* domain crates, not invoked by users — hand-rolled `#[derive(Event, Clone, Debug)]` is fine.
- **High-frequency continuous signals** (joystick, drag deltas, telemetry): use the `ControlStream` channel in [`docs/architecture/01-ontology.md`](docs/architecture/01-ontology.md#controlstream), not the Command Bus.

## 5. Implementation Patterns
### Dynamic Update Pattern
When adding a new tunable parameter:
1.  Define/Update a Bevy `Resource` to hold the data.
2.  Use that resource in your `System` queries.
3.  **Prefer reactive dispatch** (change detection, events, cursors) **over per-frame recomputation**. See §8 — per-frame work is the path of least resistance in Bevy, but almost never the right default for UI state that's "stable most of the time".

### Principle Hierarchy
Always verify your implementation plan against `docs/principles.md`. If a feature request conflicts with the project's principles (e.g., suggesting a non-plugin-based architecture), you must flag this to the user and prioritize principle integrity.

## 6. Tooling & Workflow
- **Search Tools**: Always skip the `target/` directory when using `grep` or other search tools to avoid searching generated artifacts.

## 7. UI Responsiveness & Frame Discipline

The app ships with a real-time 3D scene, a Modelica simulator, and a
heavyweight egui UI on top. The frame budget is shared — UI work that
looks "cheap in isolation" still competes with the physics step and
the renderer every tick. Three rules:

### 7.1 Per-frame work is the anti-default
Bevy makes it easy to write `Update` systems that do work every tick.
**Do not treat that as the right shape for UI state.** A system that
runs every frame for information that changes once a minute is a bug,
even if the per-frame cost looks small in a profiler — it burns cache
lines, pushes allocations through the frame, and makes it impossible
to reason about where a spike came from. Ask instead:

1. **Does this state change only on an event?** → react to the event.
   Use observers (`app.add_observer(...)`) or `EventReader`.
2. **Does this state change when a resource mutates?** → gate on
   `Res::is_changed()` / `Query::is_changed()` / `Changed<T>` filters.
3. **Is "nothing changed" the common case but detection requires
   comparing a few values?** → stash a fingerprint
   (`Local<Cursor { last_gen, last_hash, ... }>`), early-return when
   it matches, only do the real work on mismatch. This is what
   `refresh_diagnostics` does — cursor holds `(doc_id, ast_gen,
   error_hash)` and skips all allocations on unchanged frames.
4. **Does the work depend on a monotonic counter (document generation,
   sample index, tick number)?** → store the last-seen counter on the
   consumer and re-run only when the producer has advanced it. Phase α
   diagram projection uses this pattern (`last_seen_gen`).

Only use unconditional-every-frame systems for genuinely continuous
work: the renderer, physics stepping, tool animation ticks, smooth
camera easing. Everything else is reactive.

### 7.2 The UI must stay responsive
The user types, drags, and right-clicks into the same event queue
the physics solver empties. Never block that queue:

- **Keep `Update` systems short.** If a system routinely takes
  >1 ms, break it up, gate it on change, or push the work to a
  background thread / task pool.
- **No synchronous I/O on the UI thread.** Load files, parse large
  sources, and scan directories via `bevy::tasks::AsyncComputeTaskPool`
  and poll the handle with `future::poll_once` each frame. The
  Package Browser's folder scan is the reference implementation.
- **No per-frame allocations in the common path.** `String` clones
  and `Vec` rebuilds that happen on a no-op path are the most
  common offenders — pre-allocate, reuse, or skip entirely.
- **Frame-rate-independent animation.** Anything using time must
  take `dt` from `ui.ctx().input(|i| i.unstable_dt)` (egui) or
  `Time::delta` (bevy). Never assume 60 Hz.

### 7.3 Heavy work goes off-thread or behind a cache
Parsing a large `.mo` file, rasterising an SVG, indexing an MSL
package — none of these belong on the UI thread every frame. Patterns:

- **One-shot + cache**: global `OnceLock<Mutex<HashMap>>` keyed by
  a stable identifier (path, hash, id). Cache-hit returns a
  `Arc<T>` clone. Reference: `svg_bytes_for` in the canvas panel,
  `msl_component_library` in `visual_diagram.rs`.
- **Background task + poll**: `AsyncComputeTaskPool::get().spawn(...)`
  returns a `Task<T>`; `future::poll_once(&mut task)` in an Update
  system yields the result when ready without blocking. Reference:
  the Package Browser's `handle_package_loading_tasks`.
- **Generation-gated recompute**: the canvas diagram only
  reprojects when the document generation moves; the panel advances
  its `last_seen_gen` to skip echo rebuilds of its own ops.

### 7.4 How to decide
Quick checklist before you write a `Update` system:

- [ ] Can I write this as an observer on a specific event? → do that.
- [ ] Can I gate on `Res::is_changed()` or a fingerprint? → do that.
- [ ] Is the work inherently continuous (animation, input, render)? →
      per-frame is fine, but keep it allocation-free.
- [ ] None of the above → it's probably the wrong abstraction.
      Reshape it.

### 7.5 Profiling subsystem — measure, don't guess

When FPS drops, **do not optimise from code reading.** A frame loop runs the
3D scene + Avian + an embedded egui IDE together, and the dominant cost is
rarely the obvious one. Use the profiling subsystem:

```sh
scripts/perf/profile.sh --release            # build → samply → symbolicated hot functions
scripts/perf/profile.sh --release --diag-only # frame time + GPU adapter only (no sudo)
```

Full reference: [`scripts/perf/README.md`](scripts/perf/README.md)
(toolkit, setup, how to read results, mechanics gotchas). Workflow:
**profile → A/B-disable to confirm → fix → re-measure**, in that order.

Two regressions keep recurring; prefer the by-design fix:

- **Never `(*arc).clone()` a heavy, shared, read-only container** (e.g. a USD
  `TextReader`) to read it — that's a deep copy. Borrow `&*arc`; share via
  `Arc`. (This was a real ~⅔-of-frame regression in the USD cosim path.)
- **Once-per-entity setup belongs in an observer** (`OnAdd<T>`), not a polling
  `run_if(Without<Marker>)` system — the latter re-scans the whole scene every
  frame if any code path forgets to insert the marker. If you must poll, mark
  **every** examined entity, including on `else { continue }` exits.

A `run_if`-gated system that still appears in a steady-state profile means its
gate isn't closing — that's the bug, not the cost.

## 8. Documentation Standards
- **MANDATORY Documentation**: All produced code MUST be documented using Rust's built-in doc comments (`///` for functions/structs/enums and `//!` for modules).
- **Maintenance Focus**: Comments should primarily aid in **system maintenance** for both **human developers and AI agents**.
- **The "Why" Over "How"**: Prioritize explaining the design intent, dependencies, and "why" a particular approach was chosen, rather than just restating what the code does. 
- **Conciseness**: Aim for "the right amount" of documentation—clear, helpful, and never redundant.

## 9. Numeric Experiments & Solver Tuning

When a model wouldn't integrate, or solver behaviour required investigation,
record the diagnosis under `docs/numeric-experiments/`. **Read existing reports
before re-deriving** — most stiff-DAE failures fall into one of a few
already-diagnosed buckets.

Each report follows the template in
[`docs/numeric-experiments/README.md`](docs/numeric-experiments/README.md):
problem → symptoms → investigation (including failed hypotheses) → root cause
→ fix → validation → TBDs.

### Known working solver configurations

- **Stiff radiative thermal models** (lunar rover, anything with σT⁴
  networks + tanh hysteresis): `solver = "tr_bdf2"`, `tolerance = 1e-3`
  (not 1e-6), `dt = 3600`. Background:
  [`docs/numeric-experiments/2026-05-28-lunar-thermal.md`](docs/numeric-experiments/2026-05-28-lunar-thermal.md).
  Scales linearly to multi-year horizons.

### Known-failing models — don't waste time tuning solvers

These fail because of structural rumoca gaps, not solver-config gaps.
Picking a different solver or tolerance won't help; only the listed
upstream fix will.

| Model | Failure | Root cause |
|---|---|---|
| `Modelica.Blocks.Examples.PID_Controller` | bails at t≈2.85e-6 on every implicit solver | Uses `initType=SteadyState` + `initial equation der(spring.w_rel) = 0`. Both demand a homotopy/continuation IC solver; rumoca has plain Newton on FD Jacobian → degenerate y₀. Needs **homotopy IC** + ideally **symbolic Jacobian**. |
| `Modelica.Blocks.Examples.RealNetwork1` | rumoca returns `EmptySystem` at stepper init | Compile pipeline produces empty DAE for this model. Separate rumoca compile bug, unrelated to solver tuning. |
| `Modelica.Mechanics.Rotational.Examples.First` | advances to t≈0.073 then fails mid-event | Event-driven dynamics that need restart-after-event. Rumoca's event support is limited. Needs **event detection + state restart**. |

**Symptom that maps here**: bit-identical `fail_t` across solver/tolerance
sweeps means the failure is *deterministic in the solver's first few
steps* — that's the IC solve, not anything tunable from the FastRun API.

### Outstanding solver / numerics tasks (rumoca)

Priority ranking; each links back to the originating experiment report.

1. **Homotopy initialization for consistent-IC solve** (~½–1 week).
   Blocks `Modelica.Blocks.Examples.PID_Controller` and any model with
   `initType=SteadyState` or `initial equation` algebraic constraints.
   This is the most impactful single task right now — would unblock a
   large share of MSL examples that currently fail at IC.
   Origin: this same 2026-05-28 session (PID_Controller diagnosis).
2. **Symbolic Jacobian via rumoca AST + cranelift** (~weeks, highest leverage).
   Replaces finite-difference Jacobian which loses ~9 sig digits on radiative
   terms. Closes most of the remaining gap to OMC/DASSL on stiff models.
   Origin: [2026-05-28 lunar thermal](docs/numeric-experiments/2026-05-28-lunar-thermal.md).
3. **Per-state `atol` vector honoring Modelica `nominal=`** (~1 day).
   `SimVariableMeta.nominal` is parsed but ignored by the solver.
4. **`EmptySystem` compile bug** investigation. Trivial models like
   `Modelica.Blocks.Examples.RealNetwork1` lower to an empty DAE — rumoca
   compile-pipeline regression. No experiment report yet; needs a
   dedicated diagnosis session.
5. **Event detection + state restart**. Needed for models with
   relational `if`/`when` clauses (most physical-modeling models).
   Today rumoca treats events as smooth, which corrupts BDF history.
   Origin: `Modelica.Mechanics.Rotational.Examples.First` mid-sim
   failure.
6. **Tiered `SolverStartupProfile`**. Today's aggressive defaults
   (100k retries, 1e-25 floor, per-step Jacobian) are global. Add
   `StiffRadiative` profile carrying them; keep `Default` conservative.
7. **Flatten `StepperOptions.solver_mode + rk_method`** into one
   `StepperSolver` enum (~30 min). Today's split allows silently-invalid
   combos like `Bdf + Tsit45`.
8. **Tsit45 mass-matrix gating** (~30 min). Reject at `build_stepper`
   time with a clear error instead of failing inside diffsol.
9. **Hairer-Wanner auto-h0** (~1 day). Currently `problem.h0` is
   span-relative (`span/5_000_000`) and silently clamped by BDF/SDIRK
   anyway; only useful once Tsit45 works on DAEs.

### Outstanding tasks (lunco-modelica)

10. **Honor `experiment(Solver=, Tolerance=, Interval=)` annotations**
    at FastRun dispatch time. Half-wired today.
11. **Stiffness diagnostics in the Experiments panel**: on failure,
    show `fail_t` as % of horizon + suggest next solver/tolerance.
    Special case: if `fail_t` is bit-identical to 12+ sig figs across
    two runs with different tolerance, surface the message "This
    looks like an IC-solve degeneracy, not a solver tuning issue —
    see AGENTS.md §9 known-failing models."
