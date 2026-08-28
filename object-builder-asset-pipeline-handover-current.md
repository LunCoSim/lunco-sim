# LunCoSim Object Builder and Asset Pipeline Handover

**Validated:** 2026-08-27  
**Runtime:** `LunCoSim-win-x64-0.6.0-nightly.60.1`  
**Runtime commit/banner hash:** `4b8aab71`  
**Repository design baseline:** `3be72d963`  
**Purpose:** Current-version handover for extending LunCoSim’s object builder into an asset authoring and external-format integration system.

## Current runtime evidence

The supplied shortcut resolves to:

```text
C:\Users\salek\AppData\Local\LunCoSim-win-x64\current\luncosim.exe
```

The newest local package found was:

```text
LunCoSim-win-x64-0.6.0-nightly.60.1-win-x64-full.nupkg
```

The installed runtime was launched with:

```text
WGPU_BACKEND=vulkan
--api 4101
```

Observed startup evidence:

- Runtime banner: `luncosim 0.6.0-nightly.60.1 (4b8aab71)`.
- Window title: `LunCoSim — Listening on 4101`.
- Window responsive during the startup check.
- Stdout reached: `LunCo REPL Ready (rhai) — snippets run against the live sim`.
- Logs: `logs/luncosim-current-vulkan-20260827-170103.out.log` and the corresponding `.err.log` in the launch workspace.

The process was not running at the final post-check. Therefore this is startup/readiness evidence, not a claim of long-running stability or completed scene-editing validation. The executable’s Windows file metadata reports `0.6.0-dev`, while the installer package and runtime banner identify the installed nightly as `0.6.0-nightly.60.1`; use the package/banner identity when referring to this runtime.

## Executive decision

LunCoSim already has a substantial USD-native object-builder foundation. Do not rebuild the editor from zero.

The next work should add:

1. A formal importer/adaptor pipeline for Blender, CAD, OBJ, STL, and similar sources.
2. A native procedural builder for primitives, components, and parameterized assemblies.
3. Automated generation and validation of collision, mass, units, sockets, joints, ports, and provenance metadata.

The canonical runtime representation remains a validated USD asset package. External formats are converted into that package before simulation.

## Already implemented

### Object Builder and editing

The existing editing crates provide:

- Spawn palette and click-to-place workflow.
- Collision-based ghost preview.
- Selection and translate/rotate gizmos.
- Inspector editing for transforms and simulation parameters.
- USD prim tree.
- Mount/socket attachment and snapping.
- USD connection canvas.
- Terrain editing tools.
- Object Builder perspective registration.
- Rhai behavior editor with diagnostics and Save & Run.
- Runtime-layer persistence for document-backed Twin scenes.
- Command-side undo/redo coverage for existing edit operations.

Primary locations:

```text
crates/lunco-luncosim-edit/
crates/lunco-scene-commands/
crates/lunco-usd/
crates/lunco-twin-journal/
crates/lunco-luncosim/
```

### Spawn catalog

`crates/lunco-scene-commands/src/catalog.rs` is the current extension point:

- A USD asset is spawnable when it authors `bool lunco:spawnable = true`.
- Categories are discovered dynamically.
- Placement is derived from the composed collision geometry. The catalog keeps a
  zero/default transform as the command field; there is no `lunco:spawnLift`
  fallback.
- Adding a new USD asset does not require a Rust catalog change.

This is an asset registration mechanism, not yet a general source-format plugin system.

### Existing asset families

The working tree contains USD assets for rovers, rover variants, satellite/spacecraft and lander assets, habitats, landing pads, communication masts, ISRU structures, solar structures, wheels, motors, gearboxes, batteries, sensors, tires, suspension, communication parts, and other reusable components.

Use these documents as the assembly conventions:

```text
docs/architecture/55-building-vessels-rovers-and-landers.md
docs/architecture/60-clean-architecture-and-usd-standards.md
```

## Current format boundary

### USD: supported canonical format

The maintained path is:

```text
USD layers -> UsdLoader -> composed CanonicalStage -> ECS projection -> renderer/physics/simulation
```

USD owns hierarchy, names, transforms, references, payloads, variants, sockets, joints, ports, parameters, and authored simulation metadata.

### glTF/GLB: supported visual-asset spoke

glTF/GLB can be referenced as a binary visual asset through the USD asset-resolution bridge. `lunco-assets` also has glTF processing support.

The USD wrapper must still provide the semantic and physics metadata. A GLB by itself is not a simulation-ready LunCoSim asset.

### OBJ/STL: incomplete bridge only

The USD composition code recognizes binary references including OBJ and STL, but the current tree does not contain a complete general-purpose OBJ/STL import pipeline. Binary-reference discovery must not be treated as finished importer support.

### Blender/CAD: not shipped

There is no shipped `.blend` integration, live Blender bridge, STEP/IGES importer, or CAD-kernel integration. The existing asset-pipeline specification describes an external conversion workflow such as FreeCAD/OCCT converting CAD into polygons and generating colliders.

Relevant source:

```text
docs/architecture/40-asset-io.md
docs/architecture/41-axes-and-units.md
docs/architecture/56-asset-resolution-and-cache.md
specs/021-asset-pipeline/spec.md
```

## Architecture rules

1. USD remains the authored source of truth. Do not create a second authoritative ECS-only object model.
2. Every user edit must lower to `UsdOp` or an existing command that lowers to `UsdOp`.
3. Live edits belong in the document runtime layer and journal; base library assets remain reusable and immutable during normal editing.
4. Importers produce USD packages. They may also produce GLB visual meshes, collision meshes, thumbnails, and manifests.
5. File-system and native conversion work belongs at the `lunco-assets`/worker boundary, not inside render or simulation systems.
6. Normalize all imported data at the import seam: right-handed, Y-up, -Z forward, SI units, using the existing stage metrics/convention conversion.
7. A render mesh is not automatically a collision mesh, rigid body, mass model, joint, actuator, or sensor.
8. Preserve source URI/path, source hash, converter version, unit conversion, coordinate conversion, and warnings.
9. Keep the runtime loader compatible with browser/native deployment constraints.

Do not directly mutate `Transform`, physics components, or ad-hoc ECS state and expect the edit to survive reload.

## Target asset package

Use a package shape like:

```text
asset-name/
  asset.usda                 # semantic and simulation wrapper
  visual.glb                 # optional render mesh
  collision/                 # optional generated collision meshes
  thumbnail.png              # optional palette thumbnail
  manifest.json              # source, hashes, converter, warnings
```

The USD wrapper should provide, as applicable:

- Valid default prim and `kind = "component"`.
- Stable prim names and hierarchy.
- Visual references or payloads.
- Collision representation and policy.
- Rigid body, mass, center of mass, inertia, and material data.
- Mount sockets and component plugs.
- Joint type, axis, limits, and drives.
- Electrical/data/actuator ports.
- Parameter bounds, units, and descriptions.
- Spawn metadata and display information.
- Source/provenance metadata.

An imported mesh without this wrapper is visual-only and must not be advertised as simulation-ready.

## Importer/plugin implementation

Introduce a source adapter contract outside the renderer. Names can change, but the contract should cover:

```text
SourceAdapter
  can_handle(source_uri, extension, mime) -> confidence
  inspect(source) -> source metadata and diagnostics
  convert(source, ImportOptions) -> AssetPackage or conversion job
```

`ImportOptions` should control coordinate system, units, mesh quality, collision strategy, hierarchy preservation, socket generation, thumbnail generation, and cache policy.

`AssetPackage` should return the USD wrapper, produced binary files, manifest, diagnostics, and stable content hashes. Adapters must not spawn ECS entities directly.

Suggested ownership:

- `crates/lunco-assets/`: discovery, download/cache, hashing, preprocessing, manifests, and native worker invocation.
- New importer module/crate: adapter registry and format-specific conversion contracts.
- `crates/lunco-usd-bevy/`: load resulting USD/GLB assets and expose resolved binary sites; do not add CAD parsing here.
- `crates/lunco-usd/`: structured USD wrapper authoring and validation.
- `crates/lunco-scene-commands/`: import/register/spawn commands and runtime-layer edits.
- `crates/lunco-luncosim-edit/`: import UI, progress, diagnostics, preview, and placement.

Implement the adapters in this order:

1. GLB/glTF normalization adapter.
2. OBJ/STL conversion to GLB plus USD wrapper.
3. Blender one-way export add-on.
4. STEP/IGES conversion through an external FreeCAD/OCCT worker.
5. Optional live Blender synchronization after deterministic import is reliable.

## Blender workflow

The first Blender integration should be a reproducible export add-on:

1. Select a Blender collection as the component root.
2. Validate names and hierarchy.
3. Export visual geometry to GLB.
4. Export or identify collision geometry using a documented collection/naming convention.
5. Export custom properties for mass, sockets, ports, and joints.
6. Generate the USD wrapper and manifest.
7. Run the LunCoSim package validator before catalog registration.

The simulator must not require Blender at runtime.

## CAD workflow

Use a native worker based on FreeCAD/OCCT or another approved converter:

1. Preserve assembly hierarchy, part names, and source units.
2. Tessellate visual geometry at configurable quality.
3. Generate simplified collision geometry separately.
4. Compute mass properties only when density/material data is available.
5. Convert axes and units to the LunCoSim convention.
6. Generate sockets from explicit metadata or a documented naming convention.
7. Emit warnings for invalid solids, open shells, unknown units, missing materials, or incomplete mass data.
8. Produce the standard USD package and manifest.

CAD conversion should be an explicit job with logs and reproducible output, not an opaque operation during scene loading.

## Native authoring roadmap

### Phase A: primitive and component authoring

Add USD-backed commands/UI for cube, sphere, cylinder, cone, capsule, plane, empty/Xform, and component-instance creation. Allow transform, visibility, purpose, material, and display-name editing. Save all changes through `UsdOp`.

### Phase B: assembly authoring

Extend the existing mount and connection systems to create sockets/plugs, select joint type and axis, attach/detach components, configure limits/drives, connect ports, rename parts, and save an assembly as a reusable USD component.

### Phase C: typed parametric templates

Add deterministic generators for rover chassis, satellite buses, houses/habitats, landing pads, masts, and solar arrays. Each generator takes a typed parameter object and emits the same USD package shape as an imported asset.

Rhai should control mission behavior. Geometry generation should initially use typed generators so results are deterministic, validated, inspectable, and undoable.

### Phase D: optional mesh editing

Only add mesh editing if primitive/component authoring is insufficient. Start with a narrow toolset—vertex/face editing, extrusion, inset, bevel, merge, boolean, normals, and collision-proxy regeneration. Never silently mutate a shared library asset; save a new package.

## Persistence and safety

- Require a document-backed scene for object-builder editing, or display a clear read-only warning.
- Route every edit through the document host and runtime layer.
- Keep “Save scenario” separate from “Save as reusable asset.”
- Cover create, delete, attach, detach, transform, parameter, connection, and generator operations with undo/redo.
- Treat the per-document `DocumentHost` undo stack as authoritative for Object Builder Ctrl+Z until a deliberate twin-wide undo decision is made.

## Validation and acceptance tests

Every imported/generated package must be validated before catalog registration:

- Valid USD and default prim.
- Stable unique prim paths.
- Declared or explicit coordinate/unit policy.
- Loadable visual geometry.
- Collision geometry for simulation-ready assets.
- Valid or explicitly warned mass/inertia data.
- Valid joint body references and axes.
- Compatible mount socket/plug frames.
- Valid port declarations.
- No unresolved external URI.
- Matching manifest hashes.
- Successful catalog discovery, spawn, preview, selection, transform, save, reload, and delete.

First vertical-slice acceptance test:

1. Import one GLB through the new package contract.
2. Register it through the existing spawn catalog.
3. Spawn and preview it in the newest simulator.
4. Attach it through a mount socket.
5. Edit a parameter and connection.
6. Undo/redo the edits.
7. Save, restart, and verify the runtime overlay.
8. Save the assembly as a reusable USD asset and spawn it in a new scene.

## Source map

```text
docs/architecture/48-object-builder.md
docs/architecture/40-asset-io.md
docs/architecture/41-axes-and-units.md
docs/architecture/55-building-vessels-rovers-and-landers.md
docs/architecture/56-asset-resolution-and-cache.md
docs/architecture/60-clean-architecture-and-usd-standards.md
specs/021-asset-pipeline/spec.md
crates/lunco-luncosim-edit/
crates/lunco-scene-commands/src/catalog.rs
crates/lunco-scene-commands/src/commands.rs
crates/lunco-usd/
crates/lunco-usd-bevy/
crates/lunco-assets/
```

## Handover warnings

- The older Object Builder architecture document still has a `Status: Design` header even though later sections document landed implementation phases. Verify against source/runtime behavior.
- Earlier specifications describe historical gaps and may be stale.
- The source checkout contains many untracked local files. Inspect `git status` before committing or removing anything.
- Do not add a second direct-ECS authoring path.
