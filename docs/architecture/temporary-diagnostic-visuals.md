# Temporary diagnostic visuals — one lease, one render path

> Status: Design · Audience: contributors adding camera, collider, frame, or
> dynamics diagnostics

This document defines the future contract for temporary camera and collider
visualization. It is a runtime diagnostic view, not authored scene content. The
design deliberately reuses the existing `Gizmos`, USD, Avian, BigSpace,
viewport, tool, and lifecycle owners. It does not add a USD schema, a second
scene model, or a second camera/physics path.

## Decision

Use one runtime `DiagnosticVisualLease` store owned by the interactive scene
editing plugin. A lease describes an explicit target, diagnostic kind, and
presentation policy. It returns an opaque handle bound to the current Twin and
scene-mount generation.

The lease store is the only public control surface for temporary diagnostics:

```text
Rhai/UI/API intent
    → typed Acquire/Update/ReleaseDiagnosticVisual command
    → one lease store keyed by Twin + mount generation + lease id
    → change-gated diagnostic snapshot
    → existing Bevy Gizmos presentation pass
```

Rhai chooses *which* target and *which* diagnostic policy to show. Rust owns
target validation, lifecycle, geometry projection, BigSpace conversion, render
isolation, and cleanup. No diagnostic operation writes USD, Avian state,
`SceneViewport`, or a physics command port.

The existing `physics_viz.rs`, `joint_viz.rs`, and `physics_gizmo.rs` are the
source-backed implementation precedents: they already use immediate-mode
`Gizmos`, read runtime state, and keep presentation outside USD. Their separate
global toggle commands (`TogglePhysicsArrows`, `ToggleJointViz`, and
`TogglePhysicsGizmo`) must be migrated under this lease boundary when the
feature is implemented; a fourth toggle API is not allowed. The migration
removes the old commands and resources in the same implementation change.

This lease is only for transient diagnostics. Authored geometry, route ribbons,
motion trails, HUDs, and the transform gizmo retain their existing owners and
contracts; they are not silently folded into a global visual registry.

## Existing owner and reader paths

| Fact or operation | Authoritative owner | Diagnostic reader |
|---|---|---|
| Target identity | `GlobalEntityId` / resolved USD prim identity in the active Twin | command resolver and `StageView`/ECS lookup |
| Scene validity | `SceneMountState` and the active scene root | lease admission and snapshot invalidation |
| Camera selection | `SceneViewport.active_camera`, reconciled by `lunco-usd-bevy::camera_switch` | camera diagnostic snapshot |
| Camera intent | `lunco_render::SceneCamera` plus Bevy `Projection`/`Camera` | camera diagnostic snapshot |
| Camera render pose | propagated `GlobalTransform` in the render frame | Gizmos draw pass only |
| Physical frame | `ActivePhysicsFrame`, `GridAnchor`, and BigSpace attachment helpers | target pose conversion; never guessed from camera state |
| Collider shape and attachment | Avian `Collider`, `ColliderOf`, `ColliderTransform`, and the USD/Avian projection | collider snapshot |
| Authored collider facts | composed USD `StageView` with standard `UsdGeom` + `UsdPhysics` | identity/diagnostic explanation only; never a second runtime collider |
| Selection | `lunco_scene_commands::SelectedEntities` | default target policy, when explicitly requested |
| Tool discovery | `lunco-tools` and `lunco-tools-rhai` | optional Rhai source library; no per-tool registry |
| Scene teardown | `SceneTeardown`, `SceneMountState`, `TwinClosed` | lease revocation and store reset |

`GlobalTransform` is a render projection. It is valid for drawing because the
diagnostic pass runs after the same BigSpace/transform propagation used by the
mesh renderer; it is never converted back into an authoritative physics or
USD pose. A collider diagnostic reads the actual Avian realization. A composed
`StageView` is used only when the question is about authored identity or
topology, not to rebuild a second collider.

## Lease contract

The implementation adds one typed command family, with names illustrative until
the command schema is authored:

```text
AcquireDiagnosticVisual { target, kind, policy } → DiagnosticVisualLease
UpdateDiagnosticVisual  { lease, target?, kind?, policy? } → DiagnosticVisualLease
ReleaseDiagnosticVisual { lease } → unit
```

`target` is an explicit entity address: a `GlobalEntityId` for a live ECS
entity or a composed USD prim address resolved through the existing `StageView`
binding. A prim name, entity order, camera order, or current selection is not
an implicit target. Selection is a named policy that resolves through the
existing `SelectedEntities` owner and is rejected when it is ambiguous.

`kind` is a closed diagnostic enum, initially:

- `camera` — one explicit `SceneCamera`/camera entity;
- `collider` — the runtime collider or compound collider attached to one
  explicit physical body.

The handle contains no raw Bevy `Entity` exposed to Rhai or the HTTP API. It
contains an opaque lease id plus the Twin identity, scene-root identity, and
mount generation needed for validation. The generation is invalidated before
deferred scene despawn, so a recycled entity id cannot make an old handle draw
on a new scene.

### Idempotence and failure

| Operation | Required result |
|---|---|
| Acquire same owner/target/kind/policy | Return the existing handle; do not add another overlay |
| Acquire changed policy | Create one new lease only when the request is not an update to an existing handle |
| Update unchanged spec | No snapshot rebuild and no transform/material write |
| Update changed spec | Replace the lease spec and increment its revision atomically |
| Release valid handle | Remove the lease and its snapshot; no entity remains |
| Release already released/stale handle | Return a visible `stale_handle` result; never recreate or retarget anything |
| Target despawned or leaves the scene root | Revoke the lease and publish the owning diagnostic; never draw at the last pose |
| Unsupported shape/projection | Keep the lease failed and report the exact unsupported kind; do not substitute a box or guessed camera |
| No render pipeline/viewport | Return `render_unavailable`; do not claim a headless visual succeeded |

Invalid target, stale handle, unsupported shape, and missing viewport are
structured command errors. They are not silent no-ops and do not fall back to
the first matching entity or camera.

The lease is runtime state only. It is not written to the USD runtime layer,
the Twin manifest, settings, the journal, or the network document plane. A
user can explicitly re-acquire it after reload; a new Twin cannot inherit it.

## Diagnostic geometry

### Camera

The camera snapshot reads the selected camera's `Projection` and propagated
render pose. It never derives projection constants from a screenshot or from a
different camera:

- perspective uses the authored/runtime `fov`, `near`, `far`, and the measured
  viewport aspect;
- orthographic uses its actual half-width/half-height and near/far planes;
- the camera frame uses the camera entity's render-frame rotation and the
  established camera forward axis (`-Z`), with an explicit triad at the camera
  origin;
- `SceneViewport.active_camera == target` and the reconciled `Camera::is_active`
  are shown as status facts, but the diagnostic never writes either one;
- a missing, non-finite, or unsupported projection is an error marker, not a
  guessed unit frustum.

The viewport rectangle comes from `SceneViewport.rect`. A render target or
sensor camera is not promoted to the main window by scanning active cameras.
The existing viewport reconciler remains the sole writer of window-camera
activation.

### Collider

The collider snapshot reads the runtime Avian realization and its attachment
transform. The first supported shapes are the same standard USD/Avian mapping
already implemented by `lunco-usd-avian`: cuboid, sphere, cylinder, cone,
capsule, plane/thin cuboid, and the supported mesh/heightfield forms. Compound
children are shown at their actual local attachment poses when Avian exposes
those shape parts; otherwise the diagnostic reports that compound detail is
unavailable instead of reconstructing it from guessed USD hierarchy.

The snapshot distinguishes:

- body frame and collider-local frame;
- the body/collider relationship (`ColliderOf`);
- the actual `ColliderTransform` offset;
- supported shape dimensions and mesh/heightfield identity;
- missing, unsupported, or stale physical state.

The visualizer never adds a rigid body, collider, joint, contact, force, or
physics transform. It also never changes collision filters or material
assignment. A collider visual is a view of the solver input/realization, not a
second collision model.

### Presentation path

Use the existing Bevy immediate-mode `Gizmos` pass. This is appropriate because
these are operator diagnostics that should be depth-tested/biased according to
the existing diagnostic gizmo configuration, not scene geometry that should be
saved, shaded, or replicated. It avoids temporary `Mesh3d`/material entities,
render-layer bookkeeping, GPU asset lifetimes, and a second cleanup tree.

The snapshot builder may use `Gizmos` only as its final renderer. It must not
perform USD parsing, compound extraction, mesh baking, BigSpace reparenting, or
entity scans in the draw loop. The heavy work is change-gated by lease revision,
target transform/shape revision, camera projection/viewport status, and scene
mount generation. Stable leases redraw only their cached bounded line/primitive
commands.

The diagnostic render pass is scheduled after finalized BigSpace transform
propagation and the viewport reconciliation boundary. It consumes the render
pose for lines, while all source facts and frame conversions are resolved before
that boundary. It does not write `GlobalTransform`, `CellCoord`, `Transform`,
`Position`, or `Rotation`.

## Rhai and tool surface

Rhai policy uses the existing typed command bridge. A source tool may provide
small intent functions such as:

```rhai
fn show_camera(target) {
    cmd("AcquireDiagnosticVisual", #{ target: target, kind: "camera" })
}

fn show_collider(target) {
    cmd("AcquireDiagnosticVisual", #{ target: target, kind: "collider" })
}

fn close_visual(lease) {
    cmd("ReleaseDiagnosticVisual", #{ lease: lease })
}
```

These functions select target and policy only. They do not read ECS
components, issue per-frame writes, parse USD, or mirror physics values. If a
clickable editor tool is useful, it follows the existing `on_click(entity_id)`
signature discovered by `lunco-tools`; it does not add a parallel palette or
target registry.

Production scenarios must invoke the commands on an explicit event or user
action. They must not use a production `on_tick` polling loop to keep a lease
alive. The lease store owns liveness from target and lifecycle changes.

## Lifecycle and multi-Twin matrix

| Event | Lease-store action | Required invariant |
|---|---|---|
| Acquire after a valid mount | bind to Twin, root, and generation | only that mounted root can be read |
| Additive Twin opens | keep separate lease namespace | Twin B cannot see or release Twin A handles |
| `SceneMountState.begin_replacement()` | invalidate generation before deferred despawn | outgoing targets cannot enqueue new visuals |
| `SceneTeardown` | release all leases and cached geometry for the outgoing root | no diagnostic survives scene replacement |
| `TwinClosed` active | clear that Twin's leases and owner state | closing the Twin cannot leave a global overlay |
| `TwinClosed` non-active | clear only that Twin's leases | active Twin diagnostics remain intact |
| target deletion | revoke only leases for that target | no detached geometry or stale handle reuse |
| reload with same source path | treat as a new mount generation | re-acquisition is required |
| repeated acquire/update/release | compare key/spec/revision | no duplicate entities, commands, or snapshots |

No name-only global map is permitted. The lease store is scoped by the same
Twin/root lifecycle that owns scene entities, and its teardown observer lives
beside the state it writes.

## Test and acceptance plan

The implementation should add only tests that protect generic mechanism seams;
observable behavior belongs in an authored Rhai scenario exercised by the
production binary.

### Focused mechanism checks

- lease identity is idempotent and generation-bound;
- update/release cannot operate on another Twin or an old mount generation;
- target deletion revokes the lease before the draw pass;
- perspective and orthographic frustum lowering preserves the source
  projection;
- supported collider shape/compound lowering preserves the Avian local frame;
- equal snapshots do not mark meshes, transforms, or physics state changed;
- no command path mutates USD, `SceneViewport`, Avian bodies, or ports.

### Authored negative and lifecycle fixture

Author `assets/scenes/tests/diagnostic_visuals.usda` from standard `UsdGeom`,
`UsdPhysics`, and existing LunCo scene contracts, plus
`assets/scenarios/tests/diagnostic_visuals.rhai`. The scenario should cover:

1. explicit camera acquire/update/release and the camera's actual projection;
2. explicit collider acquire for a simple and compound body;
3. a missing target, unsupported collider, stale handle, and headless
   `render_unavailable` result;
4. Twin add/close and scene reload, proving isolation and revocation;
5. unchanged repeated commands, proving no duplicate lease or geometry;
6. no USD or physics-port changes after the diagnostic commands.

### Production evidence

After implementation, launch only the production
`target/debug/luncosim` with an explicit API port and an absolute scene path.
Set `LUNCO_ASSET_ROOT` to the current worktree's `assets` directory. Through
the API/Rhai command surface, acquire each visual, inspect the lease/query
result, capture the windowed viewport, switch the active camera through the
existing typed camera command, and release the leases. Verify:

- camera frustum and axes match the selected camera's framing;
- collider outlines match the runtime body/compound shapes and local frames;
- the authored mesh/materials and physics telemetry are unchanged;
- repeating commands does not multiply overlays;
- reload and Twin close remove all diagnostic lines;
- a typed `Exit` releases the session and API port.

The visual capture is required for acceptance. A source review, `--validate`,
or a headless command result alone cannot prove that the overlay is visible,
correctly framed, and cleaned up.

## Non-goals and rejection rules

- No `LunCoDiagnosticAPI` or other USD schema: this state is not authored scene
  meaning.
- No custom `lunco:*` duplicate of `UsdGeom`, `UsdPhysics`, `UsdShade`, or
  `UsdLux` camera/shape/material/shadow facts.
- No temporary USD prims or per-frame journal/runtime-layer edits.
- No temporary physics bodies, collider clones, contact probes, or force paths.
- No second viewport/camera selector and no first-camera fallback.
- No raw absolute `f32` pose, direct `GlobalTransform` physics authority, or
  camera-relative correction to hide a BigSpace error.
- No global name-only lease registry, process-global target cache, or leaked
  state across `TwinClosed`.
- No per-frame Rhai polling, JSON change detection, or duplicate UI palette.
- No use of a diagnostic line as production geometry, route state, or an
  acceptance substitute for actual scene behavior.
