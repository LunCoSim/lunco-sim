# Architecture

The design narrative — how LunCoSim fits together, and *why*. Specs
([`../../specs/`](../../specs/)) are contracts; crate READMEs are how-to-use-it;
these are the reasoning.

> **A doc here describes what IS.** No changelogs, no "recently we fixed…". The
> short *why* notes are deliberate — they are what stops someone re-introducing a
> bug that was expensive to find. Where a doc describes something not yet built,
> it says so in a banner (see [`14-simulation-layers.md`](14-simulation-layers.md)
> for the pattern).

## Start here

1. [`00-overview.md`](00-overview.md) — what LunCoSim is, the three-tier model, crate layers
2. [`01-ontology.md`](01-ontology.md) — the vocabulary: Space System, Port, Connection, Command
3. [`10-document-system.md`](10-document-system.md) — the foundational data model: Documents, Ops, Views
4. [`12-api.md`](12-api.md) — the one command funnel every UI, script, agent and test goes through

## Foundation (00–09)

| Doc | What it covers |
|---|---|
| [`00-overview.md`](00-overview.md) | System overview, three-tier model, crate layering |
| [`01-ontology.md`](01-ontology.md) | Terminology: Space System, Port, Connection, Attribute, `ControlStream` |

## Framework (10–19)

| Doc | What it covers |
|---|---|
| [`10-document-system.md`](10-document-system.md) | Documents, `DocumentOp`s, `DocumentView`s |
| [`11-workbench.md`](11-workbench.md) | UI/UX: workspaces, panels, command palette |
| [`12-api.md`](12-api.md) | Transport-agnostic typed commands + queries. **Three HTTP routes**: `POST /api/commands`, `GET /api/commands/schema`, `GET /api/health` |
| [`13-twin-and-workflow.md`](13-twin-and-workflow.md) | What a Twin is; save / load / workflow |
| [`14-simulation-layers.md`](14-simulation-layers.md) | Twin → Scenario → Run; `Backend`/`Participant` traits |
| [`15-adaptive-fidelity.md`](15-adaptive-fidelity.md) | Multi-clock and level-of-detail |
| [`16-document-identity-and-collaboration.md`](16-document-identity-and-collaboration.md) | Documents vs assets; identity = the path; op addressing decides merge; the layer/resolver/live-layer target |
| [`17-view-and-intent.md`](17-view-and-intent.md) | Cameras and the 5-layer control model |
| [`18-unified-journal-and-history.md`](18-unified-journal-and-history.md) | The edit journal and Twin history |
| [`19-unified-time-and-clock.md`](19-unified-time-and-clock.md) | One clock. The fixed-step grid, warp regimes, USD animation |

## Domains (20–29)

| Doc | What it covers |
|---|---|
| [`20-domain-modelica.md`](20-domain-modelica.md) | Modelica / rumoca; the `output` convention |
| [`21-domain-usd.md`](21-domain-usd.md) | USD as the authored scene; op-driven projection |
| [`22-domain-cosim.md`](22-domain-cosim.md) | The FMI-CS master loop, the **macro-step contract**, control-plane vs data-plane |
| [`23-domain-environment.md`](23-domain-environment.md) | Gravity, lighting, the sun feed |
| [`24-domain-sysml.md`](24-domain-sysml.md) | SysML |
| [`25-experiments.md`](25-experiments.md) · [`26-parallel-experiments.md`](26-parallel-experiments.md) · [`27-target-resolution.md`](27-target-resolution.md) | Batch runs, sweeps, and how a run resolves its target |
| [`28-modelica-realtime-physics.md`](28-modelica-realtime-physics.md) | The **realtime-safe** promise: which programs may drive predicted physics |
| [`29-rumoca-workarounds.md`](29-rumoca-workarounds.md) | Confirmed rumoca bugs we work around, the probe that retires each one, and the chokepoint that must not be bypassed |

## Infrastructure (30–39)

| Doc | What it covers |
|---|---|
| [`30-wasm-web-worker.md`](30-wasm-web-worker.md) | Off-thread Modelica in the browser |
| [`31-networking-and-state-sync.md`](31-networking-and-state-sync.md) | The replication planes, the wire, AOI, prediction & reconciliation |
| [`33-spacecraft-modeling.md`](33-spacecraft-modeling.md) | The lander slice |
| [`34-scenario-and-multidomain.md`](34-scenario-and-multidomain.md) | Scenarios, multi-domain vehicles |
| [`35-animate-perspective.md`](35-animate-perspective.md) | Timeline / sequence editor |
| [`36-components-and-sky.md`](36-components-and-sky.md) | Reusable components; sky visualization |
| [`37-model-synthesis-and-multidomain-composition.md`](37-model-synthesis-and-multidomain-composition.md) · [`38-domains-as-packages.md`](38-domains-as-packages.md) | Composition; domain-neutral core |
| [`39-usd-native-migration-plan.md`](39-usd-native-migration-plan.md) | The USD-native core migration |

## Subsystems (40–49)

| Doc | What it covers |
|---|---|
| [`40-asset-io.md`](40-asset-io.md) | Asset I/O policy; the wasm-safe I/O layer |
| [`41-axes-and-units.md`](41-axes-and-units.md) | **Convert once, at the importer.** `StageMetrics` / `ConventionTransform` — a Z-up/cm USD stage imports correctly |
| [`42-ui-frame-discipline.md`](42-ui-frame-discipline.md) | Frame discipline for UI |
| [`43-orbital-view.md`](43-orbital-view.md) | Satellites, ground stations, the site frame; the **IAU/WGCCRE rotation model** |
| [`44-surface-orbital-spaces.md`](44-surface-orbital-spaces.md) | The surface/celestial space split |
| [`45-big-space-correct-usage.md`](45-big-space-correct-usage.md) · [`46-bigspace-deep-analysis.md`](46-bigspace-deep-analysis.md) · [`47-bigspace-option-b-execution.md`](47-bigspace-option-b-execution.md) | `big_space` contract, the jitter root cause, and the physics/render split. **`cell_edge_length` and `switching_threshold` are PRECISION knobs, not extent knobs** |
| [`48-object-builder.md`](48-object-builder.md) | The object builder |
| [`49-connectivity-link-kernel.md`](49-connectivity-link-kernel.md) | The generic link kernel (comms is a domain over it, not a kernel) |
| [`50-usd-driven-visuals.md`](50-usd-driven-visuals.md) | Beams, plumes, ribbons: geometry+look authored in USD, logic in Rust, bound by name (`info:id`). **`radius`/`height` bake at instantiation — live size is `xformOp:scale`**; a `lunco:*` property needs THREE files or it is inert |
| [`51-cinematic-camera.md`](51-cinematic-camera.md) | Authored camera paths (`UsdGeomBasisCurves` + a per-object driven clock). **`Ts` splines are SCALAR-ONLY** — no `double3` translate; hold via the clock tree, never `Playback.mode` |
| [`52-connectivity-gaps-and-test-plan.md`](52-connectivity-gaps-and-test-plan.md) | Companion to 49: the connectivity gap audit and what closed it — radio shadow needs an opt-in `LinkOccluder` (occlusion is NOT the physics collider), and link ids are GIDs |
| [`53-usd-suspension-specification.md`](53-usd-suspension-specification.md) | Wheels and suspensions in canonical PhysX names (`springStrength`/`springDamperRate`), the three `LunCo*API` extensions PhysX doesn't model, and detection **by applied schema, never by attribute presence**. A raycast wheel with no resolvable suspension refuses to spawn — no silent defaults |
| [`54-electrical-domain-and-modelica-libraries.md`](54-electrical-domain-and-modelica-libraries.md) | USD assembles / Modelica is the maths / rhai is behaviour, worked on EPS. A physical bus is **one acausal circuit** (`Pin` + `flow`, `connect()` → Kirchhoff for free), one `LunCoProgram` under a domain scope. The shipped `LunCo` library loads demand-driven in the compiler; a twin's `<twin>/models` via a `TwinRoots` watcher — both rumoca built-ins |
| [`55-scene-addressing-and-roots.md`](55-scene-addressing-and-roots.md) · [`56-asset-resolution-and-cache.md`](56-asset-resolution-and-cache.md) | **Identity is not location.** A scene is addressed by a root-relative source (`twin://`), a referenced asset by a logical identity (`@lunco://models/x.glb@`) — only the resolver knows paths. A bare relative path outside `assets/` is the failure both close |
| [`57-dem-georeferencing.md`](57-dem-georeferencing.md) · [`59-georeferenced-rasters-as-assets.md`](59-georeferenced-rasters-as-assets.md) | **The raster carries its own spatial reference.** Writing it out (a self-describing GeoTIFF, never a sidecar restating the transform) and reading it back in (an external GIS raster enters as an asset, not through an import subsystem) |
| [`58-vessel-envelope-and-routes.md`](58-vessel-envelope-and-routes.md) | Vehicle capability is **derived, not copied** — slip limit is `atan(μ)`, not a constant retyped into six files. HUD derivation and rhai accessors are built; routes and tiers are proposed |
| [`60-curvature-elevation-and-gravity.md`](60-curvature-elevation-and-gravity.md) | **PLANNED.** The measured curvature-feather defect (the edge feather descends ABSOLUTE relief, so a 1 km site renders as kilometre-tall spikes) and the plan for radial gravity on curved ground |
| [`61-scene-lifecycle-and-teardown.md`](61-scene-lifecycle-and-teardown.md) | **A scene owns more than its entities.** Entities die by structural tag (`CelestialDerived`); resources, caches and handles die in the `SceneTeardown` schedule — a schedule rather than a registry so the reset lives beside the code that writes the state. Remove vs restore, and why gravity is the restore case |

## Cross-cutting

| Doc | What it covers |
|---|---|
| [**`engineering-backlog-and-standards.md`**](engineering-backlog-and-standards.md) | The engineering backlog: adopted standards (ANISE, FMI 3.0, ROS 2, AOUSD conformance), architecture debt, testing debt, the measure-first list, the watch list, and **validated non-adoptions** — recorded so they don't get re-litigated. The deliberate exception to "describes what IS", and un-numbered because it spans every range rather than sitting in one |
| [**`render-decoupling.md`**](render-decoupling.md) | **The material is the boundary.** Domain crates state appearance *intent* (`PbrLook`, `ShaderLook`, `SceneCamera`, `WorldLabel`); only `lunco-render-bevy` names `bevy_pbr`. This is why `--no-ui` links no wgpu/`bevy_render`/egui/winit — and why the `cargo tree` CI guard exists |
| [**`shader-layers-and-params.md`**](shader-layers-and-params.md) | Shader looks: WGSL-reflected `dyn_params` and named texture layers. Parameter names, ranges and defaults come from the shader source — **adding a parameter is editing a shader, not editing Rust** |
| [`command-journal.md`](command-journal.md) | One op log for identity, undo and sync. **Document-domain ops are journaled; command/session replay is not built** |
| [`terrain-substrate.md`](terrain-substrate.md) · [`terrain-layered-rendering.md`](terrain-layered-rendering.md) | The height oracle (one `HeightSource` from orbit to rover) and the layered Data→Transfer→Blend rendering pipeline |
| [`terrain-lod-audit.md`](terrain-lod-audit.md) · [`terrain-precompute-plan.md`](terrain-precompute-plan.md) | The CDLOD streamer audited against the real moonbase DEM (surface only; the globe is out of scope), and the precomputed-tile architecture that replaces its finding #6 |
| [`telemetry-subsystem.md`](telemetry-subsystem.md) | Channels, rates and clock binding. **Phases 0–1 landed; 2–5 are proposal** |
| [`ports-system-design.md`](ports-system-design.md) | `PortRegistry` — the one scalar-port surface (Substrate D) |
| [**`lint-substrate.md`**](lint-substrate.md) | Authoring mistakes that have no runtime symptom. **Facts in Rust, rules in rhai policy** (`lint.<domain>`), one linter per domain, findings in one report. Nothing lints on load — `RunLint` is a verb, and a scenario calling it on a cadence is the realtime linter |
| [`derive-substrate.md`](derive-substrate.md) · [`precompute-substrate.md`](precompute-substrate.md) · [`hashing-substrate.md`](hashing-substrate.md) · [`mobility-substrate.md`](mobility-substrate.md) | The derived-artifact substrates (A–E) |
| [`caching-and-precompute-strategy.md`](caching-and-precompute-strategy.md) · [`scenario-program-cache.md`](scenario-program-cache.md) | Caching strategy; the rhai program cache |
| [`efficiency-and-maintainability.md`](efficiency-and-maintainability.md) | The North Star |
| [`bevy-0.19-migration.md`](bevy-0.19-migration.md) | Bevy 0.18 → 0.19 migration analysis |

## Reviews & posture

- [`../reviews/2026-07-13-remediation-report.md`](../reviews/2026-07-13-remediation-report.md) — the closing report; the best single summary of the current shape of the system
- [`../reviews/2026-07-12-full-code-review.md`](../reviews/2026-07-12-full-code-review.md) — the review that drove it
- [`../reviews/TODO-rbac-not-enforced.md`](../reviews/TODO-rbac-not-enforced.md) — **the project does not enforce access control.** Trusted LAN only; never expose a host to an untrusted network

## Research

[`research/`](research/) — historical analysis, inspiration, rejected paths. Nothing
there describes running code.

## Numbering

| Range | Category |
|---|---|
| `00`–`09` | Foundation |
| `10`–`19` | Framework |
| `20`–`29` | Per-domain design |
| `30`–`39` | Infrastructure & deployment |
| `40`–`49` | Low-level subsystems |
| `50`–`59` | Authoring contracts — what a scene, an asset or a vessel may state, and how it resolves |
| `60`+ | Physical fidelity — planned work on the world model itself |
| un-numbered | Cross-cutting substrates and boundaries |
| `research/` | Historical / inspiration |
