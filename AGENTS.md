# LunCoSim AI Agent Guidelines

This document provides specific instructions and context for AI agents (Claude, Gemini, Antigravity, etc.) working on the LunCoSim codebase. Adherence to these guidelines is mandatory for maintaining simulation integrity and modularity.

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

### 3.1 Theme binding (`lunco-theme`)

All UI colors, spacing, and rounding come from the `Theme` resource in
`lunco-theme` (see `crates/lunco-theme/README.md` for the full API):

- **No `Color32::from_rgb` / hex literals outside `lunco-theme`.** Add
  a semantic token or register a per-domain override
  (`theme.register_override` / `theme.get_token`) instead.
- **Prefer `theme.tokens.*`** (`accent`, `success`, `warning`, `error`,
  `text`, …) over raw `theme.colors.*` swatches.
- Read via `Res<lunco_theme::Theme>`; in `&mut World` widgets, clone
  the fields you need out of `World` before touching `ui`.
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
  reprojects when the document generation moves; snarl advances
  its `last_seen_gen` to skip echo rebuilds of its own ops.

### 7.4 How to decide
Quick checklist before you write a `Update` system:

- [ ] Can I write this as an observer on a specific event? → do that.
- [ ] Can I gate on `Res::is_changed()` or a fingerprint? → do that.
- [ ] Is the work inherently continuous (animation, input, render)? →
      per-frame is fine, but keep it allocation-free.
- [ ] None of the above → it's probably the wrong abstraction.
      Reshape it.

## 8. Documentation Standards
- **MANDATORY Documentation**: All produced code MUST be documented using Rust's built-in doc comments (`///` for functions/structs/enums and `//!` for modules).
- **Maintenance Focus**: Comments should primarily aid in **system maintenance** for both **human developers and AI agents**.
- **The "Why" Over "How"**: Prioritize explaining the design intent, dependencies, and "why" a particular approach was chosen, rather than just restating what the code does. 
- **Conciseness**: Aim for "the right amount" of documentation—clear, helpful, and never redundant.
