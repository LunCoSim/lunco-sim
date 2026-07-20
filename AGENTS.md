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

## Before You Write Code — prior art, layer, no legacy

Most of the worst code in this repo's history was not badly written. It was written at the
wrong layer, or it reinvented something that already had a standard spelling.

**1. Is there already a standard?** Check USD/OpenUSD before inventing a schema — UsdLux,
UsdGeom, UsdShade and UsdPhysics already express most of what a simulator needs, and a
standard spelling composes, round-trips, and opens in other tools. A custom `lunco:*`
attribute does none of that. Ambient light was a custom `lunco:env:ambientBrightness`; it
is an untextured `UsdLuxDomeLight`. Camera exposure was a Bevy constant; `UsdGeomCamera`
declares `exposure:iso`/`:time`/`:fStop`/`:responsivity`.

**The test, before you type `lunco:`:** name the standard field this quantity would have
if USD had thought of it. If you can name one, USD *did* think of it — use it. A vendor
namespace is only correct when USD has **no concept at all** for the thing, and then it
should cover only the genuinely new part.

The lathe is the worked example, both ways. `lunco:lathe:profile` / `throatRadius` /
`contour` are legitimate: USD has no surface-of-revolution schema — the parametric gprims
are Sphere/Cube/Cylinder/Cone/Capsule/Plane, and `UsdGeomNurbsPatch` is a *result* format
(points, weights, knots), not a generator. But the same schema also shipped
`lunco:lathe:rings` and `lunco:lathe:vOrder`, which were simply `vVertexCount` and `vOrder`
— standard `NurbsPatch` fields — under second names. Sampling density and polynomial degree
are properties of the patch; only the *shape* was new. Two spellings of one quantity is the
same defect as rule 3, arrived at from the other side.

Also prefer a widely-adopted external standard to a bespoke blob: the mission ephemeris is
CCSDS OEM / SPICE SPK, not a hand-rolled JSON schema.

**2. Which layer?** Ask in order, stop at the first that fits. **Rust is the last resort,
not the default.**

| Layer | For | You are in the right place when |
|---|---|---|
| **USD** | scene description: geometry, lights, materials, cameras, camera *paths*, bodies, joints, sensors, composition | a human could see and edit it in usdview |
| **Modelica** | continuous dynamics — thermal, electrical, propulsion, structural; anything with `der()` | you are writing an equation, not a procedure |
| **Behaviour tree** | sequencing and mission logic | you were about to write a state machine with an index and a pile of flags |
| **rhai** | scenario glue, per-scene policy | it reads as intentions, not a computation |
| **Rust** | kinematics and dynamics (avian), engine mechanism, hot paths | it must be fast, or it is what the layers above stand on |

**Rust owns rigid-body physics; Modelica owns everything else that evolves.** Bodies,
colliders, contacts and joints are the solver's — do not re-derive them in an equation.
Thermal, electrical, propulsion and structural dynamics are Modelica's, and reach physics
through cosim ports. Modelica running GNC or flight-software math is fine — an equation is
an equation — but a Modelica model must never become a second physics engine.

**Physics ports vs sensors — two layers, and mixing them is a bug.**

| | Physics ports | Sensors |
|---|---|---|
| Exposed because | the body/collider EXISTS | someone AUTHORED an instrument in USD |
| Ports | `position_*`, `velocity_*`, `contact`, `contact_force` | `range`, `accel_*`, `spec_force_*`, `contact` |
| Adds | nothing — it is ground truth | mount offset, range limits, out-of-range mode, noise, failure |
| Read by | **physical parts** — a strut, a damper, a structure | **flight software** — GNC, OBC, autopilot |

A physical part reads PHYSICS. A landing-leg strut compresses because its pad is being
pushed on, so `LegStrut.mo` takes `contact_force` off the pad's collider — gating that
behind an authored sensor would mean a spring that only compresses if someone remembered
to install a switch. Flight software reads SENSORS, because a computer only knows what its
instruments tell it: `DescentGuidance` reads the altimeter, with its mount point, its
`rangeMax` and its out-of-range behaviour, not the true height.

Getting this backwards caused a real bug: the struts were gated on the ALTIMETER, whose
datum sits 3.3 m above the pads, so a hand-copied `contact_alt` constant had to restate the
geometry — and it was wrong, firing the legs 3.9 m before touchdown.

**A sensor READS physics, it never re-derives it.** One computation, two consumers: the
touchdown switch and the collider contact ports both call `avian::contact_of`. Two copies
are free to disagree, and nothing in the log says which is right.

**No per-tick computation.** Prefer an on-demand port read to a mirror component kept in
step by a sync system, and a `Changed<T>`-filtered system to an unfiltered one. The avian
port groups read straight off avian's components and contact graph when something asks; the
lathe re-meshes only when a parameter changes. Per-tick work in rhai is forbidden outright
(see 5). The exception is a rhai *test*, where per-tick stepping is the point.

Both campaign fixes followed this: a per-frame trigonometry camera in rhai became a
`BasisCurves` prim (a curve you can drag beats code you cannot see until you record it);
a hand-rolled shot state machine duplicated across two episodes became one behaviour tree.

**Prefer a USD feature to scripting it in rhai.** Before writing a script that computes
something about the scene, check whether USD already expresses it — curves, xform ops,
composition arcs (`references`/`over`/`payload`/variants), relationships, time samples,
`UsdSkel`, `UsdPhysics` joints. A prim is inspectable in usdview, editable without a
rebuild, diffable, and composes with layers; the equivalent rhai is none of those, and it
only runs when the scenario runs. The camera above is the canonical case: the *same* move
as 30 authored control points is a thing you can see, drag, and hand to someone else.
Script the parts USD genuinely cannot express — decisions, timing, vehicle commands.

**3. No legacy, shims, or fallbacks.** Replace a mechanism and delete the old one in the
*same* change. Two spellings of one fact means two writers, and which wins becomes a
function of load order — that is exactly how a scene that rendered correctly went dark
when someone gave it a sky. If a migration truly cannot land at once, say so and leave a
`TODO` naming the trigger; never a silent half-state. **A write with no reader is worse
than no write** — it makes the journal claim a setting persisted when it did not.

**4. Reuse, don't reinvent.** Check for a maintained crate (the repo already leans on
`openusd`, `avian3d`, `big_space`, `rumoca`, `catppuccin`). **Reach into the crate before
writing your own** — `openusd` in particular already knows how to resolve composition arcs,
walk stages, and read typed attributes, so a hand-rolled path parser, a bespoke attribute
reader, or a private re-implementation of an arc is almost always a sign the crate's API
was not read. If the crate genuinely lacks something, the honest move is a narrow wrapper
(or an upstream patch) with a comment saying what was missing — not a parallel
implementation that drifts from it. Check for an existing pattern
here — `lunco-doc`/`lunco-doc-bevy`, `lunco-usd`/`lunco-usd-bevy`,
`lunco-render`/`lunco-render-bevy` are one split applied three times; a fourth should look
like them. Check the actual spec, not your memory of it. Reinventing is sometimes right,
but it should be a defended decision recorded in a comment, not a default.

**5. No math in rhai.** It is interpreted scenario glue — no per-tick numerics, control
loops, or vector algebra. Those go in Rust (fast, tested) or Modelica (equations).
**Prefer events to polling:** `wait_for("cmd:PossessVessel")` costs nothing while idle;
a per-tick condition check costs the same whether anything happened or not.

## 1. Project Context
LunCoSim is a digital twin of the solar system built with the Bevy engine. It follows a modular, hotswappable plugin architecture and mandates Test-Driven Development (TDD).

## 2. Core Technologies

Versions are authoritative in the workspace `Cargo.toml` — **check there, not here**, and
fix this list if it drifts.

- **Bevy 0.19** — buffered events are `Message` (`MessageReader<AssetEvent<T>>`), not `Event`.
- **Physics**: Avian3D 0.7 — `xpbd_joints` for joints. `Position`/`Rotation` are *required components* of `RigidBody` and default to zero until derived.
- **Large-scale space**: big_space (pinned git rev) — f64 floating-origin.
- **Input Management**: leafwing-input-manager 0.21
- **Modelica**: `rumoca` (consumed from its `main` branch) compiles `.mo` → DAE; runtime in `lunco-modelica`, Bevy cosim bridge in `lunco-cosim`.
- **Scripting**: **rhai** is the canonical embedded language (`lunco-scripting`; tool layer `lunco-tools` + `lunco-tools-rhai` for script-binding + `lunco-tools-bevy` for behaviour-tree `run_tool` action dispatch). Python is **one-shot eval only** (`RunPython`); Lua/Luau is a *reserved, unimplemented* language id — do not write docs/code implying it works.
- **Networking**: **lightyear** (WebTransport) in `lunco-networking` — shipped: server-authoritative sync, client prediction + Hermite smoothing + reconciliation, RBAC relay gating, headless `--no-ui --host` server.
- **3D/USD**: `openusd` (consumed from `main`); native USD mesh + trimesh colliders via `lunco-usd*` crates.

## 3. The Tunability Mandate

**Hardcoded magic numbers are forbidden** (Article X of the Project Constitution).

- **Visuals** — colours, line widths, fade ranges, subdivisions live in Bevy `Resources` (global) or `Components` (per-entity).
- **Physics** — gravity constants, SOI thresholds, sampling rates are configurable parameters.
- **UI** — padding, margins, transition speeds and **every colour** come from `lunco-theme`, never a panel literal.
- **Persisted preferences** go through `lunco-settings` (one `~/.lunco/settings.json`, namespaced): implement `SettingsSection` and call `app.register_settings_section::<T>()`. Do **not** invent per-feature JSON files. The documented exceptions (`docs/architecture/11-workbench.md` §9/§9b) are `layouts.toml`, `recents.json`, and per-project `workspace-state/<hash>.json`. Window geometry still goes through `lunco-settings`.

### 3.1 Theme binding (`lunco-theme`)

All UI colour/spacing/rounding comes from the `Theme` resource — **no `Color32::from_rgb`
or hex literals outside `lunco-theme`**. Use the **highest tier that fits**:
(1) `theme.tokens.*` semantic; (2) `theme.schematic.*` block-diagram; (3) a domain
extension trait (e.g. `ModelicaThemeExt`) mapping domain names to tier-2 fields — **no
palette picks in the trait body**; (4) `register_override` for user-pinned values that must
not track the palette. Palette reads (`theme.colors.*`) are legitimate **only** inside
`from_palette` builders.

Read via `Res<lunco_theme::Theme>` (clone it out before touching `ui` in `&mut World`
widgets). `lunco-workbench` pushes visuals and auto-adds `ThemePlugin` — add it explicitly
in headless UI tests. Dark/light via `theme.toggle_mode()`.

**Full rules + API:** the `lunco-theme` skill and [`crates/lunco-theme/README.md`](crates/lunco-theme/README.md).

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

**Example** — `lunco-sandbox-edit` splits `SandboxEditPlugin` (src/lib.rs, Layer 2: spawn,
selection, undo — no UI) from `SandboxEditUiPlugin` (src/ui/mod.rs, Layer 4: panels).
A full app adds all four layers; a headless one adds `MinimalPlugins` + Layer 2 only and
must still simulate correctly.

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

The frame budget is shared by the 3D scene, the Avian step, the Modelica simulator and a
heavyweight egui UI.

- **Per-frame work is the anti-default.** A system running every tick for state that
  changes once a minute is a bug. Prefer, in order: an **observer** on the event; a
  **change-detection gate** (`Res::is_changed()`, `Changed<T>`); a **fingerprint**
  `Local<Cursor>` early-return; a **generation counter**. Reserve unconditional per-frame
  systems for genuinely continuous work — render, physics, animation, input.
- **Never block the UI thread.** No synchronous I/O or heavy parse/index on `Update` —
  offload to `AsyncComputeTaskPool` + `future::poll_once`, or cache behind a keyed
  `OnceLock<Mutex<HashMap>>`. Keep `Update` short and allocation-free on the no-op path.
- **Frame-rate-independent timing** — take `dt` from `Time::delta` or egui `unstable_dt`.
- **Profile, don't guess.** Run `scripts/perf/profile.sh` and A/B-disable before fixing.
  Two recurring regressions: never `(*arc).clone()` a heavy shared read-only container
  (borrow `&*arc`); do once-per-entity setup in an `OnAdd<T>` observer, not a
  `run_if(Without<Marker>)` poll.
- **~1 FPS when backgrounded is NORMAL** — winit/OS power-save throttles unfocused
  windows. Not a hang, do not "fix" it. It also means a screenshot or FPS reading taken
  while backgrounded reflects the throttle, not real performance: foreground the window
  (or measure the headless `--no-ui` loop) before judging frame rate.

**Full guide:** [`docs/architecture/42-ui-frame-discipline.md`](docs/architecture/42-ui-frame-discipline.md).

## 8. Documentation Standards

Document with `///` (items) and `//!` (modules), for maintainers human and agent alike.
**Explain WHY — design intent, the constraint that forced this shape, the alternative that
failed — never restate what the code already says.** A comment that survives is one that
records something the next reader cannot recover from the code. Be concise; redundant
docs rot fastest.

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
