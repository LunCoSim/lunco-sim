# Architecture

The design narrative — how LunCoSim fits together, and *why*. Specs
([`../../specs/`](../../specs/)) are contracts; crate READMEs are how-to-use-it;
these are the reasoning.

> **A doc here describes what IS.** No changelogs, no "recently we fixed…". The
> short *why* notes are deliberate — they are what stops someone re-introducing a
> bug that was expensive to find. A doc whose only content is *how we got here*
> (a migration plan, a completed execution checklist, a closed audit) is deleted
> once the work lands; git remembers it.

Every doc opens with a status line:

```
> Status: Active | Design | Draft · Audience: <who should read this>
```

**Active** = as built (the default). **Design** = agreed shape, not fully built —
say what is missing (see [`14-simulation-layers.md`](14-simulation-layers.md) for
the banner pattern). **Draft** = under live review, may be wrong.

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
| [`25-experiments.md`](25-experiments.md) · [`27-target-resolution.md`](27-target-resolution.md) | Batch runs, sweeps, and how a run resolves its target |
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
| [`38-domains-as-packages.md`](38-domains-as-packages.md) | Composition; the domain-neutral-core thesis (absorbs the deleted 36/37 analyses) |
| [`39-usd-native-migration-plan.md`](39-usd-native-migration-plan.md) | The USD-native core migration |

## Subsystems (40–49)

| Doc | What it covers |
|---|---|
| [`40-asset-io.md`](40-asset-io.md) | Asset I/O policy; the wasm-safe I/O layer |
| [`41-axes-and-units.md`](41-axes-and-units.md) | **Convert once, at the importer.** `StageMetrics` / `ConventionTransform` — a Z-up/cm USD stage imports correctly |
| [`42-ui-frame-discipline.md`](42-ui-frame-discipline.md) | Frame discipline for UI |
| [`43-orbital-view.md`](43-orbital-view.md) | Satellites, ground stations, the site frame; the **IAU/WGCCRE rotation model** |
| [`44-surface-orbital-spaces.md`](44-surface-orbital-spaces.md) | The current surface/body-fixed and orbital/inertial reference-frame contract |
| [`45-big-space-correct-usage.md`](45-big-space-correct-usage.md) · [`46-bigspace-deep-analysis.md`](46-bigspace-deep-analysis.md) | Current `big_space` ownership, f64-to-cell projection, physics bridge, and maintenance checklist |
| [`48-object-builder.md`](48-object-builder.md) | The object builder |
| [`49-control-programs-and-live-rebuild.md`](49-control-programs-and-live-rebuild.md) | Generic control programs, OBC/FSW composition, and live USD rebuild boundaries |
| [`49-connectivity-link-kernel.md`](49-connectivity-link-kernel.md) | The generic link kernel (comms is a domain over it, not a kernel) |
| [`50-usd-driven-visuals.md`](50-usd-driven-visuals.md) | Beams, plumes, ribbons: geometry+look authored in USD, logic in Rust, bound by name (`info:id`). **`radius`/`height` bake at instantiation — live size is `xformOp:scale`**; a `lunco:*` property needs THREE files or it is inert |
| [`51-cinematic-camera.md`](51-cinematic-camera.md) | Authored camera paths (`UsdGeomBasisCurves` + a per-object driven clock). **`Ts` splines are SCALAR-ONLY** — no `double3` translate; hold via the clock tree, never `Playback.mode` |
| [`53-usd-suspension-specification.md`](53-usd-suspension-specification.md) | Wheels and suspensions in canonical PhysX names (`springStrength`/`springDamperRate`), the three `LunCo*API` extensions PhysX doesn't model, and detection **by applied schema, never by attribute presence**. A raycast wheel with no resolvable suspension refuses to spawn — no silent defaults |
| [`54-electrical-domain-and-modelica-libraries.md`](54-electrical-domain-and-modelica-libraries.md) | USD assembles / Modelica is the maths / rhai is behaviour, worked on EPS. A physical bus is **one acausal circuit** (`Pin` + `flow`, `connect()` → Kirchhoff for free), one `LunCoProgramAPI` under a domain scope. The shipped `LunCo` library loads demand-driven in the compiler; a twin's `<twin>/models` via a `TwinRoots` watcher — both rumoca built-ins |
| [`55-building-vessels-rovers-and-landers.md`](55-building-vessels-rovers-and-landers.md) | The step-by-step architectural guide for a mission-grade rover or lander — which layer owns which decision |
| [`55-scene-addressing-and-roots.md`](55-scene-addressing-and-roots.md) · [`56-asset-resolution-and-cache.md`](56-asset-resolution-and-cache.md) | **Identity is not location.** A scene is addressed by a root-relative source (`twin://`), a referenced asset by a logical identity (`@lunco://models/x.glb@`) — only the resolver knows paths. A bare relative path outside `assets/` is the failure both close |
| [`57-dem-georeferencing.md`](57-dem-georeferencing.md) · [`59-georeferenced-rasters-as-assets.md`](59-georeferenced-rasters-as-assets.md) | **The raster carries its own spatial reference.** Writing it out (a self-describing GeoTIFF, never a sidecar restating the transform) and reading it back in (an external GIS raster enters as an asset, not through an import subsystem) |
| [`58-vessel-envelope-and-routes.md`](58-vessel-envelope-and-routes.md) | Vehicle capability is **derived, not copied** — slip limit is `atan(μ)`, not a constant retyped into six files. HUD derivation and rhai accessors are built; routes and tiers are proposed |
| [`60-curvature-elevation-and-gravity.md`](60-curvature-elevation-and-gravity.md) | **PLANNED.** The measured curvature-feather defect (the edge feather descends ABSOLUTE relief, so a 1 km site renders as kilometre-tall spikes) and the plan for radial gravity on curved ground |
| [`61-scene-lifecycle-and-teardown.md`](61-scene-lifecycle-and-teardown.md) | **A scene owns more than its entities.** Entities die by structural tag (`CelestialDerived`); resources, caches and handles die in the `SceneTeardown` schedule — a schedule rather than a registry so the reset lives beside the code that writes the state. Remove vs restore, and why gravity is the restore case |

## Cross-cutting

| Doc | What it covers |
|---|---|
| [**`engineering-backlog-and-standards.md`**](engineering-backlog-and-standards.md) | The engineering backlog: adopted standards (ANISE, FMI 3.0, ROS 2, AOUSD conformance), architecture debt, testing debt, the measure-first list, the watch list, and **validated non-adoptions** — recorded so they don't get re-litigated. The deliberate exception to "describes what IS", and un-numbered because it spans every range rather than sitting in one |
| [`runtime-authored-ui.md`](runtime-authored-ui.md) | The small native HTML/CSS-like surface layer for Twin-facing HUDs, including its generic exposure boundary and reload/performance contract |
| [**`render-decoupling.md`**](render-decoupling.md) | **The material is the boundary.** Domain crates state appearance *intent* (`PbrLook`, `ShaderLook`, `SceneCamera`, `WorldLabel`); only `lunco-render-bevy` names `bevy_pbr`. This is why `--no-ui` links no wgpu/`bevy_render`/egui/winit — and why the `cargo tree` CI guard exists |
| [**`shader-layers-and-params.md`**](shader-layers-and-params.md) | Shader looks: WGSL-reflected `dyn_params` and named texture layers. Parameter names, ranges and defaults come from the shader source — **adding a parameter is editing a shader, not editing Rust** |
| [`command-journal.md`](command-journal.md) | One op log for identity, undo and sync. **Document-domain ops are journaled; command/session replay is not built** |
| [`terrain-substrate.md`](terrain-substrate.md) · [`terrain-layered-rendering.md`](terrain-layered-rendering.md) | The height oracle (one `HeightSource` from orbit to rover) and the layered Data→Transfer→Blend rendering pipeline |
| [`terrain-precompute-plan.md`](terrain-precompute-plan.md) | **Design.** Precomputed tiles + monotone progressive refinement — the target streaming architecture, replacing finding #6 of the audit. Its measurement steps are live tests in `lunco-terrain-surface/tests/precompute_*.rs` |
| [`terrain-lod-audit.md`](terrain-lod-audit.md) | The CDLOD streamer measured against the real moonbase DEM (surface only; the globe is out of scope). Kept because measurement **falsified** the intuitive story — the wrong version will be re-derived by the next reader |
| [`telemetry-subsystem.md`](telemetry-subsystem.md) | Channels, rates and clock binding. **Phases 0–1 landed; 2–5 are proposal** |
| [**`lint-substrate.md`**](lint-substrate.md) | Authoring mistakes that have no runtime symptom. **Facts in Rust, rules in rhai policy** (`lint.<domain>`), one linter per domain, findings in one report. Nothing lints on load — `RunLint` is a verb, and a scenario calling it on a cadence is the realtime linter |
| [`derive-substrate.md`](derive-substrate.md) | The unified derived-artifact substrate (async compute/bake patterns) |
| [`caching-and-precompute-strategy.md`](caching-and-precompute-strategy.md) · [`scenario-program-cache.md`](scenario-program-cache.md) | Caching strategy; the rhai program cache |
| [`efficiency-and-maintainability.md`](efficiency-and-maintainability.md) | **The North Star + substrates B–E in full**: the one principle, the tier ladder, `lunco-precompute` (B), `Mobility` (C), ports resolve→handle (D), `lunco-hash` (E) |
| [`usd-source-of-truth.md`](usd-source-of-truth.md) | **USD is the truth; ECS is a projection of it.** The rule every edit path obeys |
| [`rhai-integration.md`](rhai-integration.md) | Why rhai, and the as-built scripting surface. The *how-to* is [`../scripting-guide.md`](../scripting-guide.md) |
| [`command-sequences.md`](command-sequences.md) | Command sequences and the visual sequence editor |
| [`waypoints-in-usd.md`](waypoints-in-usd.md) | Routes and waypoints as authored USD, not runtime-only state |
| [`tutorial-autopilot-and-port-contracts.md`](tutorial-autopilot-and-port-contracts.md) | Same control path for human/autopilot tutorial tests; declared cosim topology versus live samples |

## Open issues & posture

- [`../reviews/open-rbac-not-enforced.md`](../reviews/open-rbac-not-enforced.md) — network command/session authorization is enforced; the loopback API remains a trusted local-authoring boundary and must not be exposed publicly
- [`../reviews/`](../reviews/) — standing issues (`open-*.md`) and dated audit reports

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
| `research/` | Prior art, inspiration, roads not taken — nothing here describes running code |

A number is an ordering hint, not an identifier: docs are linked by filename, so
gaps (`32`, `47`, `52`) and the two `55-*` docs are harmless and are **not**
renumbered — renumbering would break every inbound link for no reader benefit.
