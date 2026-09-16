> Status: Design · Audience: contributors and agents building interactive USD assemblies

# Interactive component authoring: capability gaps

This note records the remaining generic capabilities that slow down building a
componentized USD assembly in the Editor. It applies to any mission, vehicle,
payload, habitat, or instrument; it is not a model-specific design contract.

## What already works

- `assembly_edit::selection_context()` identifies the focused preview, exact
  document, edit target, selected USD path, and generation.
- `assembly_builder` produces dry typed operation plans for references,
  frames, transforms, primitive shapes, collision envelopes, clearance, and
  joints. `assembly_edit::batch` supplies one generation-checked journal/undo
  unit.
- Component documents can be edited independently in a headful Editor
  preview. A saved or committed component edit propagates to dependent
  previews without a scene restart, while the projection owns the camera and
  selection state.
- `QueryUsdPrim` and the measurement/authoring facades expose composed
  topology, bounds, frames, schemas, relationships, and source provenance.
- Twin-owned SysML is the source of requirements and parameters; Rhai loads
  that source and emits structured component and assembly evidence. Missing or
  stale facts fail closed.
- Physics admission and deferred USD work are ordered by authored path. The
  deterministic profile is single-threaded physics with explicit clocks and
  strict replay tolerances.

## Highest-value gaps

### 1. One checkpoint helper for the live edit loop

The normal edit still requires several hand-written calls: apply a plan, poll
the command, wait for projected generation, query the affected paths, capture
an Editor screenshot, and run the component gate. A generic
`editor_workflow::checkpoint` Rhai helper should accept the exact document,
generation, affected paths, and optional test command, then return one bounded
structured record:

```text
apply acknowledgement -> projected generation -> readback -> screenshot -> gate
```

It must preserve the current view, never restart the scene, and report a loud
`pending`, `stale`, or `projection_failed` state rather than retrying through a
second writer.

### 2. Atomic primitive replacement

Changing a rough visual primitive to a better primitive currently requires a
manual remove/add/move sequence and a hand-maintained requirement update. Add a
generic `assembly_builder::replace_gprim_plan` that takes an exact path, new
Gprim type, explicit geometry attributes, transform, purpose, material and
collision policy. It should preserve the path and parent, reject undeclared
children or missing required fields, and return one typed operation list. This
would make iterative silhouette repair safe without flattening a component.

### 3. Shape and material recipes

The primitive planners cover cubes, cylinders and meshes, but common visual
forms still require repetitive attribute maps. Add reusable, renderer-neutral
recipes for rounded/beveled boxes, rings, extruded profiles, brackets and
PBR/material presets. Recipes must remain Rhai plans over standard USD
`UsdGeom`/`UsdShade`; the Rust core should only gain a capability when the
standard operation cannot represent the requested fact.

### 4. Source-backed measurement report for every Gprim

The measurement facade handles explicit rules, but an author still has to
spell each radius/height/extent rule. Provide a generic report that enumerates
the selected component's supported Gprims, returns canonical SI dimensions and
world/local frames, and marks each value as authored, composed or unavailable.
It is a read-only aid for preparing SysML requirements, not an inferred
requirement generator; no guessed dimensions may be promoted to a PASS.

### 5. Component test runner without fixture boilerplate

`luncosim test` is reliable but each detached component needs a small scene
fixture and command line wiring. A generic `component test <asset> <rhai>`
entry point could create an ephemeral test host through the normal typed
composition path, run the Rhai gate in the current process, and tear it down
with an explicit lifecycle result. It must not bypass the Twin resolver or
create a second scene graph. The existing fixture path remains the auditable
and portable form for CI.

### 6. Explicit command/persistence status

Deferred commands expose `command_result`, but save and projection workflows
still make agents poll several unrelated surfaces. The API should expose one
typed lifecycle record with command status, document generation, persisted
revision, projected generation, and diagnostics. `pending` must remain a real
state; an acknowledgement must never be presented as a saved or visible edit.

### 7. Visual acceptance evidence

Typed geometry checks cannot prove that a component is recognizable or that a
camera sees all interfaces. Add a generic Editor checkpoint record containing
preview/view handles, camera preset, screenshot path, selected path, and the
matching document/projected generations. Human review remains authoritative for
recognizability; the record makes that review reproducible and prevents a stale
or unrelated screenshot from being attached to a PASS.

## Deliberately not a feature

Do not add model-specific Rust builders, hidden fallback dimensions, direct
USDA writers, per-component ECS mutation, or a second reload path. Geometry and
identity stay in USD, requirements and mission policy stay in Twin-owned SysML
and Rhai, continuous equations stay in Modelica, and Rust supplies only the
generic typed document/projection/physics seams. When a capability is absent,
return a visible error and add the smallest reusable operation at its owning
layer.

## Implementation order

1. Add checkpoint/status helpers and use them for every component iteration.
2. Add atomic Gprim replacement and shape/material recipes.
3. Add the read-only measurement and component-runner conveniences.
4. Add the visual evidence record and CI checks for stale generations.

This order shortens the feedback loop first and improves modeling fidelity
without expanding the simulator core or creating another authoring path.
