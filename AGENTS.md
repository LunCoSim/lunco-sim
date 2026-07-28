# LunCoSim AI Agent Guidelines

This document provides specific instructions and context for AI agents (Claude, Gemini, Antigravity, etc.) working on the LunCoSim codebase. Adherence to these guidelines is mandatory for maintaining simulation integrity and modularity.

## Repository Navigation

Start here, in order (new to the codebase? the canonical narrative path is **[docs/README.md → Reading order for newcomers](docs/README.md#reading-order-for-newcomers)**; the list below is the agent-oriented quick map):

0. **[skills/README.md](skills/README.md)** — task-oriented runbooks. If a skill covers the task, read it *first*; it encodes the traps this file only gestures at.
1. **[docs/crates-index.md](docs/crates-index.md)** — the map of the ~50-crate workspace and each crate's responsibility. **First stop for "which crate does X".**
2. **[docs/principles.md](docs/principles.md)** — the non-negotiable design principles. Verify every plan against these.
3. **[docs/architecture/README.md](docs/architecture/README.md)** — the indexed design narrative. Numbers are an ordering hint, not an identity: **00s** overview/ontology, **10s** framework (document, workbench, API, twin, time), **20s** domains (modelica, usd, cosim, environment, sysml, experiments), **30s** platform (wasm/web, networking, spacecraft), **40s** cross-cutting (asset-io, axes-units, frame discipline), **50s–60s** feature subsystems, un-numbered substrates. Start at `00-overview.md`.
4. **[specs/README.md](specs/README.md)** — feature-spec status index (Implemented / Partial / Not-built / Superseded).
5. **[docs/reviews/](docs/reviews/)** — what is known-broken right now (`open-*.md`).
6. **This file (AGENTS.md)** — the rules below.

**Every doc carries a status line.** `Status: Active` describes the system as built;
`Status: Design` is an agreed shape that is *not* fully built. Never implement against a
Design doc without checking what actually exists first.

## Agent Mandates
- **Crate Maintenance**: Whenever a new crate is added to the workspace, the agent MUST update `docs/crates-index.md` to include the new crate in the appropriate category with a concise responsibility summary.
- **Doc accuracy**: when you rename/remove a crate, type, or binary, grep the docs (`*.md`) for the old name and fix references in the same change — don't leave dangling docs for a later audit.
- **Generated artifacts are generated.** `crates/lunco-usd/schema/generatedSchema.usda` and
  the `Types` block of `schema/plugInfo.json` both come from `scripts/gen_schema.py`. Edit
  `schema.usda` and re-run it; never hand-edit the outputs. `plugInfo.json` *was*
  hand-maintained and drifted — three API schemas were declared but unregistered, so no
  external USD runtime could resolve them, which is the entire reason they are codeless
  schemas rather than loose `customData`. A schema that isn't in `plugInfo.json` does not
  exist outside this engine.
- **Verify before you assert.** Read the source before reporting a finding — never relay a
  claim from a subagent report, a doc comment, or a summary as fact. In one session a
  survey reported a "bug" in a visibility walk that was provably correct, and a "missing
  schema" that was a word inside a code comment. Both were caught only by opening the
  file. A doc comment is evidence of intent, not of behaviour.
- **Capture real exit codes.** `cmd > log 2>&1; echo "EXIT=$?"` reports the *echo's* status
  to any watcher; a background task that "succeeded" can contain a failing test. Grep the
  log for the verdict, and when a test fails, confirm whether it fails on a clean tree
  before attributing it to your change.
- **Subagent batches**: give each agent a disjoint file lot, and tell it explicitly **not**
  to run `cargo build`/`check`/`test` — parallel builds thrash the machine and mask each
  other. The coordinator runs one workspace check after all agents land. See
  `skills/subagent-batches`. Repo skills live in `skills/`, never `.claude/`.
- **Behaviour-changing refactors need a baseline.** Capture the parity verdicts *before*
  editing (`cargo run -p lunco-sandbox --bin sandbox -- test --scene scenes/sandbox/…`,
  exit 0=PASS/1=FAIL/2=no verdict) and re-run after. "It compiles" is not evidence that a
  drivetrain still behaves the same.
- **Rules go in policy, not in Rust.** The lint layer is FACTS in Rust (the crate that
  owns the subject) and RULES in `assets/scripting/policy/lint_<domain>.rhai`, reached via
  the `lint.<domain>` hook — one linter per domain (`usd`, `rhai`, `modelica`). Add a rule
  by editing the script; add Rust only when no existing fact can answer the question
  (`facts.prims[].schemas` answers most). Nothing lints on load: `RunLint` is a verb
  (`cmd("RunLint", #{})` / `{"command":"RunLint"}`), `query("LintReport")` reads it back,
  and `sandbox --validate` runs the same rules over a file. Design:
  [`docs/architecture/lint-substrate.md`](docs/architecture/lint-substrate.md).
- **Hierarchy is namespace; a joint is attachment.** A mounted part that applies
  `PhysicsRigidBodyAPI` and is named by no joint is a SEPARATE free body and falls out of
  the vehicle — silently, with every parity gate green (four motors per rover did exactly
  that). Internal part = mass + geometry, no body; movable part = body **and** joint,
  authored together. Guarded by `nested-body-no-joint`
  (`crates/lunco-scene-commands/tests/shipped_assets_lint_clean.rs`) and, behaviourally,
  by `scenes/tests/parts_attached.usda`.
- **A test scene lives in `assets/scenes/tests/`, its scenario in
  `assets/scenarios/tests/`** — the DIRECTORY is what makes it a test, not an `_test`
  suffix. `scripts/run_scene_tests.sh` runs that directory, the Scene menu hides it
  (one checkbox in Settings), and two checks in
  `crates/lunco-scene-commands/tests/test_scenes_are_tests.rs` catch both failure
  directions: a test scene that asserts nothing, and a rig named like a test that is
  sitting outside the directory where anything would run it.
- **A green gate must be able to go red.** A linter with no broken fixture, or a scene
  test whose subject never moves, reports "clean" and "not running" identically. Ship the
  negative case with the check — `lint_selftest.usda` exists for exactly this, and a
  vessel that never simulates is excluded from `parts_attached.usda` rather than counted
  as a pass.

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

The lathe is the worked example, and it cuts both ways. `lunco:lathe:profile` /
`throatRadius` / `contour` are legitimate: USD has no surface-of-revolution schema — the
parametric gprims are Sphere/Cube/Cylinder/Cone/Capsule/Plane, and `UsdGeomNurbsPatch` is a
*result* format (points, weights, knots), not a generator. But sampling density and
polynomial degree are properties of the patch, so they are read from the standard
`NurbsPatch` fields `vVertexCount` and `vOrder`. Only the *shape* was new. Two spellings of
one quantity is the same defect as rule 3, arrived at from the other side.

Also prefer a widely-adopted external standard to a bespoke blob: the mission ephemeris is
CCSDS OEM / SPICE SPK, not a hand-rolled JSON schema.

Watch for a *rename* of a standard, which is the same mistake wearing a namespace. A
program prim names its source the way `UsdShadeShader` does — `info:implementationSource` /
`info:id` / `info:sourceAsset` / `info:sourceCode` — because a `lunco:program:` set spelled
token-for-token the same is a second name for one thing. If your new attribute set reads
like a standard one with a prefix swapped, use the standard one.

**1a. A `lunco:` schema is for an EXPOSED ENGINE CAPABILITY — nothing else.** That is the
test, and it is narrow. Intents, program dispatch, terrain layers, control bindings are
engine capabilities: the engine defines the vocabulary, so USD is where it gets declared
and validated. A number that merely *describes a part* is not a capability, and a
behaviour with state is not one either. Three ways this goes wrong:

- **Reinventing a standard** — see above.
- **Modelling physics the standard already models.** Solver-facing actuation is
  `UsdPhysicsDriveAPI` on a joint (`type`/`targetPosition`/`targetVelocity`/`stiffness`/
  `damping`/`maxForce`) — that is the whole standard vocabulary for "something drives
  this". There is no AOUSD vehicle schema, so `physxVehicle*` names are adopted for
  interop (**names, not runtime semantics**); invent `lunco:` only where neither exists.
- **Attributes with no reader.** `lunco:obc:powerDraw` was authored on two assets and read
  by nothing but a doc comment justifying its own existence. A schema property nothing
  consumes is dead weight that reads as architecture.

**1b. USD holds nameplate, models hold equations and state.** A part's authored numbers —
mass, `stallTorque`, gear `ratio`, efficiency — are scene data: swapping a motor is
swapping one reference arc, and the Inspector gets a slider free from `customData`. The
*equations* and anything with state — thermal derating, battery sag, current limits — are
Modelica or rhai, and **the program overrides the scalars** the way a wired port beats a
constant (`assets/models/RoverMotorThermal.mo` is the exemplar). Same line UsdPhysics
draws: mass is authored, `F=ma` is the solver's.

Corollaries. **Do not derive a physics quantity from geometry** — a wheel's rolling radius
is authored (`physxVehicleWheel:radius`) because under load it legitimately differs from
the mesh; deriving it silently couples two things allowed to disagree. And **a flow is a
network, not a per-part scalar**: power and heat are `outputs:` ports feeding one Modelica
circuit (`motor.usda` publishes `outputs:heat`), because "a circuit is one Modelica model,
not one per part."

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

A physical part reads PHYSICS. A landing leg carries load because the ground pushes on
it, so the strut's glow takes the `force` port off the leg's own prismatic joint — the
number the solver just computed. Gating that behind an authored sensor would mean a
strut that only reports load if someone remembered to install a switch. Flight software
reads SENSORS, because a computer only knows what its instruments tell it:
`DescentGuidance` reads the altimeter, with its mount point, its `rangeMax` and its
out-of-range behaviour, not the true height.

Getting this backwards costs real bugs. An altimeter's datum sits 3.3 m above the pads, so
gating a strut on it forces a hand-copied constant to restate the geometry, and the legs
fire before touchdown. **When a constant exists only to translate between two prims'
positions, the wire is wrong.**

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

**A visual is a CONSEQUENCE of physics — wire it, never script it.** A strut reddens
because it is carrying load, on the same tick and by the number the solver computed.
Shader parameters are ordinary port sinks: `float inputs:load_frac.connect =
</Lander/LegPX_Spring.outputs:force>` on the bound gprim, and the value lands on the
WGSL uniform through the same graph a thruster force uses — no new resolver, no
per-frame script. Normalise on the WIRE, with the SSP affine `lunco:factor:<port>` /
`lunco:offset:<port>` the sink already carries — not in the shader, never in rhai, and
not in a `.mo` written to hold a single rating. **Publish the physical RESULT, not the
driving term** — a strut's load is the spring's own reaction,
`stiffness * (targetPosition - displacement) + damping * (targetVelocity - velocity)`,
which is zero until compression starts, not a proximity-gated force pressed onto it, which
reads fully loaded while the leg is still in the air. That reaction is positive in
compression, so the joint's axis — and only the joint's axis — carries the sign; a
`lunco:factor:` is a unit conversion and never a sign fixup. When a visualization happens too early, the
model is publishing an input. See
[`visualize-physics-with-shaders`](skills/visualize-physics-with-shaders/SKILL.md).

**A port backend must claim only names it KNOWS it owns — never guess and never widen
to compensate.** Registry precedence is registration order and plugin add-order is not
a contract, so a backend that accepts a name provisionally will silently swallow
another layer's writes and return `true`, leaving propagation nothing to report. If a
backend cannot answer from what it has, give it an authoritative set from the layer
that can: the shader backend claims a parameter only when the USD authoring pass — which
resolved the bound shader and knows its declared inputs — recorded it in
`ShaderLook::driven`. A guess plus a precedence workaround is two mechanisms where one
fact belongs.

**3. No legacy, shims, or fallbacks.** Replace a mechanism and delete the old one in the
*same* change. Two spellings of one fact means two writers, and which wins becomes a
function of load order — that is exactly how a scene that rendered correctly went dark
when someone gave it a sky. If a migration truly cannot land at once, say so and leave a
`TODO` naming the trigger; never a silent half-state. **A write with no reader is worse
than no write** — it makes the journal claim a setting persisted when it did not.

**Deleting the old mechanism is not done until its traces are gone**, and the traces
outlive the code:

- **The abstraction it needed.** A trait with one implementor is a shim for the implementor
  that left. When the flattened `sdf::Data` reader was retired, 106 functions stayed
  generic over `UsdRead` for a second source that no longer existed.
- **One name, one definition.** `rel_target` existed as *both* an inherent method
  (`-> Option<SdfPath>`) and a trait method (`-> Option<String>`). Rust inherent methods
  **shadow** trait methods, so removing the generics silently re-pointed every call to the
  other one. It happened to break the build; it could as easily have compiled and behaved
  differently.
- **The prose.** Comments asserting the retired design are worse than stale — they are
  read as current and copied. Grep the old mechanism's *name* and fix every doc hit in the
  same change, including the ones that only mention it in passing.
- **Duplicate implementations of one concept.** "The bound shader" had four
  implementations; three did a raw `material:binding` read and silently dropped inherited
  bindings. Duplication does not announce itself — it drifts, and the copies that are
  wrong keep passing their own tests.

**4a. Two read planes; never conflate them.** Composed reads (`UsdRead` on `StageView`
over the canonical stage) resolve references, variants and inherits — that is what the
domain extractors and anything solver-facing must use. Authored-layer reads (`UsdDataExt`
over `sdf::Data`) are deliberately *pre*-composition and exist for the document/authoring
plane: "which layer holds this opinion" is a question only they can answer, and document
tests assert exactly that. Using the authored plane where composition is meant hides
inherited opinions; using the composed plane in a document test destroys what it tests.

**4b. Use openusd's computed APIs, not a hand-rolled walk.** `ComputeBoundMaterial`
resolves inherited bindings, collection bindings and binding strength; a raw
`rel_target(prim, "material:binding")` sees only an opinion authored on that exact prim —
and inherited bindings are the common case, not the corner case. Likewise
`ComputeVisibility`/`ComputePurpose` (purpose resolves to the *nearest ancestor that
authors an opinion*, so a child can opt back out of a `guide` group — an "is any ancestor
guide?" walk gets that wrong), and `read_preview_surface` over hand-walking `inputs:`.
These are correctness, not style.

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

The same rule governs `docs/`. Conventions — where a doc belongs, the status header, and
the lifecycle — are in [docs/README.md § Conventions](docs/README.md#conventions). Two
that bite most often:

- **A doc describes what IS.** No changelogs, no "recently we fixed…". A doc whose only
  content is *how we got here* — a migration plan, a completed execution checklist, a
  closed audit — is **deleted** once the work lands. Git remembers it; a stale plan reads
  as an open commitment.
- **A pointer to a doc must resolve.** If you move or delete a doc, grep for its name
  across `docs/`, `skills/`, `crates/` and `assets/` and fix every reference — including
  the ones in `//!` comments. A dangling pointer is worse than no pointer.

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

## 10. Diagnosing a mechanism that misbehaves

A mechanism failure names itself badly. "The spring loads the wrong way", "the
joint's sign is inverted", "the limits must be backwards" — these describe a
*reading*, not a cause, and acting on the reading is how a compensating sign flip
gets committed. Two readings that look identical from a port:

- a DOF that is **jammed** (nothing reaches it), and
- a DOF that is **reversed** (the load reaches it with the wrong sign).

Separate them before touching any authored number.

**Isolate by single variable, and let the measurement kill the hypothesis.** One
change per run, against a rig that already works. Hypotheses that feel compelling
and are cheap to refute — change a mass 20× for solver conditioning, halve μ for
friction, widen one limit for a boundary. A behaviour invariant to mass and
friction is *geometric*, and geometry means contact or frame.

**Verify the load path, not just the state.** A vehicle at a believable height,
level and at rest, proves nothing about *how* it is held up. Check that the
element you designed to carry the load is carrying it: sum what the springs report
and compare it against the weight. When they disagree, something rigid is in the
path.

**Trust a second, independent measurement over a better argument.** Derive the
same quantity two ways — a port and raw world positions, a constraint's residual
from each body separately — and compare. A port and a solver that share a bug
agree with each other perfectly. This is what turns "I think the axis is wrong"
into "the axis is right and the strut never moved".

**Correct the record.** A wrong diagnosis written into a doc or skill costs more
than the bug did, because the next reader spends their budget on your dead end.
When a documented cause turns out to be wrong, rewrite it — do not append.


<claude-mem-context>
# Memory Context

# [main] recent context, 2026-07-28 12:46pm GMT+7

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (14,111t read) | 641,738t work | 98% savings

### Jul 28, 2026
47881 12:23a 🔴 Rover jitter fixed via gear differential constraint unification
47882 12:25a ✅ Rover drive test initialized with gear constraint at throttle 0.6
47883 12:26a ✅ Jitter data collection in progress for gear-constrained rover
47884 " 🔴 Rover jitter quantified and fixed: gear constraint reduces height bobbing 57x
47885 " 🔵 Gear constraint introduces performance cost: 65% higher frame time
47886 12:27a ✅ Performance A/B test configured: gear constraint vs baseline
47887 12:28a ✅ Full mobility test suite launched for regression validation
47888 12:29a 🔵 Performance A/B test completed: gear constraint costs 25% frame rate
47889 " ✅ Environment variable gate added for gear constraint A/B testing
47890 " ✅ Final A/B validation run: identical scene with gear constraint toggled
S7121 Debug and fix rover jitter during movement caused by competing coordinate systems. Implement holonomic gear constraint solver and characterize performance with self-timing instrumentation. (Jul 28, 12:31 AM)
47892 12:32a 🔵 Identical-scene A/B test shows gear constraint is performance WIN, not cost
47893 " ✅ Instrumentation added for constraint solver self-timing
47894 " ✅ Self-timing instrumentation completed for constraint solver performance profiling
S7127 Pull latest changes and debug rover jitter caused by two systems fighting over coordinates (Jul 28, 12:33 AM)
S7128 Status check: "what's left?" on rover suspension physics work. User asked for remaining tasks and blockers after shimmer/jitter fix was implemented. (Jul 28, 12:57 AM)
S7130 Status check and shimmer fix validation: restore jitter probe diagnostic, create paired measurement harness, and run before/after test of gear-joint suspension change (Jul 28, 5:58 AM)
47924 5:58a ✅ Jitter probe diagnostic re-added for suspension smoothness measurement
47925 5:59a ✅ Jitter probe plugin integrated into sandbox core initialization
47926 " 🔵 Paired before/after measurement harness for shimmer fix validation
S7132 Debug gear-joint suspension constraint implementation that fails catastrophically under drive load, compared to penalty-spring baseline (Jul 28, 5:59 AM)
47927 6:03a 🔵 Paired A/B measurement completed with suspicious data disparity
47928 " 🔵 AFTER measurement crashed after 7.8 seconds; gear-joint implementation unstable under drive load
47929 " 🔵 AFTER run silent failure: rover never accelerated, process stopped without panic or error
47930 " 🔵 Both BEFORE and AFTER runs failed test; AFTER crashed much faster
S7151 Fix starfield circle-in-sky rendering and terrain shadow issues in summer-space-school scene; improve performance and visuals; review recent commits (Jul 28, 6:04 AM)
47964 6:25a 🔵 Starfield rendering uses view-ray emissive dome, not surface shading
47965 " 🔵 Starfield parameters exposed via shader metadata annotations for live hot-reload
47966 " 🔵 Terrain vertices include morph targets for geomorphing LOD transitions
47967 " 🔵 Headless server mode built via Cargo feature flags, not separate binary
47968 " 🔵 NoFrustumCulling used for sky/starfield in big_space and trajectory rendering
47969 6:27a 🔴 Starfield dome culled when using shader materials—added double_sided attribute preservation
47970 " 🟣 Per-instance vertex shader support via info:wgsl:vertexAsset USD attribute
47976 6:41a 🔵 Unreachable public item warnings in lunco-celestial and lunco-sandbox crates
47977 " ✅ Restrict JitterProbePlugin visibility from pub to pub(crate)
47978 " 🔵 Port 3001 already in use on localhost
47979 6:42a 🔵 Existing sandbox process occupies port 3001
47980 " ✅ Sandbox process terminated via API exit command
S7153 Fix starfield circle-in-sky rendering and terrain shadow issues in summer-space-school scene; improve performance and visuals; review recent commits (Jul 28, 6:42 AM)
47983 6:45a ✅ Headless server build completed successfully
S7156 Fix starfield appearing as circle in sky; investigate and improve bad terrain shadows in summer-space-school; review latest changed commits for regressions (Jul 28, 6:46 AM)
47984 6:47a 🔵 Sandbox API does not support DiscoverSchema command
47985 6:48a 🔵 DiscoverSchema is implemented in code but not exposed by running sandbox API
47986 " 🔵 Sandbox API missing query and queries endpoints
47987 " 🔵 Sandbox API schema endpoint is accessible and responsive
47988 " 🔵 Sandbox API exposes 181 commands including scene loading and screenshots
47989 6:49a 🔵 Sandbox API command signatures for scene loading and imaging
47990 " ✅ Initiated loading of summer-space-school twin via API
47992 " 🔵 OpenTwin command accepted but twin not reflected in open documents
47994 " 🔵 Sandbox API lacks command result polling and status query mechanisms
47996 " 🔵 Summer-space-school twin loaded with terrain layers and rendering pipeline active
47997 6:50a ✅ Captured screenshot of summer-space-school scene rendering
47998 " 🔵 Clock hierarchy in luncosim time domain
47999 6:51a 🔵 SetClock command successfully isolates sky rendering at extreme timescale
48000 " 🔵 Sky lighting cycles correctly through day-night; starfield circle issue is rendering-layer problem
48002 6:55a 🔵 No Pipeline or Shader Validation Errors in Build Output
48003 " 🔴 Fixed Starfield Sky Dome Appearing as Circle (Backface Culling Issue)
48004 " 🔴 Fixed Terrain Shadow Quality in Summer School Scene
S7165 Investigate and fix multiple bugs across LunCo: starfield rendering, terrain shadows, Modelica parsing on Windows, co-sim wiring, schema units, render robustness, Earth direction, and time handling. (Jul 28, 6:56 AM)
S7166 Investigate and fix 6 Windows bug-report items (B-04/B-06, B-11/B-09, B-07/B-10) affecting Modelica MSL gating, Windows .mo parsing, co-sim antenna wiring, USD schema units, render robustness, and celestial/time handling. Four async agents launched to cover four distinct clusters. (Jul 28, 10:16 AM)
**Investigated**: **Four parallel diagnostic agents deployed:**
1. **B-04/B-06** (MSL gating + Windows strip): Located MSL background install trigger at `msl_remote.rs:758` (unconditional spawn with no networking-mode gating). Confirmed bound-input strip at `lib.rs:1134` calls `ast_extract::strip_input_defaults_with_report()` which parses FILE CONTENT not path. Rumoca grammar supports CRLF at scanner level (`modelica_parser.rs:131`). Path prefix `\\?\` is diagnostic only; actual parse happens on file bytes read via `std::fs::read_to_string()`.

2. **B-11/B-09** (YawJoint + schema): Found antenna.usda wires at lines 262 (`inputs:angle.connect = </outputs:az>`) and 274 (`inputs:angle.connect = </outputs:el>`). Rocker_bogie.usda shows correct pattern: declares `float outputs:drive_left = 0.0` on chassis chassis (line 116). B-09 schema lookup uses tuple keys `(schema, name)` where schema="UsdGeomCylinder" but warning references instance "Cylinder_1.radius"; likely instance-vs-class keying mismatch at `schema.rs:420`.

3. **P0 Shadow OOM**: Full read of `render_robustness.rs` (578 lines). Ladder correctly escalates: Healthy → ShadowMapsOff (disables all DirectionalLight/SpotLight/PointLight shadow_maps_enabled) → GaveUp (sets Camera.is_active=false). Lines 396-413 show escalation logic. Issue: frames still submit at ~600/s after camera deactivation because render graph continues (likely shadow passes, egui, or other systems not gated on Camera.is_active).

4. **B-07/B-10** (Earth + time): Sandbox scene HAS solar_system.usda reference at line 46 and site anchor (lat/lon/height/body at lines 27-30), so CELESTIAL opt-in is present. B-07 EarthDirectionWorld degenerate message not found in grep (likely in lunco-environment/src/earth.rs, not yet read). B-10 discontinuity detector not yet located; re-anchor message implies time jump logic exists but order/reset unclear.

**Learned**: - **MSL offline**: No networking mode consulted before spawn_native_install(). Code path: msl_remote.rs:758 → spawn_native_install() with no check of `MslSettings` or offline flag.
- **Bound-input parsing**: Failure is in file content parse not path; CRLF supported by Rumoca lexer but file must parse through `rumoca_phase_parse::parse_to_ast()` → `ast_extract::strip_input_defaults_with_report()`.
- **Render mitigation**: Ladder state machine is pure and works correctly, but presentation loop doesn't honor Camera.is_active=false; suggests render graph continues submitting other passes or presentation queue has independent logic.
- **YawJoint wires**: Authored correctly in antenna.usda. Port declaration mismatch likely in cosim runtime registry keying (class "PhysicsRevoluteJoint" vs instance "YawJoint").
- **Schema lookup**: CORE_LINEAR_UNITS entries keyed by `(schema_class_name, property_name)` tuples; lookup in `apply_core_linear_units()` at schema.rs:420 tries to match by stringified schema+name, but warning shows instance name "Cylinder_1.radius" suggesting instance-level lookup happening somewhere else.
- **Sandbox scene**: Celestial reference is present and correct; issue is likely wiring/publisher side not scene authoring.

**Completed**: None. All work is diagnostic; no code changes applied yet. Four agents (afc45bbf4bc11095e, aff002da57f60cf2b, aa03e745418fe6e70, a01b313984e5623d3) launched async and still running.

**Next Steps**: **Immediately upon agent completion:**
1. **B-04 (MSL gating)**: Add networking-mode gate before spawn_native_install() at msl_remote.rs:758. Check for `MslSettings` and abort if networking mode is None.
2. **B-06 (Windows .mo)**: Determine if strip failure is CRLF or BOM by inspecting first few bytes of failing .mo file; if BOM, strip in read_to_string() result; if CRLF, ensure rumoca parser handles line-ending normalization.
3. **B-11 (YawJoint)**: Fix cosim port registry keying — likely need to register "angle" port explicitly on PhysicsRevoluteJoint, or fix wire to use correct port name.
4. **B-09 (schema units)**: Change CORE_LINEAR_UNITS lookup from instance-name keying to class-name keying, or add UsdGeomCylinder/Capsule entries to vendored schema.
5. **P0 (render death)**: Upstream fix: clamp DirectionalLight shadow-casting count at startup via light_policy or limit shadow map size. Downstream: fix frame submission loop to stop when presentation stopped (investigate why Camera.is_active=false doesn't halt render graph).
6. **B-07 (Earth direction)**: Locate EarthDirectionWorld degenerate check in lunco-environment; verify wiring is correct and add NaN guard to port publisher.
7. **B-10 (epoch jump)**: Find re-anchor and discontinuity-detector code; reorder so discontinuity check runs AFTER re-anchor and state is reset properly.

All fixes are minimal and targeted. No starfield/terrain shadow work until P0 render chain is unblocked.


Access 642k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>
