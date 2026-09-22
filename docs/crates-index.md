# LunCoSim Crates Index

This document provides a comprehensive index of all crates in the LunCoSim workspace, categorized by their functional domain and architectural responsibility. It serves as a navigation guide for both developers and AI agents.

---

## 1. Workspace & Core Foundation
Low-level primitives, document/journal systems, time, and cross-cutting concerns (storage, assets, theming, settings).

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-core`** | Stable ECS engine facts: identity/provenance, shared markers, typed scene requests, runtime diagnostics/fault contracts, state markers, and small ECS utilities. Reconciliation, exposure storage, synchronization helpers, pacing, and domain composition have their own owners. |
| **`lunco-geometry-core`** | Precision-preserving, renderer-independent geometry kernels: finite f64 AABB/OBB relations and convex profile extrusion. Shared by authored geometry tools without depending on ECS or USD mesh evaluators. |
| **`lunco-core-runtime`** | Bevy runtime owner for core contracts: fixed simulation ticks, rollback/netcode schedule anchors, pacing/barriers, gate instrumentation, subsystem toggles, recoverable synchronization helpers, and the runtime plugin that installs those mechanisms. |
| **`lunco-exposure-core`** | Renderer-independent typed exposure registry (`EngineExposures`, `ExposureValue`, and refresh state). It has no application projection or UI policy; `lunco-luncosim-exposures` supplies the production projection. |
| **`lunco-command-contracts`** | Pure mutation, session, acknowledgement, rejection, and synchronization-channel contracts shared by document, transport, networking, and runtime adapters without ECS. `Ack.data` uses the shared typed `HookValue` ABI; API JSON is created only at the external adapter. |
| **`lunco-id`** | Platform-neutral 53-bit operation/entity ID generation shared by document and runtime identity boundaries. |
| **`lunco-viewport-core`** | Renderer-independent viewport contract: explicit active-camera binding, scene visibility and layout state, viewport scheduling boundary, and the shared camera-ray construction used by scene-click owners. |
| **`lunco-interaction-core`** | Renderer-independent cursor interaction contract: registered USD button policy, semantic single-owner possession arbitration, editor tool gates, drag state, and the affected-entity marker consumed by scene, avatar, and camera runtimes. |
| **`lunco-port-core`** | Shared co-simulation port substrate: `Port`, endpoint/control-surface components, `PortRegistry`, backend registration and resolution, topology invalidation state, metadata, collision reporting, and resolved fast-path handles. It is independent of the general engine core. |
| **`lunco-spatial`** | BigSpace spatial substrate: f64 coordinate/frame helpers, the persistent `WorldRoot`/`WorldGrid` shell, atomic grid migration, hierarchy invariants, spatial markers, and the vehicle-neutral navigation law. It depends on `lunco-core` for the shared runtime-diagnostic resource, but core does not depend on spatial. |
| **`lunco-core-session`** | Always-on session and authority substrate: network role/status, `SessionRegistry`, generic `ClaimControl`/`ReleaseControlClaim` transitions, `ControlAuthorityChanged`, RBAC policy, prediction markers/input watermarks, and session-dependent identity admission. |
| **`lunco-command-macro`** | Procedural macros for the typed command system (`#[Command]`, `#[on_command]`, `register_commands!`; re-exported by `lunco-core`). |
| **`lunco-workspace`** | Headless editor session management: open Twins, active documents, perspectives, recents, generic active-Twin setting persistence (`SetTwinSetting` / `ResetTwinSetting`), and Twin-entry command payloads such as `rename::RenameTwinEntry`. |
| **`lunco-workspace-api`** | API adapter for Workspace-owned queries (`ListOpenDocuments`, `ListRecentFiles`, `ListTwin`, `ReadActiveTwinContract`), installable by windowed, headless, or offscreen hosts without making the data-only Workspace crate depend on the API layer. |
| **`lunco-twin`** | The simulation unit on disk: folder structure, `twin.toml` manifest parsing, generic scalar `[settings]`, explicit Twin-approved native hook-provider entries, file indexing, and strict component-to-verification selection. |
| **`lunco-twin-journal`** | Canonical Twin-scoped op log: Lamport-ordered entries, DAG parents (for future merges), Streams + Composition, ChangeSets, Markers (named milestones), Branches, `UndoManager`. CRDT-shapable schema; in-memory backend today, yrs-swap-ready. |
| **`lunco-doc`** | Foundation for structured artifacts (Modelica, USD, SysML): process-wide live document handle allocation, the `DocumentHost` container and atomic `DocumentOp` pattern with built-in undo/redo. |
| **`lunco-doc-bevy`** | Bevy ECS integration for the Document System: lifecycle events, document-identity command payloads such as `rename::RenameOpenDocument`, `JournalResource` (Bevy wrapper around the canonical Twin journal), `BevyJournalSink` for remote-replay, `EditorIntent` keybindings, `Presence` collab seed. |
| **`lunco-storage`** | I/O abstraction layer (`Storage` trait — Native FS, Memory, future WASM/Remote backends). The single write path; raw `std::fs` is disallowed. |
| **`lunco-assets-path`** | Platform-neutral URI and relative-path algebra: scheme parsing, canonicalization, separator normalization, and traversal checks. It has no Bevy, filesystem, storage, or application dependency. |
| **`lunco-assets-core`** | Lightweight asset identity and resolution: canonical `lunco://`/`twin://` sources, cache/Twin roots, traversal-safe path/cache operations, and storage-facing identity contracts. It excludes source catalogs, scripting, text loaders, discovery, network, and archive/runtime integration. |
| **`lunco-assets-runtime`** | Bevy asset-source and authored-text runtime: source registration, discovery/catalogs, library/model/script/text loaders, web fetch integration, and the asset-manifest tool. It consumes `lunco-assets-core` without making the identity layer depend on runtime services. |
| **`lunco-assets-datasets`** | Lightweight `Assets.toml` declarations, scoped dataset identity, artifact-path contracts, process-output ownership validation, lifecycle state, and the typed request/process/cancel command contracts. It has no HTTP, archive, image, GeoTIFF, or native processing dependencies. |
| **`lunco-assets-transport`** | Small native HTTP transport boundary: shared timeout, retry/backoff, and resumable byte-transfer primitives. It has no manifest, archive, raster, or Bevy dependency. |
| **`lunco-assets-download`** | Manifest-aware native download, SHA-256 verification, archive extraction, staging, and atomic source installation. It has no Bevy or raster-processing dependency. |
| **`lunco-assets-processing`** | Native offline image/DEM/map/albedo/normal-map/glTF processors, with shared staging/commit and an open `ProcessorRegistry` selected by `ProcessConfig.kind`. Heavy decode and raster math live here. |
| **`lunco-assets`** | Explicit Bevy provisioning and manifest-processing workers plus the asset-manager CLI. It composes the lightweight dataset contract with the transport, download, and processing crates; ordinary asset readers should not depend on this package. Rhai selects and sequences authored provisioning and bake policy. |
| **`lunco-modelica-assets`** | Native Modelica asset packaging and indexing: bundles source-library files and pre-parsed Rumoca definitions for the web runtime and provides the shared editor-index tools; keeps Modelica build-only dependencies out of the generic asset manager. |
| **`lunco-hash`** | Hashing substrate: Fast tier (FNV-1a) for change/cache keys and CID tier (CIDv1 raw+sha2-256) for on-disk/on-wire content-addressing. Draws a firewall between ephemeral process keys and cross-peer persisted content. |
| **`lunco-precompute`** | Content-addressed precompute disk cache (`bake_or_load`): runs expensive pure functions once, persists results keyed by content hash (via `lunco-hash` + `lunco-storage`), and loads them on subsequent runs/peers. |
| **`lunco-settings`** | Centralised user-settings: one JSON file (`<OS config dir>/lunco/settings.json`), namespaced sections, auto-persist on change; also owns the shared `DownloadSettings` retry/backoff policy. |
| **`lunco-theme`** | Centralized design tokens (Catppuccin-based) for consistent UI across all panels and domains. |
| **`lunco-time`** | Unified mission-time spine (architecture doc 19): `MissionClock`/`TimeTransport`/`WorldTime`, the `TimeDomain` clock tree + animation transport, and the `scales` projection layer over `celestial-time`. |
| **`lunco-worker-transport`** | Generic Web Worker pool transport (wasm-only): spawn / lazy-grow, boot wire-id handshake, byte + Transferable-`ArrayBuffer` post, crash respawn. Payload-agnostic (the caller supplies decode/route callbacks); shared by the Modelica Fast-Run workers and the DEM bake worker so neither reimplements the plumbing. |
| **`lunco-status-core`** | Renderer-independent lifecycle, progress, and status infrastructure: `StatusBus`, scoped busy handles, tracked tasks, discrete diagnostics, and telemetry mirroring. Consumers such as the workbench status bar, busy widgets, and headless diagnostics read the same contract. |

---

## 2. Simulation Engine
The "Laws of Nature" — celestial mechanics, environmental state, terrain, obstacle fields, batch experiments, and co-simulation orchestration.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-celestial`** | Headless celestial semantics: canonical body catalog/NAIF identities, ephemeris contracts, typed f64 frame transforms, geodesy, body rotation, and Kepler propagation. |
| **`lunco-celestial-data`** | Dependency-free authoritative celestial constants shared by semantic and asset-processing packages without making the general core depend on the celestial domain. |
| **`lunco-celestial-spatial-core`** | Lightweight Bevy/BigSpace contracts shared by celestial consumers: semantic frame lookup, canonical surface poses, surface axes, scene body declarations, orbital-view state, cached local-gravity facts, detached celestial Sun presentation state, and render-independent connectivity state. |
| **`lunco-celestial-spatial`** | Bevy/BigSpace projection of celestial semantics: scene hierarchy, gravity, surface placement, terrain/globe integration, detached-time globe and Sun projection, render-only body-fixed marker copies, links, cadence, and runtime celestial commands. It consumes the separate presentation package for trajectory rendering. |
| **`lunco-celestial-presentation`** | Render-facing trajectory sampling and presentation for celestial runtime facts. It depends on the spatial contracts but keeps mesh/material and presentation scheduling out of the semantic spatial runtime. |
| **`lunco-celestial-ephemeris`** | Concrete high-fidelity ephemeris provider for `lunco-celestial` (VSOP2013 + ELP/MPP02 via `celestial-ephemeris`); the heavy, non-Windows-MSVC half of the celestial split and the one place `celestial-time` is allowed. |
| **`lunco-environment`** | Per-entity position-dependent environment state (atmosphere, radiation, local gravity). |
| **`lunco-terrain-core`** | Projection-agnostic terrain LOD spine: quadtree-CDLOD selection, tile-grid math, and the `HeightSource` trait. Pure (std + serde), shared by both the planar DEM streamer and the cube-sphere planetary tiler. |
| **`lunco-terrain-globe`** | Whole-body cube-sphere terrain tiling (orbital/planetary scale): quadtree-CDLOD globe, avian heightfield collision ring, `big_space` anchoring; the "globe" projection of the terrain family over the shared `lunco-terrain-core` LOD spine. |
| **`lunco-terrain-surface`** | Local high-detail DEM ground terrain (surface scale): one committed surface-change contract shared by visual tiles, collider tiles, and derived maps; independent product resolutions and caches; `big_space` per-tile anchoring and the layered color pipeline. |
| **`lunco-terrain-bake`** | Pure (bevy/avian-free) DEM bake pipeline shared verbatim by the native async task and the wasm Web Worker: GeoTIFF decode → crop/resample → crater stamp → `HeightGrid`. Owns the `dem_worker` companion binary + its main-thread client (over `lunco-worker-transport`), moving the ~40 MB decode + crater stamp off the page's main thread on web (coarse-then-full progressive). |
| **`lunco-geotiff`** | The **geo** half of a GeoTIFF: GeoKey/tie-point/pixel-scale read and write, shared by the writer (`lunco-assets-processing`) and the reader (`lunco-terrain-bake`). A raster states its own extent and projection; nothing restates it in a sidecar. See `docs/architecture/57-dem-georeferencing.md`. |
| **`lunco-physics`** | The physics **readiness, backend-admission, determinism, solver-configuration, contact-fact, raw-query, Avian force-port, and joint-mechanics contract owner** — `avian_backend` is the single numeric/shape contract for Avian's f64-to-f32 points, AABBs, and compound-child structure; lifecycle bridges decide when to admit it. It owns the shared contact aggregation, Avian impulse-to-load conversion, raw `raycast::RaycastObservation` sampling, `force_ports` admission classification, and reusable revolute-joint torque/axis, port-name, and holder-selection mechanics used by telemetry, co-simulation ports, mobility, and render/editor consumers. It decides whether the world is safe to integrate, installs the single cross-platform Avian eight-substep contract, publishes the explicit `PhysicsDeterminism` admission state, and keeps Avian's collider-tree optimization on the owner schedule so async worker joins cannot stall a physics tick. A DEM still baking or a collider ring not yet paged in suspends integration without touching the user's transport clock, so a `Dynamic` body cannot free-fall through a collider that does not exist yet. |
| **`lunco-obstacle-field`** | Procedural crater + rock field generation (with LOD) for rover testing. |
| **`lunco-experiments`** | Backend-agnostic experiment / batch-run registry: models a single Fast Run as a first-class artifact (params, bounds, trajectory) with `RunStatus` (`Pending`/`Queued`/`Running`/`Done`/`Failed`/`Cancelled`) and `RunBounds`; the sim backend plugs in via the `ExperimentRunner` trait, parallel runs schedule across a worker pool. |
| **`lunco-experiments-ui`** | Render-independent experiment view state: per-plot variable/run selections, Twin-scoped plot-state preservation, active-plot selection, and the change-gated trajectory sample cache. It has no Modelica, document, egui, or workbench ownership. |
| **`lunco-cosim-core`** | Backend-neutral co-simulation contracts: `SimComponent`/`SimStatus`, `SimConnection`, `ConnectionBinding`/`BoundConnection`, `BindingRevision`, diagnostics, control holds, generic force/torque actuator metadata, realtime-safety marker, typed control commands (`SetPorts`, `ReleasePort`, `ReleaseControl`), shared connector constants, and the `schedule::{CosimSet, CosimApplySet}` fixed-step schedule anchors. |
| **`lunco-cosim`** | Co-simulation orchestration for Modelica, FMU, GMAT, and Avian participants. Owns binding, port backends, joint/force application, fixed-step propagation, and the `ControlAuthorityChanged` adapter that safe-stops released live endpoints. It consumes schedule anchors from `lunco-cosim-core`; backend-neutral participants do not depend on this Avian-backed package merely to order their systems. |

---

## 3. Vessel Control & Hardware
The "Brains and Brawn" — Flight Software (FSW), On-Board Computer (OBC), mobility physics, and robotics assembly.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-mobility`** | Parameterized surface-vehicle physics: contact-plane raycast wheels (incl. leaning bikes), suspension, drive mixing, rocker-bogie differential. |
| **`lunco-control-core`** | Generic semantic-control contracts: `ControlLink`, `AcquireControl`, `ReleaseControlSource`, the shared `UserIntent` vocabulary, authored intent-to-port bindings, input state, egui focus gate, interaction schedule boundary, and bounded causal-edge trace. Input producers and domain consumers depend on this focused package instead of placing control policy in `lunco-core`. |
| **`lunco-interaction-core`** | Small cross-runtime cursor-interaction contract: registered USD button policy, semantic exclusive possession arbitration, editor tool gates, drag state, and the affected-entity marker consumed by camera, possession, and follow runtimes. It contains no editor implementation. |
| **`lunco-input-core`** | Shared user input settings: the bundled keyboard/pointer map, persisted overrides, semantic labels, pointer-chord resolution, and Leafwing `InputMap` projection. It is the focused input contract used by controller, avatar, UI, and Rhai consumers. |
| **`lunco-input-ui`** | Optional egui presentation for the shared input state: the recording/observation input overlay and its typed visibility command. It does not translate input or own vessel control. |
| **`lunco-camera-core`** | Backend-neutral camera-rig contracts and reusable pose math: free-flight, orbit, spring-arm, surface, smoothing defaults, pose-transition state, adaptive clip-plane math, camera input accumulators, deterministic authored-camera display labels, and the `camera.default_presentation` policy-hook contract. Device translation, rendering, and UI adapters consume these contracts. |
| **`lunco-camera-runtime`** | Generic interactive camera realization: camera-mode exclusivity, the one-writer interaction-easing rule, frame-handoff rebasing, free-flight/surface pose writers, persisted camera-input settings, and the typed `SetCameraInput` command over the camera-core contracts. Rhai authors presentation policy through the generic command surface. |
| **`lunco-embodiment-core`** | Backend-neutral embodiment contracts: ECS role markers and the derived local-embodiment index. Camera, control, notification, spatial-handoff, input, and presentation adapters are supplied by focused packages. |
| **`lunco-notifications-core`** | Backend-neutral transient notification command and queue contracts. The application runtime consumes the command; optional UI adapters render the resulting toasts. |
| **`lunco-avatar-camera-core`** | Avatar-specific camera transition contracts: BigSpace orbit-return state, transient orbit history, arrival/input markers, and surface/orbit handoff constants. It depends on the generic camera contracts without making generic camera consumers carry avatar frame state. |
| **`lunco-avatar-input`** | Avatar-specific semantic input runtime: pointer look, unit-normalized wheel zoom, camera behavior updates, and pause/cancel intents. It consumes shared control and camera contracts without coupling input changes to possession/authority implementation. |
| **`lunco-avatar-policy`** | Twin-scoped avatar safety policy and physical collision-controller settings. It is shared directly by the runtime and UI, so the UI does not depend on the monolithic avatar implementation. |
| **`lunco-avatar-camera`** | Avatar-specific camera realization: typed subject-binding and release transactions, explicit inertial BigSpace orbital placement, focus/return transactions, interactive-camera initialization, bounded body resolution, vessel spring-arm follow, collision-aware local locomotion, and the surface/orbit lifecycle handoff. It consumes avatar camera contracts as a focused runtime package. |
| **`lunco-avatar`** | Headless-safe specialized local-avatar runtime: control authority, scene interaction, and avatar control transactions. A successful camera-bound possession emits a generic camera transaction; subject binding, follow policy, and presentation transitions are realized by `lunco-avatar-camera`. Semantic input projection lives in `lunco-avatar-input`; generic camera realization and easing ownership live in `lunco-camera-runtime`, and optional egui presentation in `lunco-avatar-ui`. Camera selection lives in `lunco-usd-bevy-camera`; the shared viewport contract lives in `lunco-viewport-core`. |
| **`lunco-avatar-ui`** | Optional egui presentation adapter for `lunco-camera-core`, `lunco-embodiment-core`, and `lunco-avatar-policy`: avatar status panel, camera/name-tag and notification overlays, and the Avatar settings row. It does not depend on the avatar runtime implementation. |
| **`lunco-hardware`** | Concrete physical actuators and sensors bridging `Port` values to the `avian3d` physics engine. |
| **`lunco-controller`** | Specialized vessel-control adapter: translates semantic `UserIntent` actions into authored port writes, handles control authority and input injection, and yields a vessel to its owning session (spec 034). Shared keymap settings live in `lunco-input-core`. |

---

## 4. USD Integration Layer
Modular bridge between OpenUSD and Bevy, covering visuals, physics, simulation metadata, and materials.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-usd-document`** | Headless authored OpenUSD document/layer lifecycle and typed operation substrate: `UsdDocument`, layer identity, edit history, and document state. No runtime, physics, rendering, or UI. |
| **`lunco-usd-data`** | Reusable render-free authored USD data contracts: metadata, stage units/conventions, and composed-value readers. No document lifecycle, authoring registry, runtime, physics, rendering, or UI. |
| **`lunco-usd-authoring`** | OpenUSD authored-layer operations and schema registry: path-addressed authoring, USDA conversion, reference/list-op helpers, and registered schema metadata. No document lifecycle, runtime, physics, rendering, or UI. |
| **`lunco-usd-core`** | Headless typed USD operation, assembly, edit-session, and edit-policy substrate: `ApplyUsdOp`/`ApplyUsdOps`, disposable `ApplyUsdTransientOps`, and operation lowerings. No document implementation, runtime, physics, rendering, or UI. |
| **`lunco-usd-queries`** | UI-free public USD query providers for document inspection, edit-session state, explicit assembly-target resolution, and document synchronization. Public query behavior is covered by authored USD/Rhai scene tests. |
| **`lunco-usd-commands`** | Headless USD document and authoring command boundary: document kind registration, open/new/save/undo/redo, document lifecycle, and typed USD authoring commands. It owns no scene admission or visual projection. |
| **`lunco-usd-bevy-runtime-core`** | Headless-safe USD scene runtime: scene admission, Twin-backed stage loading, live document-to-stage projection, and generic projection-change boundaries consumed by domain adapters. Authored control/program behavior is composed from `lunco-usd-bevy-authored-runtime`; runtime overlay persistence is composed from `lunco-usd-bevy-runtime-persistence`. |
| **`lunco-usd-bevy-authored-runtime`** | Reusable Bevy adapter for authored USD control surfaces and generic `LunCoProgramAPI` behavior. It consumes the visual projection boundary and owns neither input policy nor scene admission. |
| **`lunco-usd-bevy-runtime-persistence`** | Twin-scoped persistence of the generated USD runtime overlay through the shared storage boundary. It owns the opt-in load/save observers and restore operation; it has no scene projection or UI policy. |
| **`lunco-usd-bevy-scene-ports`** | Reusable Bevy adapter for USD-connected scene-property sinks: light intensity/radius/color channels and transform components. It owns the `PortBackend` and lifecycle markers; the aggregate USD runtime installs it explicitly. |
| **`lunco-usd-bevy-runtime`** | Application-level composition of the USD runtime, visual, diagnostics, physics, simulation, and document-command plugins. The default `simulation` feature includes the standard vehicle/simulation projector; lean hosts can omit it, while the Modelica/Rhai co-simulation projection is opt-in through `cosim` (which implies `simulation`). |
| **`lunco-usd-geometry`** | Render-free USD geometry substrate: BasisCurves evaluation, NURBS evaluators, trimmed-domain tessellation, and rotation-minimizing curve-sweep mesh data. Isolates heavy numeric geometry dependencies from stage and camera policy. |
| **`lunco-usd-bevy-stage`** | Renderer-independent composed-USD stage boundary: stage loading/composition, authored-layer reads, canonical stage ownership, instance identity, projection plans, standard material/purpose/variant readers, transforms, units, authoring helpers, and stage integration tests. It has no runtime projection systems. |
| **`lunco-usd-bevy-core`** | Small runtime projection-mechanism package: authored animation, live edits, mount coordination, point instancers, and executable program runtime. It consumes `lunco-usd-bevy-stage`; stage readers and canonical APIs are not re-exported from this crate. |
| **`lunco-usd-bevy-scene`** | Render-free Bevy scene contract shared by visual and domain projections: `UsdPrimPath`, scene/revision lifecycle markers, projection ordering boundaries, generic projection-reset and authored info-change messages, visual-split markers, preview/ancestry ownership, authored billboard contracts, canonical USD primitive/mesh geometry readers, and composed collision/placement envelopes. It depends on the core reader and has no visual adapter or renderer dependency. |
| **`lunco-usd-bevy-twin`** | Render-free Twin-backed USD document identity: document-to-`twin://` lookup, workspace/preview leases, projection cursors, user-ownership events, and the live-projection wake signal. It owns no stage loading, composition, rendering, or UI. |
| **`lunco-usd-bevy-camera`** | Render-free USD camera adapter: standard `UsdGeomCamera` projection/look-at intent, camera roles/pose, mounted/cinematic camera pose, camera-track selection, the `camera.default_presentation` fact/decision boundary, and the single-authority viewport-camera reconciler. It contains no avatar behavior parser or raw input mapping and does not own geometry math or visual projection. |
| **`lunco-usd-bevy-lathe`** | Independent parametric NURBS/lathe projection: reflected surface definitions, profile evaluation, and change-detected Bevy mesh regeneration. |
| **`lunco-usd-bevy`** | Visual Bevy adapter (`UsdVisualPlugin`): projects USD hierarchy, shapes, transforms, and material intent into Bevy entities/components on top of `lunco-usd-bevy-stage` and the focused runtime projection mechanisms. Owns async projection orchestration while consuming mesh geometry from `lunco-usd-bevy-mesh` and installing the independent camera and light adapters. |
| **`lunco-usd-bevy-mesh`** | Render-free USD visual mesh projection for built-in primitives, native `UsdGeomMesh`, `BasisCurves`/`NurbsCurves`, and `NurbsPatch`, including quality invalidation and low-level geometry tests. |
| **`lunco-usd-bevy-animation`** | Render-free animation adapter (`UsdAnimationPlugin`): binds projected USD prims to the shared time domains, plans authored `timeSamples` topology, and samples transform/visibility/material intent. It depends on the core reader and visual scene contract, not on mesh projection. |
| **`lunco-usd-bevy-light`** | UsdLux light and textured dome projection: authored light components, ambient-dome semantics, HDRI equirectangular-to-cubemap conversion, environment-camera binding, and light-owned live refresh from the generic scene info-change boundary. It is independent from the visual mesh projector. |
| **`lunco-usd-bevy-diagnostics`** | Optional visual USD asset-failure and placeholder diagnostics: glTF fallback hiding, load-time replacement stubs, and labeled failure geometry. Installed by `lunco-usd-bevy-runtime`; kept separate from the visual projector. |
| **`lunco-usd-avian-filters`** | Render-free USD/Avian collision-filter boundary: interprets standard `PhysicsFilteredPairsAPI` and `PhysicsCollisionGroup`, owns transient joint-pair suppression, and installs the single Avian collision/contact hook. |
| **`lunco-usd-avian-contracts`** | Shared USD/Avian ECS carriers, normalized joint-drive contract, and generic physics-projection lifecycle seam used by physics projection, runtime live edits, readiness, queries, and co-simulation; contains no stage traversal or projection systems. |
| **`lunco-usd-avian-reader`** | Shared render-free composed OpenUSD physics readers for collider geometry, joint topology/limits/drives, and typed physics attributes. Reused by the runtime Avian projector and authored-stage lint without owning Bevy systems or lifecycle. |
| **`lunco-usd-avian-joints`** | Reusable native Avian joint boundary: typed constructors, joint-pair filtering, seating, solver-island admission, and graph-safe detach. It consumes shared contracts and is usable by authored USD and synthesized mechanisms. |
| **`lunco-usd-avian`** | USD physics projection (`UsdAvianPlugin`): maps `UsdPhysics` schemas (RigidBody, Colliders, all joint kinds + drive API) to normalized Avian joint plans and body/collider components. Native joint lifecycle is owned by `lunco-usd-avian-joints`; its USD physics-material reader is isolated from the main projection module. It consumes the independent collision-filter package and `lunco-usd-avian-reader`; lint fact production is in `lunco-usd-avian-lint`. |
| **`lunco-usd-actuation`** | Render-free composed USD force/torque actuator reader. It converts authored actuator geometry and limits into generic co-simulation components without depending on the Avian runtime projection. |
| **`lunco-usd-avian-lint`** | Render-free composed `UsdPhysics` fact producer for the authored Rhai lint policy. It reuses Avian's authoritative geometry/joint readers without making the runtime physics crate own lint orchestration. |
| **`lunco-usd-sim`** | Vehicle-specific simulation-schema bridge (`UsdSimPlugin`): intercepts specialized schemas such as PhysX Vehicles and maps them to LunCo mobility models. It registers the vehicle wheel owner with `lunco-usd-bevy-core` for in-place live edits. It publishes avatar role/spatial identity only; camera behavior is owned by the avatar runtime and authored Rhai. It no longer installs the heavy USD cosim translator. |
| **`lunco-usd-sim-authoring`** | Render-free composed readers for PhysX vehicle wheel-attachment and gear-drive authoring, shared by vehicle projection and scene validation; it also publishes the corresponding typed USD lint facts. Runtime ECS resynchronization remains in `lunco-usd-sim`, registered through the generic live-edit owner in `lunco-usd-bevy-core`. |
| **`lunco-usd-sim-core`** | Small render-free protocol package for the shared USD simulation schedule, processed marker, pending differential contract, physical-wheel display state, and ground-collider readiness state used by vehicle, cosim, editor, and scene-runner packages. It contains no projection systems. |
| **`lunco-usd-sim-cosim`** | USD-authored program discovery, connection wiring, readiness, and Modelica/Rhai participant projection (`UsdSimCosimPlugin`). API query providers are isolated in `lunco-usd-sim-cosim-api`; generic scene admission and mounting belong to `lunco-usd-bevy-runtime-core`. |
| **`lunco-usd-sim-cosim-api`** | Optional API query providers for cosimulation ports, status, causal traces, binding diagnostics, camera audits, and broken-connection reports. It depends on the runtime projection but keeps API/JSON serialization out of the default cosimulation crate's direct source and dependency set. |
| **`lunco-usd-sim-domain`** | Render-free USD domain projection: its public `network` module reads and validates component-network facts, `synthesis` owns Rhai-backed policies and generated-plan contracts, and the parent module owns Modelica member-class lifecycle plus ECS projection. Generic USD actuator lowering lives in `lunco-usd-actuation`; its optional API query providers live in `lunco-usd-sim-domain-api`. |
| **`lunco-usd-sim-domain-api`** | Optional API query providers for generated Modelica source inspection. Kept outside the render-free domain projector so its direct dependency set does not include the `lunco-api`/JSON query surface. |
| **`lunco-usd-sim-celestial`** | Independent render-free projector for USD-authored celestial anchors, orbits, link nodes, occluders, and reflected-light metadata. It owns the celestial projection marker and does not depend on vehicle or cosimulation projection. |
| **`lunco-usd-sim-shader`** | Independent render-free projector for `UsdShade` WGSL material intent. It authors `ShaderLook` and owns its shader-resolution marker; generic scene refresh invalidation arrives through the scene contract rather than a runtime-to-shader dependency. |
| **`lunco-usd-sim-telemetry`** | Independent render-free Avian rigid-body and wheel telemetry recorder. It publishes through the shared signal/telemetry registries and is isolated from USD vehicle projection changes. |
| **`lunco-usd-terrain`** | Terrain bridge: projects authored terrain prims into `lunco-terrain-surface`'s `DemTerrainRequest` + composable `TerrainLayerStack` (craters / rocks / edits), and carries hand edits back as journaled, undoable USD ops on the document's **runtime** layer. Standard `UsdShade` owns terrain material intent. |
| **`lunco-scene-command-contracts`** | Typed scene-command payloads shared by editor producers and runtime handlers. It owns no observer behavior; UI packages depend on these contracts instead of the scene-handler implementation. |
| **`lunco-scene-commands`** | The render-free scene/document **mutation handlers**: runtime spawn, move, delete, and USD connection edits. It owns command observers, API reflection registration, and `SpawnCommandPlugin`; catalog resources live in `lunco-scene-catalog`, document-backed property and shader authoring live in `lunco-scene-authoring`, and camera commands live in `lunco-scene-camera`. |
| **`lunco-scene-camera`** | Render-free scene-camera adapters (`FocusEntityById` and `FocusEntityByPath`) plus the spatial observer for the generic `lunco-camera-core::SetCameraLookAt` and resolution of stable scene targets into `PendingFocus`. It does not choose an embodiment or perform camera-rig policy. |
| **`lunco-scene-selection`** | Render-free scene-lifetime selection state and the shared selection-to-telemetry-focus projection. It is independent from scene mutation commands so exposure and other headless consumers do not inherit the command layer's dependency closure. |
| **`lunco-scene-catalog`** | Production USD-backed catalog boundary: asynchronous asset enumeration, authored `doc`/`lunco:spawnable` metadata, parser/read status, shader/source listings, `SpawnCatalog`, and the generic runtime USD spawn constructor. `ListSpawnCatalog` and `ListUsdAssetMetadata` publish the same authored `doc` text consumed by the catalog UI and Rhai scene tests. It also owns catalog-only rescan commands and is installed by `SpawnCommandPlugin`. |
| **`lunco-scene-queries`** | Production read-only scene boundary: `QueryEntity` for active-physics identity/pose, `QueryUsdPrim` for one composed USD prim, and `QueryUsdPrims` for deterministic multi-prim reads from one composed-stage snapshot. Installed by `SpawnCommandPlugin`; shared by Rhai, HTTP, MCP, and headless hosts. |
| **`lunco-scene-authoring`** | Production USD authoring boundary: document ownership resolution, `SetObjectProperty`, standard USD property persistence, and journaled/live shader source commands. Installed by `SpawnCommandPlugin`; it has no dependency on scene mutation handlers. |
| **`lunco-scene-validation`** | Production asset, loaded-stage, scene-test discovery, and Twin pre-flight: `ValidateAsset`, `ValidateTwin`, source-focused `ValidateSysml`, selectable typed-fact `AnalyzeSysml`, `luncosim test --list`, live `RunLint`, USD/SysML lint-fact aggregation, and Twin namespace inspection. It owns composition/parse/lint integration while `lunco-scene-commands` owns scene mutation. |
| **`lunco-materials`** | Shader appearance **intent**, render-free: `ShaderLook` (`.wgsl` path + open `dyn_params` + texture layers), the WGSL-reflected param schema, the CDLOD vertex attribute. Names no material type. |

---

## 5. Networking & API
External communication, ECS replication, telemetry extraction, and distributed attributes.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-networking-core`** | Transport-independent client netcode: snapshot interpolation, ownership prediction, rollback/reconciliation, correction smoothing, and their session state. It has no WebTransport/lightyear dependency. |
| **`lunco-networking-scenario`** | Transport-neutral scenario manifest and asset wire contracts: CID identity/revision calculation and manifest/chunk/offer messages. It has no Bevy, transport, asset I/O, or sync-runtime dependency. |
| **`lunco-networking-sync`** | Transport-neutral Bevy synchronization runtime: replicated state, journal convergence, scenario asset transfer, typed bounded envelopes, sync policy execution, and the ECS resources that hold local/remote scenario manifests. Scenario wire contracts live in `lunco-networking-scenario`; this package has no WebTransport/lightyear dependency. |
| **`lunco-networking`** | Multiplayer replication and lightyear WebTransport adapter. It owns transport setup, peer/session handshakes, channel ferrying, and network-only adapters while consuming the transport-neutral `lunco-networking-sync` runtime. |
| **`lunco-api-contracts`** | Pure API wire envelopes and shared API endpoint constants. It has no ECS or language-runtime dependency, so native clients and transport adapters compile against the same contract without linking the runtime. |
| **`lunco-api-client`** | Generic native command-API client. It owns endpoint configuration and HTTP request/response handling; it knows no Rhai command or simulator implementation. |
| **`lunco-api-core`** | Lightweight typed API values and request/response/schema contracts. It depends on the language-neutral hook value ABI, not the ECS API runtime or transport wire format. |
| **`lunco-api`** | ECS API runtime: typed command/query execution, reflection-based discovery, entity identity, query registration, and response/telemetry infrastructure. In-process callers exchange `ApiValue`; it has no JSON conversion. |
| **`lunco-api-codec`** | Converts typed API values to/from JSON at HTTP and networking wire boundaries. It has no ECS or scripting-runtime dependency. |
| **`lunco-api-transport`** | Application-bound API transports: native Axum HTTP listener, asset endpoint, and wasm browser bridge. It adapts the pure wire contract through `lunco-api-codec` to the typed ECS API runtime. |
| **`lunco-telemetry-core`** | Transport-neutral telemetry contracts, typed event/value bus, reflection registration, black-box logging, and projection of generic core lifecycle facts into telemetry. It does not own sampling or retained history. |
| **`lunco-telemetry`** | Telemetry channels: per-channel rate + deadband, bound to a `TimeDomain` (so pause/warp come free), retained in `lunco-signal`'s ring buffer, plus the OpenMCT-shaped query surface (catalog / history / recording). |
| **`lunco-signal`** | The signal DATA model — `SignalRegistry`, `SignalRef`, `ScalarHistory`, and the backend-neutral `SimRegistry`/`SimStream` snapshot publication path. **Render-free by construction**: split out of `lunco-viz` (which links bevy_egui → bevy_render) so a headless run can retain history without a GPU stack. `lunco-viz` re-exports the signal registry. |

---

## 6. Workbench & UI Tools
The editor shell, visualization framework, generic 2D canvas, in-scene/luncosim editing tools, render look, and web boot.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-workbench-core`** | Renderer-independent workbench contracts: `Panel`/`PanelCtx`, instance tabs, tab/source-view commands, scene display state, pending close state, panel registration, perspective layout plans, menu contributions, the published `WorkbenchSnapshot`, and shell scheduling labels. It uses the Bevy ECS substrate and egui types but does not pull `bevy_render`, `bevy_egui`, `egui_dock`, storage, or window/render services. |
| **`lunco-viewport-core`** | Small renderer-independent measured viewport geometry contract. Owns the physical-pixel `PanelRect` value shared by scene, camera, editor, and shell adapters without coupling that value to egui or the Workbench implementation. |
| **`lunco-workbench-widgets`** | Shell-independent egui presentation primitives: semantic vector icons, standard text editors, and consistent hierarchy rows. Lightweight panel crates use it without linking the concrete dock shell. |
| **`lunco-workbench-layout`** | Renderer-independent `egui_dock` layout state: perspective registration/activation, dock snapshots, panel placement, split sanitization, and scene-interaction synchronization. It consumes workbench contracts/state without the concrete Bevy/egui shell. |
| **`lunco-workbench-perf-ui`** | Reusable performance capability: persisted HUD settings, typed toggle command, Bevy frame diagnostics, and live `PerfStats`. Physics adapters publish optional step timing into this package; the concrete shell only renders the values. |
| **`lunco-workbench`** | The concrete IDE-like shell: `bevy_egui` rendering, panel-host consumption, viewport integration, and shell-owned command observers. Dock layout state and perspective materialization are supplied by `lunco-workbench-layout`; headless adapters use the core/layout contracts without linking this shell. |
| **`lunco-workbench-help-ui`** | Optional rendered Help/About perspective presentation: help registry, perspective help menu item, and version/source display. It consumes workbench contracts without putting egui rendering into `lunco-workbench-core. |
| **`lunco-workbench-guided-ui`** | Optional application-level guided presentation: Rhai-driven persistent HUDs, widget spotlights, coach-mark tours, and recoverable guided-target surfaces. It consumes the workbench core's generic anchors and render-set contracts but does not make the base shell depend on guided/tutorial behavior. |
| **`lunco-workbench-file-dialog`** | Reusable native/wasm file-dialog capability: typed open/save/folder requests, backend resolution events, browser-picked text, and browser downloads. It owns dialog dependencies (`rfd`/wasm DOM) outside storage, document, and shell contracts. |
| **`lunco-workbench-file-ops`** | Reusable windowed file-workflow adapter: typed picker commands, picker-result routing, Twin/document save and rename coordination. It reuses `lunco-storage`, `lunco-workbench-file-dialog`, `lunco-workspace`, and document contracts without making storage own UI policy. |
| **`lunco-workbench-runtime-ui`** | Reusable HUI/Flair runtime-authored surface mechanism: manifest loading, retained surface lifecycle, placement, styling, input regions, capture readiness, and generic semantic action transport. Hosts supply exposures, gates, Twin state, and recorder mode; application-specific actions stay outside this crate. |
| **`lunco-workbench-text-editor`** | Reusable generic source editor: source-only text paths, asset/Twin/ephemeral source loading, async storage writes, tab lifecycle, and source-editor panel registration. It reuses the core source/tab contracts and shared text-editor widget without depending on the concrete docking shell. |
| **`lunco-workbench-state`** | Reusable per-Twin session state: persisted document snapshots, domain codec registry, dock snapshot schema, runtime-surface layout state, storage-backed load/save, and the layout-provider contract used by concrete shells. |
| **`lunco-workbench-window`** | Reusable OS-window capability: typed minimize/maximize/close commands, merged-titlebar construction, settings-backed geometry persistence, and explicit native placement. It depends on Bevy window APIs and `lunco-settings`, not the concrete Workbench shell. |
| **`lunco-workbench-browser`** | Reusable Twin and Files browser feature: browser section registry and query state, filesystem and library navigation, rename/open actions, and the `TwinBrowserPanel`/`FilesPanel` surfaces. It depends on workbench core/widgets and document/workspace contracts, but not the concrete docking shell or dataset processing stack. |
| **`lunco-workbench-datasets-ui`** | Optional browser presentation for Twin-declared downloadable resources. It projects `lunco-assets-datasets`' shared registry and emits its typed request/cancel events without making the generic browser depend on provisioning and processing. |
| **`lunco-capture`** | Render-bound screenshot and deterministic offline-recording capability: typed capture commands, GPU readback, frame pacing, PNG/video sinks, and recording status. It is an application capability shared by the workbench and windowless/offscreen hosts, not a workbench subsystem. |
| **`lunco-ui`** | Reusable UI infrastructure: cached widgets, 3D world panels, command builders, and the shared bounded log model/renderer. It uses workbench contracts and shell-independent widgets, not the concrete workbench shell. |
| **`lunco-viz-core`** | Render-free visualization identity substrate: the shared `VizId` used by visualization registries, plot state, and headless consumers. It has no Bevy or rendering dependency. |
| **`lunco-viz`** | Domain-agnostic visualization: `SignalRegistry`, LinePlots, reusable multi-series trajectory plots, and future 3D/Rerun bridges. |
| **`lunco-canvas`** | Stateful 2D scene editor substrate for diagrams and annotation overlays. |
| **`lunco-luncosim-edit-core`** | Headless-safe scene-editing mechanisms: spawn and terrain tools, scene picking, typed command registration, and ECS state. |
| **`lunco-luncosim-edit-gizmo-ui`** | Focused rendered transform-gizmo capability: render-space proxies, live/preview pose transactions, camera binding, and kinematic-drive lifecycle. It owns the external `transform-gizmo-bevy` dependency independently of the editor panels. |
| **`lunco-luncosim-edit-ui`** | Rendered scene-editing presentation: egui/workbench panels, selection and preview adapters, and physics diagnostics. It composes the focused transform-gizmo package. |
| **`lunco-luncosim-edit-inspector-core`** | Renderer-independent Inspector readout snapshot and change gate. It owns the bounded ECS queries for sun, camera, ambient, and joint facts; the rendered Inspector consumes this package without moving those scans into egui paint. |
| **`lunco-luncosim-edit-inspector-ui`** | Domain-heavy Inspector and authored USD panels: standard USD joint/animation/mount/variant/parameter view models plus environment/entity authoring surfaces. It is installed explicitly by windowed composition roots. |
| **`lunco-usd-prim-tree-ui`** | Reusable composed-USD prim hierarchy panel and reactive view model. It is independent of the domain Inspector and its physics/environment authoring dependencies. |
| **`lunco-render`** | Render-free appearance intent, including screen-constant marker sizing and visibility, and typed graphics settings; `RenderQualityPolicyPlugin` resolves Rhai-owned profiles for graphical and headless scene projection. Names `Mesh3d`, never `MeshMaterial3d`. |
| **`lunco-render-recovery`** | Render-bound GPU health and presentation recovery: wgpu error handling, adapter shadow-capability admission, bounded failure escalation, presentation gating, and scene-teardown rearming. It is independent of the workbench shell. |
| **`lunco-render-bevy`** | The **only** crate that names `bevy_pbr`. Binds the intent (`PbrLook`/`ShaderLook`/`SceneCamera`/`WorldLabel`) to real materials & cameras; owns `ShaderMaterial`. Headless never adds it. |
| **`lunco-web`** | Shared web frontend for wasm apps: streaming loader, `WebReadyPlugin`, and the HTML/CSS/Rhai tool host routed through `lunco_rhai`. |

---

## 7. Scripting & Modeling
Logic engines for dynamic simulation behavior, the tool registry, and industrial modeling.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-modelica-runtime`** | Render-free Modelica runtime contract: the `ModelicaModel` ECS component, worker command/result protocol, source asset loader, generated-source metadata, communication schedule, notices, sample stream, and telemetry layout. It deliberately has no Rumoca compiler, worker implementation, document editor, or UI closure. |
| **`lunco-modelica-telemetry`** | Render-free Modelica telemetry capability: retains landed solver variables in the shared signal registry, applies the shared rate/retention/channel policy, and publishes inspectable Modelica metadata. It is installed by the execution host and is separate from compiler/document ownership. |
| **`lunco-modelica-index`** | Reusable Modelica metadata boundary: AST-derived document index, source-library editor-index artifact, diagram metadata/data, package-browser value types, class lookup, documentation extraction, and authored connect-line extraction. It is separate from the compiler host so asset/index consumers rebuild independently of worker and solver changes. |
| **`lunco-modelica-library`** | Shared source-library capability: persisted library-root settings, parsed-source bundle admission, browser fetch/decode, lazy source unpacking, editor-index handoff, and the typed Modelica worker bridge. It is a production runtime package, not a test harness; compiler and execution hosts consume its contracts without owning its transport implementation. |
| **`lunco-modelica-source-roots`** | Twin/workspace Modelica source-root admission: root inventory, manifest-aware Twin path resolution, demand-driven worker loading, and source-root readiness state. It is the host lifecycle adapter; the document/runtime core consumes no Twin-specific admission policy. |
| **`lunco-modelica-compiler`** | Headless Rumoca compiler and source-admission host: one production `ModelicaCompiler` session, source-root seating, strict reachable-DAE compilation, diagnostics, and library revision tracking. It is shared directly by workers, runners, asset tooling, and command-line hosts; tests exercise this same production package. |
| **`lunco-modelica-core`** | Headless Modelica document/runtime host: document lifecycle, compiler-engine resource synchronization, and UI-agnostic runtime contracts. It consumes the compiler, source-library, and source-root capabilities but does not own Twin-specific source admission, Rumoca compilation, source-library transport, solver workers, Fast Run execution, browser fetch, document editing, editor indexing, pure annotation values, solver implementation, API query registration, or generated USD-document metadata. It has no workbench, egui, tutorial, or UI dependency. |
| **`lunco-modelica-runner`** | Modelica experiment backend: source snapshots, compile-once DAE caching, native scheduling, shared batch/interactive run paths, run-bound resolution, and experiment-side Bevy resources. It consumes compiler and solver contracts without owning worker transport. |
| **`lunco-modelica-worker`** | Stateful Modelica worker engine: live steppers, command dispatch, worker-local artifact/prepared-solve caches, native worker loop, and the Bevy co-simulation bridge. It is a production runtime package, not a test harness. |
| **`lunco-modelica-execution`** | Modelica execution host: plugin assembly, native worker launch, and wasm worker transport. It composes `lunco-modelica-worker` and `lunco-modelica-runner` through explicit contracts; compiler-only consumers do not inherit this host. |
| **`lunco-modelica-solver`** | Renderer-free Modelica solver capability: Rumoca backend registration, solver-option translation, adaptive live sessions, and the deterministic fixed-step session. The execution host supplies lowered solve models and owns worker lifecycle; this package owns numerical integration construction and solver-specific dependencies. |
| **`lunco-modelica-api`** | Production transport-free API capability for Modelica: registers document, compiler, experiment, solver, source, and run query providers plus document edit, headless scratch-document creation, and explicit-snapshot solve commands. It depends on the headless document/runtime core and compiler contracts but is not part of the compiler host's default closure; Workspace queries remain in `lunco-workspace-api`. |
| **`lunco-modelica-ui-core`** | Render-independent Modelica UI contracts: shared command/event payloads (`OpenClass`, `FocusDocumentByName`, `SetModelicaParameter`) and stable plot identities. It has no Modelica compiler, workbench shell, panel, or renderer dependency; observers remain in the owning UI package. |
| **`lunco-modelica-ui`** | Modelica workbench UI and `lunica` application facade. It adapts core state to workbench contexts and owns Modelica panels, diagram/editor adapters, onboarding, and experiment-result view-models; reusable log, icon, documentation, and trajectory rendering lives in shared UI crates. It has no tutorial catalog or lifecycle. |
| **`lunco-modelica-icon-ui`** | Reusable egui renderer for authored Modelica `Icon`/`Diagram` graphics, including orientation, text substitution, themed colors, polygon tessellation, and bitmap loading through the source-library asset boundary. It has no workbench panel or document lifecycle ownership. |
| **`lunco-modelica-docs-ui`** | Reusable egui Modelica documentation renderer: cached HTML-to-Markdown conversion, CommonMark presentation, and workbench URI-link dispatch. It does not resolve documents or own panel selection. |
| **`lunco-modelica-ast`** | Pure Modelica source boundary: BOM normalization, strict/recovering Rumoca parse wrappers, AST interface/component extraction, typed Icon/Diagram/Placement annotation values and extractors, icon transforms, source-derived memo primitives, shared expression/description display projections, and Modelica lint facts. It has no Bevy, UI, worker, storage, or solver ownership; authored lint policy remains in `assets/scripting/policy/lint_modelica.rhai`. |
| **`lunco-sysml-ast`** | Pure SysML v2 parser/resolver projection: source files, snapshot-scoped element/feature handles, typed literals and source-spanned constraint expression trees, requirements, verification links, diagnostics, and policy-neutral fact tables/selectors. It has no Bevy, filesystem, Twin, or scripting ownership. |
| **`lunco-sysml-report`** | Optional serialized report adapter for an explicit external boundary. It owns no parsing, filesystem, Twin, Bevy, or verdict policy; the in-process `AnalyzeSysml` Rhai path uses typed `HookValue` facts instead. |
| **`lunco-sysml`** | SysML source asset/document lifecycle and journal integration. It consumes the pure AST projection but does not own API validation or Rhai policy. |
| **`lunco-sysml-rhai`** | Native Rhai adapter for immutable typed SysML snapshots, handles, expressions, and requirement/verification reports. It does not parse source or query USD; domain feature mappings and constraint policies are authored in Rhai. |
| **`lunco-scripting-bridge-core`** | Interpreter-free reflected world mechanism: native value construction, ECS reads/writes, typed `HookValue` command/query dispatch, hierarchy, authority, and capability facts. It has no JSON transport dependency. Clock and domain projections are isolated in dedicated adapters. |
| **`lunco-scripting-bridge-spatial`** | Domain adapter for active-frame pose, rotation, navigation, geolocation, and entity enumeration. It owns the BigSpace, celestial, Avian, and steering dependencies needed by those projections. |
| **`lunco-scripting-bridge-time`** | Domain adapter for deterministic simulation-clock reads and the clock-domain snapshot. It owns the physics and time-domain dependencies needed by those projections. |
| **`lunco-scripting-bridge-usd`** | Domain adapter for composed USD document generations and prim-path identity lookups. It owns the USD document/scene dependencies needed by those projections. |
| **`lunco-scripting`** | Language/runtime-neutral scripting host: document ownership, backend-neutral scenario lifecycle, scheduling, hot reload, teardown, lifecycle/document commands, and optional Python one-shot execution. Window audience detection is an opt-in application feature. The Rhai world runtime is isolated in `lunco-scripting-rhai-runtime`; this crate does not own product-level Rhai policy, tools, or timelines. |
| **`lunco-scripting-rhai-runtime`** | Production Rhai application boundary: command registration, tool/timeline persistence, `.rhai` asset dependency loading, and composition of the language-neutral host with `lunco-scripting-rhai-world`. It keeps application-facing integration separate from the high-churn world/policy closure. |
| **`lunco-scripting-rhai-world`** | Reusable Rhai world/runtime substrate: reflected world bridge, `RhaiScenarioRuntime`, authored application/Twin policy activation, policy status projection, and optional Twin-scoped native hook providers. It is usable by Rhai catalog/diagnostic consumers without pulling the application command/tool/timeline composition package. |
| **`lunco-scripting-rhai-core`** | Reusable Rhai backend substrate: asset-scoped module resolution, native vector/quaternion functions, task-tree lowering, typed UI request values, persisted-name validation, and the Rhai-to-`HookValue`/reflected-write boundary. It does not own JSON, the scenario host, or product-level policy. |
| **`lunco-scripting-rhai`** | Rhai authoring/query surface: catalog discovery, script diagnostics, and dataset queries. It is a production package installed by Rhai hosts, but is separate from the language-neutral scripting lifecycle so editor/query changes do not rebuild that core. |
| **`lunco-tools`** | Backend-agnostic, dependency-free tool trait + layered registry: a *tool* is a named, reusable bundle of callable functions whose implementation is pluggable (rhai/native/future). Owns the bevy-free `Tool` trait (discovery + `as_any` downcast), standard/core/application/Twin scope resolution, and discovery. Behaviour-tree execution lives in `lunco-tools-bevy`. |
| **`lunco-tools-rhai`** | rhai adapter binding for the `lunco-tools` registry: `RhaiTool` (source) + `NativeRhaiTool` (native Rust), and `bind_registered_tools`, which binds every registered tool into a rhai `Engine` as a static module callable as `name::fn(...)`. |
| **`lunco-tools-bevy`** | Bevy dispatch adapter for `lunco-tools` — the behaviour-tree execution half. Defines a bevy-aware `ExecutableTool` supertrait + `ClosureTool` (a closure that triggers its typed command directly via `&mut World`, no JSON/reflect). Observes `ToolFired`, downcasts to `ExecutableTool`, runs it. Instruments register via `register_closure_tool`. |
| **`lunco-hooks`** | Language-agnostic hook registry and shared typed `HookValue` ABI: a *hook* is a named, link-collected decision point with a reflected typed function signature and `HookValue` in/out; its implementation is pluggable. `HookValue` also carries in-process command acknowledgements. Backs first-class policies — journal **merge** order, RBAC **authorize** gate, and authored actuation policies — as data, not Rust branches. It also owns the bounded typed wire used by native providers and exact registration admission/teardown. |
| **`lunco-hooks-rhai`** | Rhai backend for `lunco-hooks`: compiles a Rhai `source` + `entry` function and registers it under a hook id (`register_rhai_hook`), so any declared installable hook can be authored in Rhai and hot-replaced. |
| **`lunco-hooks-plugin-api`** | Small edition-2024 native-provider ABI: stable descriptor/capability structs, callback status protocol, and typed wire helpers. It is the compile-time contract for shared-library providers. |
| **`lunco-hooks-native`** | Optional native-provider host: loads Twin-approved shared libraries, validates descriptors against reflected installable hooks, adapts callbacks to `ScriptHook`, and removes exact registrations at Twin teardown. |
| **`lunco-lint`** | Universal lint substrate: `LintFinding`/`LintReport` and `run_lint(domain, facts)`, which asks the `lint.<domain>` hook what is wrong with a domain's FACTS. Rules are authored (`assets/scripting/policy/lint_<domain>.rhai`), never compiled here — this crate names no domain and knows nothing about USD, Rhai, Modelica, or SysML. Nothing lints on load; `RunLint` and `ValidateAsset` in `lunco-scene-validation` are the two entry points. See `docs/architecture/lint-substrate.md`. |
| **`lunco-behavior`** | Dependency-free task-tree kernel (mechanism, no bevy/avian/rhai): `Ctx`-driven composites (`Sequence`/`Selector`/`Parallel`), reactive composites, loops, decorators, and timed/event leaves. `lunco-scripting-rhai-runtime` owns the Rhai-facing task programs. Node catalogue: [docs/behaviour-trees.md](./behaviour-trees.md). |

---

## 8. Applications
Primary entry points and simulation assembly targets.

| Crate | Binary | Responsibility |
| :--- | :--- | :--- |
| **`lunco-luncosim-exposures`** | — | Headless-safe runtime exposure projection plugin. Resolves authoritative ECS/domain state and authored telemetry into the shared `EngineExposures` registry for HTML, egui, API, telemetry, and remote consumers; it has no renderer or UI dependency. |
| **`lunco-luncosim`** | `luncosim` | Thin process/CLI shell and production authored-scene test command. It dispatches headless mode to `lunco-luncosim-runtime`, GUI mode to `lunco-luncosim-ui`, and the `rhai` subcommand to `lunco-rhai-repl`. |
| **`lunco-luncosim-presentation`** | — | Application-edge visual bridges: status/environment projection, terrain horizon, USD camera/light composition, capture integration, and scene presentation wiring. |
| **`lunco-updater`** | — | Native desktop update capability and rendered update surface. It owns Velopack admission and update UI behind the application’s opt-in `updates` feature. |
| **`lunco-scene-runner`** | — | Production headless runner for authored USD + Rhai scene and Twin verification checks. It owns deterministic stepping, readiness barriers, telemetry verdicts, and exit codes, keeping the GUI composition crate focused on startup and presentation. |
| **`lunco-luncosim-core`** | — | Dependency-light Bevy substrate shared by GUI, server, and scene-test hosts: raw input/state schedules, asset source/type registration, task-pool policy, build identity, and log deduplication. Bevy state/input features are explicit here; window-backed input focus belongs to the UI composition. It does not install domain plugins. |
| **`lunco-luncosim-simulation`** | — | Renderer-independent domain composition: world shell, physics, USD, terrain, celestial, Modelica/cosimulation, mobility, avatar, controller, hardware, telemetry, shared render-quality policy, scene commands, and headless execution. |
| **`lunco-luncosim-services`** | — | Production application services: startup Twin resolution, API/query registration, networking, journal projection, and persisted experiment artifacts. It is composed by the runtime boundary rather than embedded in the generic core. |
| **`lunco-luncosim-runtime`** | — | Production application composition: services plus Rhai plugin/policy projection, `SetRhaiPolicy`, scripting journal consumers, headless builders, and the headless launcher. |
| **`lunco-luncosim-server`** | `luncosim-server` | Thin headless launcher that depends on `lunco-luncosim-runtime` with API + networking enabled; the GUI shell is not linked. |
| **`lunco-rhai-repl`** | — | Terminal adapter for the reflected `RunRhai` command. It reads stdin/files and presents results while delegating evaluation to the running simulator and HTTP to `lunco-api-client`. |
| **`lunco-modelica-ui`** | `lunica` | The Modelica workbench application and UI facade. |
| **`lunco-modelica-icon-ui`** | — | Reusable egui Modelica icon/diagram graphics renderer used by the diagram canvas and model preview. |
| **`lunco-modelica-docs-ui`** | — | Reusable egui Modelica documentation renderer used by the model view. |
| **`lunco-modelica-index`** | — | Reusable Modelica index, diagram metadata, package-browser values, and editor-index artifact contract. |
| **`lunco-modelica-library`** | — | Source-library runtime admission, artifacts, browser handoff, and the shared Modelica worker bridge. |
| **`lunco-modelica-source-roots`** | — | Twin/workspace source-root inventory and demand-driven `LoadSourceRoot` admission. |
| **`lunco-modelica-runner`** | — | Modelica experiment scheduling and shared batch/interactive run backend. |
| **`lunco-modelica-worker`** | — | Headless Modelica worker engine and Bevy co-simulation bridge. It owns worker lifecycle internals, command dispatch, live stepping, and execution caches. |
| **`lunco-modelica-execution`** | `lunica_worker`, `modelica_run`, `modelica_tester` | Modelica execution host. It assembles the worker engine, owns native/wasm host transport, and keeps the shared command/result protocol in `lunco-modelica-runtime`; `lunco-modelica-runner` owns experiment scheduling and run orchestration. |
| **`lunco-modelica-assets`** | `build_modelica_library_assets`, `modelica_library_indexer`, `modelica_library_parse_bench` | Native Modelica source-library packaging and indexing tools. The indexer is shared by the CLI and the Modelica UI's background lifecycle adapter; the package remains independent of Bevy. |
| **`lunco-modelica-api`** | — | Transport-free Modelica API capability: query providers plus document edit and explicit-snapshot solve commands, installed by API-enabled Modelica and LunCoSim hosts. |

> Other binaries: `build_modelica_library_assets` (`lunco-modelica-assets`), `net_smoke` (`lunco-luncosim`), `dem_worker` (`lunco-terrain-bake`, the off-thread DEM bake Web Worker — staged next to the wasm by `build_web.sh`).

---

## Detailed Crate Responsibilities

Below, selected crates whose responsibilities benefit from extra detail. (Crates not listed here are adequately described by the tables above.)

### Core Foundation

**`lunco-core`**
The stable ECS contract layer. It defines identity/provenance, shared markers,
typed scene-lifecycle contracts, structured runtime faults/diagnostics, and
generic state utilities. Pure mutation envelopes are owned by
`lunco-command-contracts`; runtime command reflection remains available through
the existing typed command API. It has no pacing, reconciliation, exposure
registry, synchronization helper, port-registry, BigSpace, or celestial
semantics.

**`lunco-core-runtime`**
The Bevy runtime owner for `lunco-core` contracts. It installs the fixed simulation tick, rollback/netcode schedule anchors, pacing/barriers, gate instrumentation, subsystem toggles, and recoverable synchronization helpers. Contract-only consumers can depend on `lunco-core` without compiling this package.

**`lunco-geometry-core`**
Owns reusable f64 bounds/SAT and convex-profile mesh kernels. It depends only on
`bevy_math`; Rhai's authored geometry tools use it directly, while USD curve
and NURBS evaluators stay in `lunco-usd-geometry`. This keeps geometry edits
out of the broad `lunco-core` invalidation fan-out and prevents the scripting
packages from acquiring USD's Truck/NURBS dependency solely for extrusion.

**`lunco-exposure-core`**
The dependency-light typed exposure store. `EngineExposures` and its value
types are reusable by UI, API, scripting, telemetry, and recovery consumers;
the domain projection remains in `lunco-luncosim-exposures`.

**`lunco-port-core`**
Owns the shared scalar port substrate (`Port`, endpoint/control-surface components, `PortRegistry`, `PortInfo`, owner-supplied metadata, backend-owned topology keys, and the durable owner-published `PortTopologyRevision`/`PortTopologyState` structural invalidation pair) for software/hardware interaction. It is independent of `lunco-core`, so changes to engine-only core types do not rebuild the port implementation.

**`lunco-spatial`**
Owns the BigSpace-specific boundary: arbitrary-grid f64 pose composition/conversion, the persistent world shell, atomic grid migration, `ActivePhysicsFrame`, spatial markers, hierarchy invariants, and the vehicle-neutral navigation law. It depends on `lunco-core` for shared runtime diagnostics; the dependency direction is one-way, so changing spatial code does not rebuild core.

**`lunco-core-session`**
The session/authority layer above `lunco-core`. It owns network role and status,
session registries and profiles, possession/RBAC policy, prediction markers and
input watermarks, and the identity-admission systems that need the current
authority role. Hosts that need session behavior add `LunCoCoreSessionPlugin`
after `LunCoCoreRuntimePlugin`; headless consumers that only need core
primitives do not compile this policy layer.

**`lunco-time`**
The unified mission-time spine (architecture doc 19). Owns `MissionClock`/`TimeTransport`/`WorldTime` (the world animation clock that also gates physics via `Time<Virtual>`), the `TimeDomain` clock tree (`Playback`, `TimeBinding`, `ResolvedDomains`) with the `AnimationPreview` domain + `ControlAnimation` transport, and the `scales` projection layer (UTC↔TAI↔TT↔TDB, sidereal) over `celestial-time`. **All time-scale/JD nuance lives here; consumers delegate.**

**`lunco-doc`**
Foundation for structured, mutable artifacts (Modelica, USD, etc.) with built-in undo/redo logic. Defines the `DocumentHost` container and the atomic `DocumentOp` pattern for state mutation and inversion.

**`lunco-storage`**
I/O abstraction layer providing a unified `Storage` trait for reading, writing,
renaming, entry-kind inspection, directory listing/preparation, and backend
selection through handles.
Supports native FS and memory (for tests), with the browser localStorage
backend and architectural stubs for future OPFS/IndexedDB and remote backends.

**`lunco-assets-path`**
The dependency-free URI and relative-path algebra shared by USD composition,
document authoring, Twin resolution, and script imports. It owns canonical
scheme parsing, separator normalization, relative-path validation, and
traversal-safe path operations without depending on Bevy, storage, or a
filesystem.

**`lunco-assets-core`**
The lightweight asset identity boundary. It owns cache and Twin-root
resolution, traversal-safe source identity, and storage-facing cache/path
operations; it uses `lunco-assets-path` for platform-neutral URI rules. It
does not own source catalogs, discovery, scripting, text loaders, HTTP,
archives, raster, SVG, GeoTIFF, or native processing.

**`lunco-assets-runtime`**
The Bevy asset-source and authored-text runtime. It owns source registration,
discovery/catalogs, library/model/script/text loaders, web fetch integration,
and the asset-manifest tool while consuming identity and storage contracts from
`lunco-assets-core`.

**`lunco-assets-datasets`**
The lightweight dataset contract boundary. It owns `Assets.toml` declarations,
scoped dataset identity, artifact-path and integrity contracts, lifecycle state,
process-output ownership validation, and the Bevy registry and command events. It deliberately excludes network,
archive, image, GeoTIFF, and native processing dependencies so readers and
domain crates can inspect dataset state without linking the provisioning stack.

**`lunco-assets-transport`**
The small native byte-transfer boundary. It owns the shared
`DownloadSettings`-driven timeout/retry/resume mechanism used by asset and
other native HTTP consumers. It does not know what the bytes mean.

**`lunco-assets-download`**
The manifest-aware materialization layer. It verifies sources, extracts
archives, and atomically installs downloaded artifacts using the path/cache
contracts from `lunco-assets-datasets`. It does not depend on Bevy or image
processing.

**`lunco-assets-processing`**
The native baking layer. It owns heavy image/DEM/GeoTIFF/SVG/glTF processing,
cooperative cancellation, bake keys, staging, and atomic publication. New
heavy processors register a `ProcessorSpec` in the `ProcessorRegistry`; the
shared pipeline owns output identity and commit semantics.

**`lunco-assets`**
The explicit application provisioning boundary. It owns user-authorised Bevy
workers and the `lunco-assets` CLI, composing the dataset contract with the
transport, download, and processing layers. Applications add it only where
provisioning is part of the composition; asset readers use
`lunco-assets-core` or `lunco-assets-datasets` instead.

**`lunco-hash`**
The workspace hashing substrate. Fast tier provides dependency-free, wasm-clean FNV-1a hashing for process-local change detection and caching keys. CID tier (via the `cid` feature) provides CIDv1 raw + SHA-256 content-addressing for files/blobs on disk and wire.

**`lunco-precompute`**
Content-addressed precompute disk cache. Provides the `bake_or_load` mechanism to run expensive pure functions once, persist the artifact keyed by its content hash (using `lunco-hash` and `lunco-storage`), and transparently load it on subsequent runs and networked peers. Serves as the home for terrain derived layers, horizon bakes, collider/mesh bakes, and USD stage compositions.

**`lunco-theme`**
Centralized design tokens based on the Catppuccin palette. Provides semantic tokens for general UI (accent, success, error) and schematic-specific colors for diagram wires and badges, ensuring visual consistency across all panels.

**`lunco-settings`**
Centralised user-settings system. Persists one namespaced JSON file at `<OS config dir>/lunco/settings.json` with auto-save on change, giving subsystems a single place to read and write per-user preferences. It also owns the application-wide `DownloadSettings` policy: total attempts, exponential retry delay, and delay cap.

**`lunco-command-macro`**
Procedural macros for the typed command system. Provides the `#[Command]`, `#[on_command]`, and `register_commands!` macros used to simplify the creation and registration of simulation actions.

**`lunco-doc-bevy`**
Bevy ECS integration for the Document System. Provides lifecycle events (Opened, Changed, Saved, Closed), the `JournalResource` Bevy wrapper around the canonical Twin journal in `lunco-twin-journal`, and a `BevyJournalSink` for replaying remote-author entries through the same store. Lifecycle observers translate `EventOrigin` into `AuthorTag` and record `EntryKind::Lifecycle` entries directly into the canonical journal; structural ops are recorded by domain mutation paths.

**`lunco-twin-journal`**
Canonical, append-only, Twin-scoped record of every change. Immutable entries keyed by `(author, lamport)` (yrs-compatible), DAG parent links, optional `change_set` grouping for atomic undo, and `EntryKind::{Op, TextEdit, Snapshot, Lifecycle}`. Higher-level: `Stream` + `Composition`, `Branch`, `Marker`, and `UndoManager` with `UndoScope::{Document, Twin}`. Pure Rust, headless, no Bevy dep.

**`lunco-worker-transport`**
The generic Web Worker pool transport (wasm-only; `#![cfg(target_arch = "wasm32")]`). wasm32 has no OS threads, so multi-second companion work (a Modelica compile, a DEM decode + crater stamp) would freeze the page; each pool member is a JS `Worker` running a *second* wasm instance with its own linear memory. `WorkerPool` owns only the payload-agnostic plumbing — spawn / lazy-grow, the boot wire-id handshake (stale-worker guard), byte + Transferable-`ArrayBuffer` post, and crash respawn — driven by caller-supplied `Callbacks` (`on_message`/`on_ready`/`on_error`/`on_wire_mismatch`). Message framing, readiness gating, and result routing stay with the caller. `lunco-modelica-execution::worker_transport` composes it for the Fast-Run pool (source-library/run state on top); `lunco-terrain-bake::worker_client` composes it for the DEM bake — so the transport is written once and reused, not duplicated.

---

### Simulation Engine

**`lunco-celestial`**
Headless celestial semantics. Owns the canonical body catalog and named semantic reference frames, the typed f64 `FrameTree`, body-fixed rotation, geodesy, Kepler propagation, and the `EphemerisResource` abstraction. It has no scene hierarchy, BigSpace, terrain, rendering, or UI dependency. The concrete high-fidelity provider lives in `lunco-celestial-ephemeris`.

**`lunco-celestial-spatial-core`**
The lightweight ECS boundary for celestial spatial facts. It owns the semantic
frame-to-grid index, canonical site/body-fixed pose query, ENU surface-frame
helpers, scene body declarations, orbital-view state, the cached local gravity
fact, detached celestial Sun presentation state, authored mission declarations,
the solar-tracking marker, and the
published `LinkNode`, `LinkState`, `LinkGeometryState`, Wi-Fi, peer, and
occluder components consumed by cameras, avatars, networking, scripting,
telemetry, USD projection, and UI. It depends only on the semantic celestial
package, generic spatial coordinates, and the Bevy/BigSpace types required by
those contracts. It does not install a celestial runtime or pull terrain,
globe, link solving, imagery, trajectory sampling, cadence, or asset
integration. It also owns the render-independent trajectory view/frame/path
contracts consumed by both the spatial mission projector and the optional
trajectory presentation package.

**`lunco-celestial-spatial`**
Bevy/BigSpace runtime adapter for `lunco-celestial`. Owns scene hierarchy and grid projection, gravity derivation, surface placement, SOI migration, globe/imagery integration, detached-time globe and solar-disc projection, render-only body-fixed marker copies, links, cadence, and runtime commands. Physical stations and links remain on the causal `WorldTime` tree; only their marker geometry is copied beneath a detached presentation grid. Trajectory data contracts live in `lunco-celestial-spatial-core`; mesh sampling and trajectory alignment are installed by `lunco-celestial-presentation`. Consumers that need only shared frame or surface facts should depend on `lunco-celestial-spatial-core`; hosts that install celestial runtime behavior use this package.

**`lunco-celestial-presentation`**
Presentation adapter for celestial runtime facts. It owns trajectory sampling,
mesh/view construction, alignment, and visibility, while `lunco-celestial-spatial` remains the
headless-safe owner of scene projection, gravity, links, and commands. This
boundary prevents rendering-oriented changes from rebuilding the semantic
spatial package's consumers.

**`lunco-celestial-ephemeris`**
Concrete high-fidelity ephemeris provider for `lunco-celestial`. The heavy half of the celestial split and the one place `celestial-time` is allowed: pulls in `celestial-ephemeris` (VSOP2013 + ELP/MPP02), `celestial-time`, and `celestial-core` (none of which build on Windows MSVC). Apps that need real planetary positions add `EphemerisPlugin`, which overwrites the default `EphemerisResource`.

**`lunco-environment`**
Position-dependent environmental state (gravity, atmosphere, radiation, etc.). Uses a provider-consumer pattern to compute local conditions for each entity based on its proximity to celestial bodies and their specific environment models.

**`lunco-terrain-core`**
Projection-agnostic terrain LOD spine. Provides quadtree-CDLOD tile selection, tile-grid math, and the `HeightSource` trait. Pure (std + serde only) with no bevy/avian/DEM/sphere dependency, so it is shared by both the planar DEM streamer (`lunco-terrain-surface`) and the cube-sphere planetary tiler (`lunco-terrain-globe`).

**`lunco-terrain-globe`**
Whole-body cube-sphere terrain tiling at orbital/planetary scale: quadtree-CDLOD globe, avian heightfield collision ring, and `big_space` anchoring. The "globe" projection of the terrain family; pairs with `lunco-terrain-surface` (local DEM ground) over the shared `lunco-terrain-core` LOD spine.

**`lunco-terrain-surface`**
Local high-detail DEM ground terrain at surface scale: `TerrainSurfaceChange` carries the replaced/current oracle identity and dirty bounds through visual tiles, the collider ring, static collider rebuilds, and derived maps; the products retain independent resolution and cache contracts. Also owns `big_space` per-tile anchoring and the layered color pipeline. The "surface" projection pairs with `lunco-terrain-globe` over the shared `lunco-terrain-core` LOD spine.

**`lunco-terrain-bake`**
The pure (bevy/avian-free) DEM bake pipeline, factored out of `lunco-terrain-surface` so the SAME code runs on native and in a browser Web Worker: GeoTIFF decode → native crop → optional coarse-preview resample → crater stamp → `HeightGrid` (`bake_grid`/`finish_bake`), plus the serializable `DemBakeJob`/`StampSpec`. On native `lunco-terrain-surface` calls it inside an `AsyncComputeTaskPool` task; on wasm — where that pool runs on the page's main thread and the ~40 MB decode + crater stamp froze the tab — it dispatches to the `dem_worker` companion binary over `lunco-worker-transport`, which decodes once then streams a coarse preview (`COARSE_RES`) and then the full native grid back (coarse-then-full progressive). Only the avian collider + Bevy mesh derive stays in `lunco-terrain-surface`, where those types live.

**`lunco-obstacle-field`**
Procedural crater + rock field generation for rover testing. Produces LOD-aware obstacle distributions usable as mobility test grounds.

**`lunco-cosim`**
Multi-engine simulation orchestrator. Wires named outputs from one engine (e.g., Modelica) to named inputs of another (e.g., Avian physics) via `SimConnection` components, following FMI/SSP causality. Owns the built-in `PortRegistry` backends: rigid-body state (position/velocity/attitude/rates + force/torque/mass-props), revolute/prismatic joint motors (`angle`/`displacement`), and USD-authored sensors (IMU, range, contact), including their authoritative port metadata. Avian forces are applied through the typed-port spec table (`AvianGroup`/`AvianPort` + `PendingForces`), not a bespoke `AvianSim` struct.

**`lunco-cosim-core`**
Backend-neutral co-simulation contract package. It owns participant, connection,
diagnostic, typed-port, control-hold, force/torque actuator, and realtime-safety
contracts, shared connector constants, connection binding state, and generic
fixed-step schedule anchors. The generic control relationship is owned by `lunco-control-core`, so
co-simulation consumers can use it without making the control contract part of
the co-simulation package.

**`lunco-experiments`**
Backend-agnostic experiment / batch-run registry. Models a single Fast Run as a first-class artifact (params, bounds, trajectory), decoupled from any one solver via the `ExperimentRunner` trait that another crate plugs in. `RunStatus` is `Pending → Queued → Running { t_current } → Done { wall_time_ms } | Failed { error, partial } | Cancelled`; `RunBounds` carries start/stop/interval; parallel runs schedule across a worker pool.

### Vessel Control & Hardware

**`lunco-mobility`**
Physics models for surface mobility and traction — the parameterized substrate (a vehicle is a USD file, not a Rust struct). Raycast wheel model with contact-plane traction (supports leaning single-track bikes), suspension (spring-damper), generic authored drive/heading output realization, and a soft rocker-bogie `DifferentialCoupling`.

**`lunco-control-core`**
Generic control contract package. It owns `ControlLink`, the typed
`AcquireControl`/`ReleaseControlSource` relationship commands, the shared
`UserIntent` vocabulary, authored intent-to-port bindings, input state, egui
focus gating, and the bounded semantic-edge trace. It is the single contract
used by raw input translation, embodiment behavior, editor tools, API/Rhai
simulation, and vessel control; the generic engine substrate remains in
`lunco-core`.

**`lunco-interaction-core`**
Small cross-runtime cursor-interaction contract package. The editor publishes
`DragModeActive` and `GizmoDragging`; camera, possession, and follow runtimes
consume those contracts to stand down during a transform drag. Its
`ScenePointerPolicy` is projected from the registered
`LunCoPointerInteractionAPI`, while possession only claims a click whose shared
semantic intent is exclusively `selection.replace`. Keeping these types here
prevents the camera runtime from depending on the high-fan-out `lunco-core`
package and keeps editor implementation details out of consumers.

**`lunco-avatar`**
Headless-safe local-avatar runtime. Implements control authority, scene
interaction, and avatar control observers. A camera-bound possession emits a
typed camera subject-binding event; the camera realization owns the spatial
pose and mode transition. Avatar-specific semantic input projection lives in
`lunco-avatar-input`; generic camera mode exclusivity, the
one-writer easing rule, input policy, clip-plane math, and free-flight/surface
pose writers live in `lunco-camera-runtime`/`lunco-camera-core`; celestial
orbital placement and surface/orbit lifecycle live in `lunco-avatar-camera`;
optional egui presentation is supplied by `lunco-avatar-ui`. The viewport
reconciler in `lunco-usd-bevy-camera` owns which camera is shown.

**`lunco-embodiment-core`**
Backend-neutral embodiment role package. Its `roles` module owns the
`Embodiment`, `LocalEmbodiment`, and `RemoteEmbodiment` markers plus the derived
`TheLocalEmbodiment` lookup and its single-claim hooks. It also resolves an
explicit embodiment request against that authoritative local role. Product
policies and camera, control, notification, and scene adapters compose around
these roles.

**`lunco-notifications-core`**
Backend-neutral transient notification package. It owns the typed
`ShowNotification` command and `ScreenNotifications`/`Toast` queue contract.
The application runtime provides the command observer and optional UI adapters
render the queue.

**`lunco-avatar-camera-core`**
Avatar-specific camera transition contract package. Owns the BigSpace-backed
`OrbitViewReturn` snapshot, transient `OrbitViewHistory`, `OrbitUserInput`,
orbit arrival markers, and shared surface/orbit handoff constants. Generic
camera consumers use `lunco-camera-core` without acquiring these avatar-only
frame and restoration types.

**`lunco-avatar-input`**
Avatar-specific semantic input runtime. It converts the shared `Look`, `Zoom`,
`Pause`, and `Cancel` intents into camera input and avatar actions, including
OS scroll-unit normalization and egui/editor ownership gates. It is a
production runtime package, not a test-only boundary; possession and session
authority remain in `lunco-avatar`.

**`lunco-avatar-camera`**
Avatar-specific camera realization package. Its `AvatarCelestialCameraPlugin`
owns typed subject-binding and release restoration, `FollowTarget`, BigSpace
orbital placement, pending-focus realization, focus/return transactions,
interactive-camera initialization, vessel spring-arm follow, collision-aware
free-flight/surface locomotion, and the surface/orbit lifecycle handoff for
avatar entities. Generic celestial surface-frame publication remains in
`lunco-camera-celestial`, while control authority and scene interaction remain
in `lunco-avatar`.

**`lunco-avatar-policy`**
Owns the generic workspace-setting interpretation for avatar soil collision and
the runtime's measured collision shape. Both the movement runtime and avatar UI
read this package directly; it has no dependency on the avatar implementation.

**`lunco-camera-core`**
Backend-neutral camera contract package. Owns reusable camera behavior
components, shared smoothing defaults, pure frame/zoom/movement/clip-plane
math, camera-mode transition state, pose-input accumulators, the typed camera
commands (`FocusTarget`, `FollowTarget`, `ReturnFromOrbit`, and
`SetCameraInput`), the `PendingFocus` request, camera transaction diagnostics,
the deterministic authored-camera display-label projection, and the
`camera.default_presentation` hook identifier. It does not choose an
embodiment, a camera source, or an authored behavior policy.

**`lunco-camera-runtime`**
Generic interactive camera realization package. Owns the exclusive camera-mode
hooks, the one-writer interaction-easing rule, frame-handoff rebasing,
free-flight/surface pose writers, persisted `CameraInputSettings`, and the
`SetCameraInput` command over `lunco-camera-core`. It is reusable by avatar,
inspection, and other authored operators; source-specific frame production
remains with the calling runtime, while Rhai can author presentation policy
through the command surface.

**`lunco-camera-celestial`**
Celestial spatial adapter for the generic `SurfaceCameraFrame` contract and
adaptive perspective clip planes. It resolves body-fixed ENU bases and
celestial bounds from live BigSpace poses, keeping those conversions out of
generic camera and avatar interaction packages. Avatar-specific orbital
placement is owned by `lunco-avatar-camera`.

**`lunco-avatar-ui`**
Optional egui presentation adapter for `lunco-camera-core`,
`lunco-embodiment-core`, and `lunco-avatar-policy`. It owns the avatar status
panel, camera/name-tag and notification overlays, and the Avatar settings row.
The application UI shell adds it explicitly when avatar presentation is
enabled; headless avatar consumers do not compile it.

**`lunco-hardware`**
Physical actuator and sensor implementations. Bridges `Port` values to the `avian3d` physics engine, providing concrete motor, brake, and sensor components that interact with the simulation world.

**`lunco-input-core`**
Shared input settings and projection. It owns the persisted `InputBindingsSettings`
section, semantic labels, pointer-chord resolution, and Leafwing `InputMap`
construction. The application supplies the authored default document through
the runtime asset pipeline (`lunco.input-bindings.v1`); this contract crate has
no compiled-in asset path or product keymap. UI, avatar, controller, and Rhai
consumers use this focused contract directly.

**`lunco-input-ui`**
Optional egui adapter for the shared input state. It owns the input overlay
settings and rendering, but no raw-device translation or vessel actuation.

**`lunco-controller`**
Specialized vessel-control adapter. It converts semantic `UserIntent` actions
into authored `SetPorts` writes, applies authority/session rules, and provides
the generic window-input injection path. It consumes `lunco-input-core` rather
than owning a second keymap.

---

### USD Integration Layer

**`lunco-usd-document`**
Headless authored OpenUSD document/layer substrate: `UsdDocument`, authoring
state, typed operations, layer identity, and edit history. Reusable authored
data lives in `lunco-usd-data`, authoring and schema helpers in
`lunco-usd-authoring`, and `StageRecipe` in `lunco-usd-compose`. It has no
runtime projection, command observers, physics, rendering, or UI dependency.

**`lunco-usd-data`**
Reusable render-free authored USD data contracts: stage metadata, unit and
up-axis conversion, and composed-value readers. It is the lower boundary for
readers that need USD conventions without the document lifecycle or authoring
registry.

**`lunco-usd-authoring`**
OpenUSD authored-layer operations and schema registry. It owns path-addressed
authoring, USDA conversion, reference/list-op helpers, and registered schema
metadata without owning document identity, journaling, runtime composition, or
UI.

**`lunco-usd-core`**
Headless typed USD operation, assembly, edit-session, and edit-policy
substrate. It owns `ApplyUsdOp`/`ApplyUsdOps`, disposable
`ApplyUsdTransientOps`, and operation lowerings, while depending on the
document package for authored-layer state.

**`lunco-usd-commands`**
Headless USD document and authoring command boundary. It registers the USD
document kind, owns file/open/save and document-lifecycle observers, and lowers
typed USD document commands through the canonical journal path. Scene
admission, Twin-backed stage loading, and live document projection belong to
`lunco-usd-bevy-runtime-core`, so document-only hosts can use this package
without the complete visual/simulation bundle. Public query-provider
registration belongs to `lunco-usd-queries`.

**`lunco-usd-queries`**
UI-free public query providers for the USD document boundary. It owns
`InspectUsdDocument`, `InspectUsdEditSession`, `ResolveUsdTarget`, and
`SyncUsdDocument`, and its `UsdQueriesPlugin` registers them beside their
implementations. The providers read the authoritative document registry,
edit-session state, journal, and mounted stage without depending on runtime
orchestration or UI presentation. Their public query contracts are tested
through the production scene gates in
`assets/scenes/tests/usd_query_api.usda` and
`assets/scenarios/tests/usd_query_api.rhai`; proposal-query coverage is shared
with the assembly editor proposal gate.

**`lunco-usd-bevy-runtime-core`**
Headless-safe scene runtime boundary. Installs scene admission, Twin-backed
stage loading, live document-to-stage projection, generic authored info-change
publication, authored control/program projection, scene
commands, and the stage terminal-outcome contract. It does not assemble the
complete visual,
diagnostics, physics, simulation, or document-command bundle.

**`lunco-usd-bevy-runtime-persistence`**
Twin-scoped runtime-overlay persistence boundary. It installs the opt-in
document-open/document-change observers and restores or stores the generated
USD runtime layer through `lunco-storage`; it does not own scene projection or
UI policy.

**`lunco-usd-bevy-authored-runtime`**
Reusable authored-behavior adapter. It attaches generic control bindings and
`LunCoProgramAPI` runtime state after visual projection and exposes the same
program refresh operation to live USD consumers.

**`lunco-usd-bevy-scene-ports`**
Reusable Bevy scene-property port backend. It exposes connected light channels
and transform components as writable simulation sinks, owns the port-surface
lifecycle markers, and is installed explicitly by the aggregate USD runtime.

**`lunco-usd-bevy-runtime`**
Application-level composition boundary. Installs the visual USD projector,
visual diagnostics, Avian physics, USD simulation, document commands, and
`lunco-usd-bevy-runtime-core` as the standard runtime bundle. Hosts that need
authored Modelica/Rhai participants enable the package's `cosim` feature;
applications that need only scene admission and live projection can depend on
the core package.

**`lunco-usd-geometry`**
Render-free reusable geometry substrate for USD projections: USD BasisCurves
evaluation, NURBS curves and patches, trimmed-domain tessellation, and
rotation-minimizing curve sweeps. Its heavy numeric dependencies are isolated
from stage and camera policy so evaluator changes do not rebuild unrelated USD
runtime code.

**`lunco-usd-bevy-stage`**
Renderer-independent composed-USD stage boundary. It owns the
`UsdRead`/`StageView` contract, resolver-backed composition, `UsdStageAsset`
loading, canonical stage ownership, authored-layer readers, instance identity,
projection plans, standard material/purpose/variant readers, transform and unit
decoding, authoring helpers, and the stage integration tests. It deliberately
contains no runtime projection systems.

**`lunco-usd-bevy-core`**
Focused runtime projection mechanisms built on `lunco-usd-bevy-stage`. It owns
authored animation, live edits, mount coordination, point instancers, and
executable program runtime. Consumers import stage facts from the stage crate
and runtime mechanisms from this crate; the boundary has no compatibility
re-export layer.

**`lunco-usd-bevy-scene`**
Render-free ECS contract between USD projection domains. It owns `UsdPrimPath`,
`UsdSceneProjected`, `UsdSceneRoot`, `UsdPreviewOnly`, `UsdAnimated`, the
projection ordering boundaries, the generic `UsdSceneProjectionReset` and
`UsdSceneInfoChanged` messages,
visual-split markers, and the
stage revision/ancestry helpers, authored billboard contracts, plus the shared
USD primitive and indexed-mesh readers and the composed collision/placement envelope readers in
`lunco_usd_bevy_scene::collision` (`collision_aabb`, `prim_geometry_aabb`, and
`ObjectAabb`). Avian, terrain, and other headless projections depend on this
package without depending on the visual mesh/camera/light adapter; the visual
crate uses the same contract when it binds presentation components.

The package's `UsdScenePlugin` installs the render-free stage revision and
failed-mount lifecycle systems. `FailedSceneLoad` is consumed by scene
transactions, so headless simulation does not depend on visual diagnostics.

**`lunco-usd-bevy-twin`**
Render-free Twin/document projection state shared by headless runtime,
authoring, terrain, validation, and UI adapters. It owns the
`DocBackedTwinScenes` lease map, the `twin://` stage-to-document resolver,
`UsdDocumentUserOwned`, `LiveRebuildExempt`, and the event-driven
`TwinProjectionWake` signal. It depends on the stage-asset identity and asset
path contracts, but it does not load or compose USD stages and does not depend
on the command/runtime boundary.

**`lunco-usd-ui`**
Interactive USD browser and document presentation. Owns workbench sections, loaded-stage and scene-file views, browser dispatch, Save-As picker integration, and UI status/placeholder adapters while consuming the document and projection APIs from `lunco-usd-commands`. Add `lunco-usd-viewport-ui` when an application needs the render-heavy preview surface.

**`lunco-viewport-core`**
Small renderer-independent viewport geometry contract. It owns the physical-pixel
`PanelRect` value used to pass measured panel bounds across render, scene, and
editor packages without coupling those contracts to egui or the Workbench shell.

**`lunco-usd-viewport-core`**
Render-independent USD preview contracts. It owns preview/session/view
identities, document-backed projection state, camera pose math, preview
commands, inspection settings, selection resolution, and typed measured-
viewport/click/orbit events. It also owns the shared drag policy, including
primary/middle pan, secondary orbit, Shift+secondary pan, and gizmo capture.
It does not own offscreen images, egui textures, render cameras, or viewport
panels, so USD editors and headless adapters can consume the state without
linking the render-heavy surface.

**`lunco-usd-viewport-ui`**
Workbench presentation adapter for the USD preview. It owns the egui viewport
panels and translates panel geometry/pointer gestures into the typed runtime
events. It installs the presentation-only `InspectUsdViewport` and
`InspectUsdInspectionPresets` query providers. Preview/session state, offscreen
images, render cameras/lights, projection binding, and typed commands come from
`lunco-usd-viewport-runtime`; this package does not own Twin-browser lifecycle
or document navigation.

**`lunco-usd-viewport-runtime`**
Render runtime for document-backed USD preview sessions. It owns preview
lifecycle, offscreen images and egui texture registration, render cameras and
lights, projection readiness, render budgets, preview commands, measurements,
and pointer-event handling. Presentation query providers live in
`lunco-usd-viewport-ui`; the runtime does not register API queries or
workbench panels.

**`lunco-usd-bevy-camera`**
Render-free camera adapter built on `lunco-usd-bevy-stage` and
`lunco-usd-bevy-core`,
`lunco-usd-bevy-scene`, and the shared curve evaluator in
`lunco-usd-geometry`. It maps standard USD `def Camera` prims to camera intent,
handles rover-mounted and cinematic camera poses, and owns camera selection plus
the single-authority viewport reconciler. It contains no visual projection and
does not own BasisCurves geometry math.

**`lunco-usd-bevy`**
Visual OpenUSD bridge built on `lunco-usd-bevy-stage`,
`lunco-usd-bevy-core`, and
`lunco-usd-bevy-mesh`. It maps USD prim hierarchies and visual facts into Bevy
entities/components and orchestrates async mesh and render-intent projection.
Authored controls and generic executable programs belong to
`lunco-usd-bevy-runtime-core`; scene-property port surfaces belong to
`lunco-usd-bevy-scene-ports`, not this visual adapter. The
visual plugin installs the camera and light adapters at its integration boundary;
`lunco-render-bevy` supplies the concrete render pipeline. Parametric
NURBS/lathe definitions and their mesh regeneration live in the independent
`lunco-usd-bevy-lathe` package, which this crate uses directly rather than
re-exporting. Add `lunco-usd-bevy-animation` when authored `timeSamples`
playback is required. See
[`17-view-and-intent.md §6`](architecture/17-view-and-intent.md).
Headless consumers import stage facts and runtime mechanisms from their owning
packages directly; this visual adapter is not a compatibility facade for the
headless API.

**`lunco-usd-bevy-animation`**
Render-free USD animation adapter built on `lunco-usd-bevy-stage` and
`lunco-usd-bevy-core` and
`lunco-usd-bevy-scene`. It owns time-domain binding, animation topology plans,
and per-frame sampling of authored transform, visibility, and material intent.
It is installed separately from the visual adapter so headless/document
consumers do not compile animation systems unless they need them.

**`lunco-usd-bevy-light`**
Production UsdLux adapter for `DistantLight`, `DomeLight`, `SphereLight`, and
`RectLight`. It owns the authored-light marker, ambient-dome aggregation, the
CPU HDRI projection used by skybox/environment-map components, and live dome
refresh after consuming the generic scene info-change boundary. The package
depends on the composed USD reader and render intent, but not on the visual
hierarchy/mesh projector, so light-reader changes do not rebuild that package.

**`lunco-usd-bevy-diagnostics`**
Optional visual USD asset diagnostics installed by `lunco-usd-bevy-runtime`. It owns
glTF placeholder hiding/replacement and the CPU-baked labels on visual failure
stubs. The visual projector only emits generic scene markers from
`lunco-usd-bevy-scene`; stage-load failure state itself remains in that
render-free package, so headless scene transactions do not pull in this crate.

**`lunco-usd-bevy-lathe`**
Production parametric NURBS/lathe projection. It retains the authored surface
definition as reflected ECS components and regenerates the Bevy mesh only when
the definition or graphics quality changes. Its geometry and tests are isolated
from the USD hierarchy loader, so a lathe edit does not recompile the main
visual projection package. It depends on the render-free geometry and intent
packages and has no dependency back into `lunco-usd-bevy`.

**`lunco-usd-bevy-mesh`**
Render-free visual geometry projection for USD built-in primitives, native
`UsdGeomMesh`, `BasisCurves`/`NurbsCurves`, and `NurbsPatch`. It owns mesh
tessellation, quality invalidation, and the low-level geometry tests. The
hierarchy loader retains async projection orchestration and material intent but
does not depend directly on the heavy geometry evaluator stack.

**`lunco-usd-avian-core`**
Core Avian/BigSpace physics-frame bridge. Owns f64 pose synchronization,
rootless collider propagation, active-frame transport/reset, backend admission
validation, and `BridgeShadow`; it does not read USD stages or contain UI
policy. Its bridge tests live with this production package so changing the USD
reader does not rebuild the bridge implementation.

**`lunco-usd-avian-contracts`**
Shared ECS carriers crossing the USD physics, vehicle, readiness, query, and
co-simulation packages. It owns `PendingUsdJoint`, joint-drive data, scene
ownership, dynamic-admission, authored-velocity, and physics-projection
lifecycle markers. Its generic invalidation seam lets the live runtime re-arm a
newly composed rigid-body prim without depending on the full USD physics
projector; that projector remains the owner of stage traversal and
authored-fact translation.

**`lunco-usd-avian-joints`**
Reusable native Avian joint boundary. It owns typed joint construction,
joint-pair filtering, initial seating, solver-island admission, and graph-safe
detach for authored USD and synthesized mechanisms.

**`lunco-usd-avian`**
Physics projection for OpenUSD (`UsdAvianPlugin`). Maps `UsdPhysics` schemas —
rigid bodies + mass-properties, all collider shapes, and **all joints**
(revolute/prismatic/fixed/spherical/distance, D6-reduced) with
`UsdPhysicsDriveAPI` motor drive — to normalized Avian joint plans and
body/collider components. Native joint construction and lifecycle live in
`lunco-usd-avian-joints`, including the programmatic wheel hinge. It consumes
the separate Avian/BigSpace core bridge. Runtime-only; its Rust tests cover
low-level mechanics with in-memory USDA fixtures, while shipped asset/runtime
assertions are owned by the Rhai scene-test gate. Lint fact extraction is
isolated in `lunco-usd-avian-reader` and `lunco-usd-avian-lint`. USD physics-material decoding is kept in a
separate internal module so changes to surface mapping do not enlarge the root
projection module.

**`lunco-usd-avian-reader`**
Shared composed-USD physics readers used by runtime projection and authored
lint. It converts standard collider schemas, joint schemas, drives, and typed
physics attributes into generic Avian-facing values. It installs no systems,
creates no entities, and owns no runtime or lint policy.

**`lunco-usd-avian-lint`**
Render-free composed-`UsdPhysics` fact producer for the authored Rhai lint
policy. It reuses the authoritative geometry and joint readers from
`lunco-usd-avian-reader`, but keeps lint dependencies and lint-only tests out of
the runtime physics package.

**`lunco-usd-sim`**
Specialized vehicle metadata bridge. Intercepts complex industry-standard vehicle schemas (like NVIDIA PhysX Vehicles) and substitutes them with optimized LunCo simulation models (e.g., Raycast wheels). Its `UsdSimPlugin` is independent from shader intent and the USD cosim translator; the application runtime installs those independent projectors explicitly. Direct USD physics lowering remains in `lunco-usd-avian`.

**`lunco-usd-sim-authoring`**
Render-free composed readers for the standard PhysX vehicle wheel-attachment
topology and gear-drive values. It is shared by `lunco-usd-sim` and
`lunco-scene-validation`, and publishes the typed facts consumed by the USD
Rhai lint policy. Runtime ECS resynchronization remains in `lunco-usd-sim`,
registered through the generic live-edit owner in `lunco-usd-bevy-core`.

**`lunco-usd-sim-core`**
Small production contract package shared by the vehicle and USD cosim
projectors and scene readiness. It owns `UsdSimSet` (including the shader
projection-preparation boundary), `UsdSimProcessed`,
`PendingDifferential`, and `GroundColliderPending`, keeping shared contracts
out of either large implementation crate and allowing scene runners to avoid
the full vehicle projector.

**`lunco-usd-sim-cosim`**
USD-to-cosim translator. `UsdSimCosimPlugin` installs source discovery,
wiring, readiness, telemetry projection, and the Modelica/script participant
exchange independently from vehicle realization. Generic scene commands and
mount/teardown mechanics live in `lunco-usd-bevy-runtime-core`; `sync` owns
the fixed-step port exchange and authored event projection. Its optional API
query providers live in `lunco-usd-sim-cosim-api`.

**`lunco-usd-sim-cosim-api`**
Optional API query providers for the cosimulation runtime: uniform ports,
causal traces, cosimulation status, binding status, scene-camera audits, and
broken-connection diagnostics. API-enabled composition installs this package
alongside the runtime; default cosimulation hosts do not inherit its direct
`lunco-api`/JSON serialization edge.

**`lunco-usd-sim-domain`**
Render-free USD domain projection. It reads composed component-network facts,
resolves Modelica member classes, invokes authored Rhai synthesizers, and
publishes generated Modelica sources. Generic force/torque actuator lowering
belongs to `lunco-usd-actuation`, while USD wiring and participant lifecycle belong
to `lunco-usd-sim-cosim`. Optional generated-source API queries are provided by
the separate `lunco-usd-sim-domain-api` package.

**`lunco-usd-sim-domain-api`**
Optional API query providers for the generated Modelica source projection. The
package is installed only by API-enabled runtime composition, keeping the
render-free domain projector's direct dependency set independent from
`lunco-api` and `serde_json`.

**`lunco-usd-sim-celestial`**
Independent render-free projection of USD-authored celestial and connectivity facts. It converts anchors, orbits, link nodes, occluder extents, and reflected-light declarations to `lunco-celestial` components. Its separate package boundary prevents celestial authoring changes from rebuilding the vehicle and cosimulation projector.

**`lunco-usd-sim-shader`**
Independent render-free `UsdShade` material-intent projection. Its
`UsdShaderPlugin` owns shader resolution invalidation, shader-port registration,
and the `ShaderLook` projection; `lunco-render-bevy` remains the only material
binder. The application runtime installs it beside `UsdSimPlugin`, so shader
authoring changes rebuild this leaf without rebuilding the vehicle projector.

**`lunco-usd-sim-telemetry`**
Independent render-free recorder for post-step Avian rigid-body and wheel state. It owns the transient telemetry cursor and publishes through the shared signal registry; telemetry implementation changes are isolated from USD schema projection.

**`lunco-materials`**
Custom shader appearance **intent** — **render-free**. Holds `ShaderLook` (a `.wgsl` path + an open `dyn_params` map + named texture layers), the WGSL-reflected `ParamSchema` (parameter names/ranges/defaults are parsed from each shader's own `struct Material` — **none are hardcoded in Rust**, so adding a parameter is editing a shader), and the CDLOD geomorph vertex attribute. It names **no** material type and no render pipeline, so a domain crate may depend on it without linking `bevy_render`. Generic procedural-background intent lives in `lunco-render`; the concrete `ShaderMaterial` described here lives in `lunco-render-bevy`. See [architecture/shader-layers-and-params.md](architecture/shader-layers-and-params.md).

---

### Networking & API

**`lunco-networking`**
Multiplayer transport adapter. Handles ECS replication, transport abstraction (UDP/WebSockets), and collaborative editing. Physics snapshots and camera/perspective state transfer f64 named-frame state; capture/apply automatically convert between each peer's private `ActivePhysicsFrame` and the semantic frame. No `CellCoord` is a public/wire reference-frame identity.

Client prediction, rollback, interpolation, and prediction session resources
live in the transport-independent `lunco-networking-core` package. The
WebTransport adapter feeds its snapshot inbox and owns lightyear/server
connection setup, so transport changes do not rebuild the prediction package.

Its optional `layout-sync` feature carries `WorkbenchSnapshot` and perspective
command payloads through `lunco-workbench-core` only; it does not depend on the
concrete egui docking shell. The `ui` feature enables that contract layer for
the in-sim overlays and menu bridge.

**`lunco-networking-core`**
Transport-independent client netcode. Owns snapshot interpolation, local
ownership prediction, rollback/reconciliation, correction smoothing, and the
resources exchanged with the replication wire. It depends on the simulation
and Avian contracts needed to execute prediction, but not on a network
transport; `lunco-networking` supplies incoming snapshots and composes it for
networked applications.

**`lunco-api`**
Transport-free API core. Owns typed command/query contracts, reflection-based discovery and execution, and the process-local entity registry used by external control and inspection. Native HTTP and browser transports live in `lunco-api-transport`.

**`lunco-api-transport`**
Application-bound API transports. Owns the native Axum HTTP listener, asset endpoint, and wasm browser bridge while consuming the transport-free `lunco-api` contracts.

**`lunco-telemetry`**
Reflection-based data extraction engine. Automatically samples and standardizes internal physics and software values for broadcast to external monitoring systems or Mission Control bridges (YAMCS/XTCE).

**`lunco-telemetry-core`**
Shared typed telemetry contracts and the generic event projection/logging boundary used by
producers, API/status consumers, scripting, and the sampling engine.

---

### Workbench & UI Tools

**`lunco-workbench`**
The engineering-IDE shell. It renders the concrete egui/bevy docking and
shell menus while consuming layout state and perspective materialization from
`lunco-workbench-layout`. File workflow and generic source editing are composed from
`lunco-workbench-file-ops` and `lunco-workbench-text-editor`. Shared
hierarchy-row and text/icon presentation lives in `lunco-workbench-widgets`;
shell-neutral tab, source-view, scene-state, and pending-close contracts live
in `lunco-workbench-core`. It does not own file bytes or backend I/O; those go
through `lunco-storage`, while Twin discovery stays in
`lunco-workspace`/`lunco-twin`. GPU health and presentation recovery live in
the independent `lunco-render-recovery` crate; the workbench only composes its
banner and recovery systems. Hosts that need Twin and Files navigation add the
separate `lunco-workbench-browser` feature package, which does not link this
shell. The shared input keymap is owned by `lunco-input-core`; the recording
overlay is supplied by `lunco-input-ui`, so the shell does not depend on the
full vessel-control adapter.

**`lunco-workbench-perf-ui`**
Reusable performance capability. It owns the persisted HUD preference, typed
toggle command, Bevy frame-time diagnostics, and live `PerfStats`; the
physics-aware editor bridge only publishes optional step timing. The concrete
workbench imports this package for rendering, so status presentation changes
do not make the shell the owner of diagnostics state.

**`lunco-workbench-help-ui`**
Rendered Help/About presentation for the Workbench. It owns the help registry's
egui menu item and version/source view and consumes `BuildIdentity` from
`lunco-workbench-core`; the core contract remains usable by headless hosts.

**`lunco-workbench-layout`**
Renderer-independent workbench layout owner. It materializes perspective
presets, maintains per-perspective dock state, sanitizes persisted split
fractions, and synchronizes scene interaction mode. The concrete shell consumes
this package, so layout changes do not rebuild menu and render code.

**`lunco-workbench-file-ops`**
Windowed file-workflow adapters. It owns picker-triggering seams and routes
resolved picker handles into the existing document and workspace commands.
Actual reads, writes, entry inspection, and renames use `lunco-storage`;
folder/Twin admission remains in `lunco-workspace`; domain crates still own
format-specific loading and serialization.

**`lunco-workbench-text-editor`**
Generic source-editor capability. It owns the source tab state, async source
read/write lifecycle, Twin-close cleanup, and the multi-instance panel while
reusing source commands from `lunco-workbench-core` and the editor builder from
`lunco-workbench-widgets`. Rich Modelica/USD editors remain domain-owned.

**`lunco-workbench-guided-ui`**
Optional application presentation for authored guided scenarios. It owns the
Rhai-facing HUD, spotlight, coach-mark, and recovery surfaces and is installed
explicitly by a host after `lunco-workbench`. The generic `HelpAnchors` and
`ViewportPlaceholder` resources remain in `lunco-workbench-core`, so the shell
and domain panels can publish/read presentation facts without depending on a
tutorial implementation.

**`lunco-workbench-file-dialog`**
The production file-dialog capability used by the workbench and domain UI.
It owns native `rfd` and browser dialog/download adapters plus the typed
picker request/result events. `lunco-storage` remains an I/O abstraction and
does not acquire dialog or platform-window dependencies.

**`lunco-workbench-runtime-ui`**
Reusable HUI/Flair surface presentation. It owns the generic manifest asset
loader, retained tree lifecycle, authored placement and styling bridge, input
regions, render-readiness acknowledgement, and open semantic action transport.
The `lunco-luncosim-ui` host supplies capture mode and named gates and handles
application actions; the runtime UI crate does not name cameras, terrain, or
models. Runtime surface assets remain under `assets/ui/` and are loaded through
Bevy's asset system rather than embedded in Rust tests.

**`lunco-workbench-window`**
Reusable OS-window capability. It owns typed window commands, merged-titlebar
construction, settings-backed geometry persistence, and explicit native window
placement. Application binaries use this package directly when constructing
their primary window; the concrete Workbench composes its command and
persistence plugins but does not re-export their APIs.

**`lunco-workbench-state`**
Reusable per-Twin session-state capability. It owns the persisted document,
dock, and runtime-surface schemas, domain codec registry, storage-backed
load/save lifecycle, and the layout-provider contract. The concrete Workbench
implements that contract for `egui_dock`; state consumers import these APIs
directly rather than through the shell.

**`lunco-workbench-browser`**
Reusable navigation feature for a rendered host. It owns the Twin and Files
panels, browser query/actions/resources, built-in filesystem and library
sections, while consuming shell-neutral panel, source-view, scene-state, and
rename command contracts from their owning crates. Domain UI crates register
their own `BrowserSection` implementations;
optional dataset controls live in `lunco-workbench-datasets-ui`, which depends
only on the lightweight `lunco-assets-datasets` contract; the native
asset-provisioning runtime is composed separately by the application. This keeps
the base workbench shell and the dataset browser independent of HTTP, archive,
image, and processing dependencies.

**`lunco-capture`**
Render-bound application capability for screenshots and deterministic offline
recording. It owns the typed capture commands, GPU readback, frame pacing, and
PNG/video sinks. The workbench installs it for windowed API hosts; the
offscreen application host installs the same capability without the workbench
shell.

**`lunco-status-core`**
Renderer-independent status and lifecycle substrate. It owns the shared
`StatusBus`, scoped busy/progress handles, tracked async tasks, and telemetry
mirroring. The concrete workbench status bar and `lunco-ui` busy widgets are
consumers; headless hosts can publish and inspect the same status without
linking the shell.

**`lunco-ui`**
Reusable UI infrastructure. Provides the `WidgetSystem` for cached ECS widgets, support for typed commands, and `WorldPanel` for 3D in-scene UI elements attached to entities.

**`lunco-viz`**
Domain-agnostic visualization framework. Collects simulation data into a `SignalRegistry` and renders it via `Visualization` kinds (LinePlots, Gauges) into various view targets like 2D panels or the 3D viewport.

**`lunco-canvas`**
2D scene editor substrate. Provides the stateful viewport and tool foundation for diagramming and node-based editing, powering the Modelica diagram editor and other schematic-based tools.

**`lunco-luncosim-edit-core`**
Headless-safe scene-editing mechanisms. Implements spawn and terrain-tool state,
scene picking, editor ECS state, and typed command registration. It has no egui,
workbench, transform-gizmo, or immediate-mode diagnostic dependency. Script-
authored click-tool dispatch is UI-owned because it consumes pointer events and
the UI tool library.

**`lunco-luncosim-edit-ui`**
Rendered scene-editing presentation. Implements the egui/workbench panels,
selection and USD preview interaction, and physics diagnostic visualization. It
depends on `lunco-luncosim-edit-core` and composes the focused
`lunco-luncosim-edit-gizmo-ui` package; the core package does not depend back on
either UI package.

**`lunco-luncosim-edit-gizmo-ui`**
Focused transform-gizmo capability. It owns the `transform-gizmo-bevy`
frontend, render-space proxy lifecycle, camera binding, and live/preview pose
transaction conversion into the existing scene/USD command owners. It has no
panel registration or scene-selection policy, so changes to editor panels do
not recompile the gizmo adapter.

**`lunco-luncosim-edit-inspector-ui`**
Domain-heavy Inspector and authored USD panels. It owns USD parameter, variant,
mount, joint, and animation view models plus environment and component
authoring surfaces. It consumes shared selection and viewport contracts without
depending on the broader scene-editing interaction package.

**`lunco-luncosim-edit-inspector-core`**
Renderer-independent Inspector readout capability. It owns the bounded ECS
queries for sun, camera, ambient, and joint facts plus the value-based change
gate that avoids rescanning a quiescent scene. The egui Inspector consumes its
`InspectorView`; it does not perform these world scans while painting.

**`lunco-usd-prim-tree-ui`**
Reusable composed-USD prim hierarchy panel and change-driven view model. It
owns no editor interaction implementation and can be installed by any
workbench host that provides the shared viewport and selection contracts.

**`lunco-render`**
Appearance **intent** and persisted Graphics quality policy — **render-free**. The vocabulary a domain crate uses to say what a thing should look like without naming a renderer: `PbrLook` (a plain surface as data — colour, roughness, metallic, emissive, alpha mode, texture channels), `ProceduralSkybox`, `ScreenConstantMarker` and its view visibility gate, `SceneCamera`, `WorldLabel`, the sun/shadow look settings, and `RenderingQualitySettings` for shared camera, light, sky, terrain, shadow, and tessellation budgets. It names `Mesh3d` but **never `MeshMaterial3d`** — that one line is the whole rule.

**`lunco-render-bevy`**
The **only** crate that names `bevy_pbr`. Binds the intent above to real Bevy materials: `PbrLook` → `StandardMaterial`, `ShaderLook` → `ShaderMaterial` (the one general self-describing `AsBindGroup`, any `.wgsl` per-instance), plus `SceneCamera` → camera bundle, `WorldLabel` → billboard text, environment light and horizon shading. Headless simply never adds this plugin — which is why `--no-ui` links **no wgpu, no `bevy_render`, no `bevy_pbr`, no egui, no winit**. See [architecture/render-decoupling.md](architecture/render-decoupling.md).

**`lunco-render-recovery`**
Render-bound resilience for the presentation boundary. It installs the
uncaptured-error handler, records adapter capabilities and shadow admission
facts, escalates repeated failures to a terminal presentation gate, and resets
that state at the explicit scene-teardown boundary. It has no workbench layout,
panel, Twin, or domain-policy ownership.

**`lunco-web`**
Shared web frontend for the wasm apps. Provides the streaming loader (`web/lunco-boot.{js,css}`), `WebReadyPlugin`, which signals the HTML loader once Bevy paints its first frame, and `mountRhaiTool`, which mounts trusted HTML/CSS tool bundles whose actions execute through the existing Rhai bridge.

---

### Scripting & Modeling

**`lunco-modelica-compiler`**
Headless Rumoca compiler and source-admission host. It owns the production
`ModelicaCompiler` session, source-root seating, strict reachable-DAE calls,
located compile diagnostics, and source admission revisions. Workers, runners,
asset tooling, command-line hosts, and the compiler's own tests all call this
same package directly.

**`lunco-modelica-core`**
Modelica document/runtime host. It consumes the headless `ModelicaDocument`
contract from `lunco-modelica-document`, owns Bevy lifecycle synchronization and
engine resources, and composes the compiler and source-library capabilities. It
does not own Twin/workspace source-root admission, Rumoca compilation, solver workers, Fast Runs,
prepared solve caches, or browser transport. API commands are opt-in, and API
query providers are owned by `lunco-modelica-api`.

**`lunco-modelica-source-roots`**
Host-side source-root admission. It inventories authored Modelica assets, resolves
Twin manifest paths, registers roots contributed by opened documents, and queues
demand-driven `LoadSourceRoot` operations. This lifecycle policy is separate from
the document/runtime core so changing Twin discovery does not rebuild its engine
implementation.

**`lunco-modelica-library`**
Production source-library capability shared by compiler, execution, and UI
hosts. It owns the persisted local-root setting, native parsed-bundle admission,
wasm manifest/bundle fetch, bounded decode and source unpack, editor-index
handoff, and the typed worker callback seam. It has no compiler session or
solver lifecycle, so changes to library transport do not rebuild the compiler
implementation.

**`lunco-modelica-execution`**
Modelica execution host. It assembles the stateful worker engine, native worker
launch, and the wasm `lunica_worker` transport. It composes the generic
`lunco-worker-transport::WorkerPool` and installs the worker callbacks consumed
by `lunco-modelica-runner`; source-library readiness and per-run routing remain
Modelica-specific here. Compiler-only consumers do not inherit this host.

**`lunco-modelica-worker`**
Stateful Modelica worker engine. It owns live solver construction and stepping,
command dispatch, native worker scheduling, worker-local compiled/prepared
artifacts, and the Bevy response/clock bridge. Keeping this package separate
from the host transport lets worker changes and browser transport changes
rebuild as independent production packages while both native and wasm paths
use the same dispatch implementation.

**`lunco-modelica-runner`**
Modelica experiment backend. It owns `ModelicaRunner`, source snapshots,
compile-once DAE caching, native scheduling, shared batch/interactive run
paths, run-bound resolution, and experiment-side Bevy resources. The runner
depends on compiler and solver contracts but not on the worker transport; the
execution host installs the typed wasm dispatch callbacks during composition.

**`lunco-modelica-telemetry`**
Render-free execution-side telemetry projection. It retains the current
variables of landed `ModelicaModel` sessions in the shared `SignalRegistry`,
uses `TelemetrySettings` for rate, retention, and channel limits, and attaches
Modelica signal metadata from the authored document and generated layout. The
execution host installs its plugin after worker responses; compiler/document
hosts do not compile this projection.

**`lunco-modelica-document`**
Headless, render-free Modelica document package. It owns the canonical source
buffer, recovering AST/index cache, typed document operations, source patching,
document-origin/file-backed contract, and low-level document tests. It has no
Bevy, compiler worker, UI, or simulation lifecycle; hosts install its
`ModelicaDocument` through the generic `lunco-doc-bevy::DocumentRegistry`.

**`lunco-modelica-solver`**
Renderer-free Rumoca solver capability. It owns backend registration, solver-option translation, adaptive live sessions, and the deterministic fixed-step session; the execution host supplies lowered solve models and owns worker lifecycle. This keeps solver-specific numerical dependencies and low-level integration tests out of the compiler host's source boundary.

**`lunco-modelica-api`**
Production API capability for Modelica hosts. It registers the domain-owned query providers for bundled sources, source-library classes, compile/run status, experiment results, document source, model metadata, variables, and share links, together with the transport-free document edit commands. `lunco-workspace-api` remains the owner of Workspace queries. Keeping the edit/query surface outside `lunco-modelica-core` keeps the compiler host focused on document and compile mechanisms; UI/server roots install the required API capability explicitly.

**`lunco-scripting`**
Language/runtime-neutral scripting host. It owns document lifecycle, backend-neutral scenario execution, scheduling, hot reload, pause, teardown, lifecycle/document commands, and the optional Python one-shot backend. Window audience detection is opt-in (`window-audience`) so headless builds remain window-free. It does not own the Rhai interpreter, world bridge, Rhai commands, policies, tools, timelines, or Rhai source assets. Those belong to `lunco-scripting-rhai-world` and `lunco-scripting-rhai-runtime`.

**`lunco-scripting-rhai-runtime`**
Production Rhai application boundary. It owns Rhai command registration, tool/timeline persistence, and the `.rhai` asset/import graph, and composes the language-neutral scripting host with `lunco-scripting-rhai-world`. Keeping this application closure separate means command/tool/timeline edits do not rebuild the reflected world/policy runtime.

**`lunco-scripting-rhai-world`**
Reusable Rhai world/runtime substrate. It owns the reflected world bridge, `RhaiScenarioRuntime`, authored application/Twin policy activation, policy status projection, and optional Twin-scoped native hook providers. Catalog and diagnostics consumers depend on this package directly instead of the application command/tool/timeline composition.

**`lunco-scripting-rhai-core`**
Reusable Rhai backend substrate: module resolution, native math, task-tree
lowering, typed UI request values, persisted-name validation, and the single
Rhai-to-`HookValue`/reflected-write boundary used by catalog and world
consumers. It does not own JSON, scenario lifecycle, document scheduling, or
product-level policy.

**`lunco-scripting-rhai`**
Production Rhai authoring/query package. It owns catalog discovery, script diagnostics, and dataset queries, and is installed alongside `lunco-scripting-rhai-runtime` by Rhai hosts. Keeping these API/editor-facing providers separate limits rebuilds of the language-neutral scripting lifecycle when query surfaces change.

**`lunco-scripting-bridge-core`**
Language-neutral, interpreter-free world bridge for scripting backends. It owns
native value construction, reflected ECS reads/writes, typed `HookValue`
command/query dispatch, hierarchy, authority, capabilities, and the resolved
scenario audience. It has no JSON transport dependency. Rhai and Python
provide only their native value builders and language bindings. Clock,
spatial/physics/celestial, and USD identity projections live in separate
adapters so a host that needs generic bridge access does not compile those
domain closures.

**`lunco-scripting-bridge-spatial`**
Production spatial bridge adapter for active-frame pose, rotation, navigation, geolocation, and entity enumeration. It is the owner of the BigSpace, celestial, Avian, and steering dependencies required by those projections; the generic bridge remains independent of them.

**`lunco-scripting-bridge-time`**
Production simulation-clock bridge adapter for deterministic tick/delta/elapsed reads and the complete clock-domain snapshot. It owns the physics/time-domain dependencies required by those projections; neither the neutral bridge nor the spatial adapter owns clock policy.

**`lunco-scripting-bridge-usd`**
Production USD bridge adapter for composed document generations and prim-path identity lookups. It is the owner of the USD document and scene dependencies required by those projections; scene identity remains authored by USD rather than duplicated in scripting policy.

**`lunco-tools`**
Backend-agnostic, dependency-free tool registry. A *tool* is a named, reusable unit a scenario reaches as a **script-call library** (`name::fn(...)` from rhai). A tool's implementation is pluggable (rhai source, native Rust, or future runtimes). This crate owns only the bevy-free `Tool` trait (discovery metadata + `as_any` downcast hook), the layered standard/core/application/Twin registry, and discovery — no bevy, no rhai, so the rhai-binding adapter (`lunco-tools-rhai`) stays slim. Behaviour-tree *execution* of a tool (the `run_tool` leaf) is a bevy-aware capability and lives in `lunco-tools-bevy`, not here.

**`lunco-tools-rhai`**
rhai adapter for the `lunco-tools` registry. Provides the two concrete `Tool` impls scenarios use today — `RhaiTool` (rhai source) and `NativeRhaiTool` (native Rust functions) — and `bind_registered_tools`, which binds every registered tool into a rhai `Engine` as a static module so it is callable as `name::fn(...)` from anywhere, including task closures and event/lifecycle hooks. Tools authored in other runtimes are exposed to rhai as a `NativeRhaiTool`.

**`lunco-tools-bevy`**
Bevy dispatch adapter for `lunco-tools` — the engine-action execution half. Defines a bevy-aware `ExecutableTool` supertrait + `ClosureTool` (a closure that triggers its typed command directly via `&mut World`, no JSON/reflect). Observes `lunco_core::tools::ToolFired`, looks the tool up in the registry, downcasts to `ExecutableTool`, and runs it. Tools register declaratively via `register_closure_tool(name, sigs, |world, subject, gid, args| { world.trigger(MyCommand{...}); Ok })` — the closure is the tool definition; adding an instrument is one closure, no per-instrument Rust struct.

---

### Applications

**`lunco-luncosim`**
The thin `luncosim` process/CLI shell: it selects headless versus GUI mode and delegates `luncosim test` to the separate production `lunco-scene-runner` package. It contains no renderer/window composition, so changes to presentation do not rebuild this dispatch crate.

**`lunco-scene-runner`**
Production headless runner for authored USD + Rhai scene and Twin verification checks. It owns deterministic stepping, asynchronous Modelica/physics readiness, telemetry verdict capture, diagnostics, and exit codes. Domain assertions and fixture knowledge remain in the authored scene/Rhai assets; the runner only owns the generic process and engine seams required to execute them. It is a production command dependency, not a Rust test-only crate.

**`lunco-luncosim-core`**
Dependency-light host substrate shared by GUI, server, and scene-test hosts. It
owns the headless Bevy plugin group, raw input/state schedules, asset
source/type registration, task-pool policy, build identity, and log
deduplication. It opts into only the Bevy state and keyboard/mouse event
features it uses; window-backed input focus belongs to the UI composition. It
does not install physics, USD, terrain, Modelica, celestial, avatar, or
scene-command plugins.

**`lunco-luncosim-simulation`**
Renderer-independent domain composition above the host substrate. It owns the
persistent world shell, Avian physics, USD loading/projection, terrain,
celestial, Modelica/cosimulation, mobility, avatar, controller, hardware,
telemetry, scene commands, and the headless execution plugin. Application
services remain in `lunco-luncosim-services` and Rhai integration remains in
`lunco-luncosim-runtime`.

**`lunco-luncosim-runtime`**
Production application integration above the generic simulation substrate. It installs the Rhai runtime, projects USD-authored policies, registers `SetRhaiPolicy`, consumes scripting/tool/timeline journal entries, and owns the public headless builders and launcher. Keeping this boundary above core prevents scripting and policy changes from invalidating the generic simulation composition.

**`lunco-luncosim-ui`**
Windowed LunCoSim application and presentation boundary: Bevy window/render and input-focus plugin composition, CLI render choices, egui workbench, interactive editor composition, status/camera/terrain/environment bridges, GPU-backed offscreen recording, and native desktop integration. Platform icon rasterization and live window-icon installation are isolated behind the packaging-only `package-icons` feature; `scripts/build_native.sh` enables it for packaged `luncosim` builds. Optional offscreen, networking, and updater edges are feature-scoped; native Velopack update handling is behind the opt-in `updates` feature, which the package script enables for installed desktop builds. The headless application core and ordinary UI builds do not compile the icon graphics toolchain or updater closure.

**`lunco-luncosim-presentation`**
Application-edge presentation composition. It owns the status/environment,
terrain-horizon, USD camera/light, capture, and scene-presentation bridges that
are needed by the windowed application but are not part of the reusable UI
shell's feature closure. The UI crate installs this package directly at the
application composition boundary.

**`lunco-updater`**
Optional native desktop update capability. It owns the Velopack startup hook and
the rendered update panel, so the `updates` feature adds one explicit updater
edge instead of putting updater dependencies into every UI build.

**`lunco-luncosim-exposures`**
Production integration crate for the renderer-independent runtime exposure
projection. `RuntimeExposuresPlugin` registers the single shared path from
authoritative ECS/domain state and authored telemetry to
`lunco_exposure_core::EngineExposures`; HTML, egui, API, telemetry, and remote
clients consume that registry. It owns no UI, renderer, or tutorial policy.

**`lunco-luncosim-server`**
Headless launcher for the luncosim runtime — a three-line binary that depends directly on `lunco-luncosim-runtime`, with the API + networking host enabled. The GUI shell is not in its dependency closure.
