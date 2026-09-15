---
name: interactive-component-authoring
description: >
  Build or repair a reusable scene component through a live LunCoSim Editor
  session. Use when the work must be decomposed into small component tasks,
  checked visually after each task, and verified with typed USD queries and
  Rhai tests. This is the normal cycle for assemblies; do not use a whole
  vehicle batch as the editing unit.
---

# Interactive component authoring cycle

This is the repository's agreed editing rule for realistic assemblies. The
simulator is an interactive workbench, not a batch USDA generator. Keep the
headful production session open, edit one component at a time, inspect what
the user can see, and stop at every checkpoint that can change the design.

Use this skill with [`edit-usd-assembly`](../edit-usd-assembly/SKILL.md) for the
Editor lifecycle and [`assembly-quality`](../assembly-quality/SKILL.md) for
dimensions, frames, collision and visual gates. For a new reusable asset also
read [`author-usd-component`](../author-usd-component/SKILL.md).

## The mandatory cycle

Keep one production Editor session open for the assembly and use **Editor
Perspective** for every authoring checkpoint. The unit of progress is one
component task, not a vehicle-wide script. After each task, stop and inspect
the live projection; do not begin the next component until the typed readback
and the Rhai gate agree with what is visible. This is the short interactive
loop the agent must run itself, even when the user is not watching the
terminal:

For each component, in order:

```text
discover the exact USD document, preview, view, edit target and generation
  -> define the component contract and its local datum in SI metres
  -> build one pure Rhai plan using typed USD operations
  -> review and commit that one component change
  -> wait for projection_ready and the matching projected generation
  -> query the authored/composed prims, bounds, relationships and schemas
  -> inspect the focused Editor view and capture a project-local screenshot
  -> run the component's source-backed Rhai requirement gate
  -> show the checkpoint / collect feedback before the next component
  -> save only after the visual and typed gates agree
```

If a component is wrong, change only that component (or its explicit mount
contract), re-project it in the same session, and repeat the checkpoint. Do
not accumulate several speculative edits and then ask for a final review.
When a task genuinely needs multiple dependent parts, make the dependency
boundary explicit (for example, a wheel and its strut), run the local gate,
then continue with the next separately named task.

Do not queue a bus, tanks, legs, engines, ramps and panels into one unobserved
call. A component task may contain the minimum atomic operations needed for
that component (for example a wheel plus its strut and mount), but it must
produce its own projection, typed evidence and Rhai verdict. Then run an
assembly integration gate that checks placement, symmetry, clearances,
references, joints and cross-component wiring; an isolated component pass does
not prove its mounting.

## Decompose by ownership

Start with the root datum and contract, then use a dependency order that keeps
each checkpoint understandable:

1. chassis/body datum and envelope;
2. one repeated or articulated component (one leg, wheel, tank, nozzle,
   ramp, or panel) and its local interfaces;
3. the remaining instances through explicit references, patterns or mirrors;
4. joints, sockets, collision and mass ownership;
5. presentation details and materials;
6. the composed assembly and mission-facing runtime behavior.

The component owns local geometry, material targets, collision envelope, mass
and named attachment frames. The assembly owns references, placement,
instance overrides, symmetry, host-facing joints and cross-component links.
SysML owns the requirement intent, Modelica owns continuous equations, Rhai
owns observation/policy, and Rust owns only generic typed substrate. Never
create a vehicle-specific Rust builder or a second scene graph to make a
checkpoint convenient.

## Practices adapted from established DCC/CAE tools

These are workflow principles, not a request to import a private CAD file or
to copy another tool's scene model:

| Reference practice | LunCoSim rule |
|---|---|
| Blender separates Object transforms from Edit Mode geometry; unapplied scale can make downstream dimensions ambiguous. | Establish the component datum, apply/normalize transforms through the typed Editor operation, then measure the composed result. Never hide a scale error in a child translation. |
| FreeCAD Part Design uses a Body, local coordinate system, datum geometry and an ordered feature history. | Give each reusable component one root, one local frame, explicit mount datums and a reviewable Rhai plan. Keep operations incremental and named rather than a flattened mesh dump. |
| Fusion 360 treats a Component as the unit with its own origin, coordinate system, timeline, joints and parts-list identity; external components are referenced into assemblies. | Keep independently reusable parts in separate Twin assets and compose them by typed USD references and named sockets. Preserve source identity and edit the assembly's placement, not a flattened copy. |
| SOLIDWORKS recommends mating to one or two common references, avoiding loops/redundant mates, fixing errors early and solving detail in subassemblies. | Anchor components to explicit assembly datums/sockets, avoid duplicate constraints, fail immediately on ambiguous frames, and verify each subassembly before the top-level assembly. |
| COMSOL keeps a geometry sequence and named selections so later physics/material/mesh nodes remain associated after geometry changes. | Use stable USD paths, named frames, sockets, collections and standard schemas as the selection/association boundary. Requirement checks must query those identities, not leaf-name guesses or screenshot pixels. |
| OpenUSD composes encapsulated assets through references, payloads and variants, and evaluates transforms through the ordered xform-op stack. | Use the existing typed reference/variant/payload tools and transform planners. Let the USD owner maintain `xformOpOrder`; never hand-edit USDA or patch a generated file behind an open document. |

Primary references:

- [Blender transform introduction](https://docs.blender.org/manual/en/latest/scene_layout/object/editing/transform/introduction.html)
- [FreeCAD Part Design](https://github.com/FreeCAD/FreeCAD-documentation/blob/main/wiki/PartDesign_Workbench.md)
- [Fusion components](https://help.autodesk.com/view/fusion360/ENU/?contextId=ASM-COMPONENTS)
- [SOLIDWORKS mate best practices](https://help.solidworks.com/2024/English/SolidWorks/sldworks/c_Best_Practices_for_Mates_SWassy.htm)
- [COMSOL named selections](https://doc.comsol.com/6.3/doc/com.comsol.help.comsol/comsol_ref_visualizationselection.22.25.html)
- [OpenUSD references](https://openusd.org/release/api/class_usd_references.html) and
  [ordered transforms](https://openusd.org/dev/api/class_usd_geom_xformable.html)

## What an agent may do

- Use Editor Perspective and the existing `assembly_edit`,
  `assembly_builder`, `component_editor` and measurement facades.
- Hot-reload a Twin-scoped Rhai tool with `RegisterToolLibrary`, prove a real
  namespaced call, and reuse it for the next component.
- Add a small generic Rust typed operation only when capability discovery shows
  that the existing owner cannot represent the required standard USD fact.
- Use `UndoDocument`/`RedoDocument` for feedback and keep save as an explicit
  approval boundary.

## What an agent must not do

- Do not edit `.usd`/`.usda` text directly, write a shadow USDA file, mutate
  ECS state to make a preview look correct, or restart the app between every
  component.
- Do not build the whole vehicle in one Rhai batch and reveal only the final
  frame. Do not infer a socket, transform, dimension, or requirement from a
  name or screenshot.
- Do not duplicate requirement thresholds in Rust or a Rhai tool. Load the
  Twin's SysML source through `sysml_requirements::source()` and report every
  missing, stale or unavailable fact as a visible failure.
- Do not accept a green component test as proof of assembly placement. Run the
  parent integration checks separately.

## Checkpoint record

Record one short entry per component: document/preview/view handles, source
file, edit target and generation; changed paths and operation count; queried
dimensions/frames/relationships; screenshot path; Rhai requirement report;
and the next feedback decision. A final handoff lists the component gates and
the assembly gate separately, plus any known runtime or visual blocker.
