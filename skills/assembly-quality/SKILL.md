---
name: assembly-quality
description: >
  Build or review a componentized LunCoSim USD assembly with a realistic,
  dimensionally checkable presentation. Use when a rover, lander, payload, or
  articulated part looks disconnected, is in the wrong frame, or needs an
  Editor-first visual and requirements review.
---

# Assembly quality gate

This is the generic guardrail for assembly work. It complements
[`edit-usd-assembly`](../edit-usd-assembly/SKILL.md) and
[`build-vehicle`](../build-vehicle/SKILL.md); it does not create a second USD
authoring path.

For a mission or operations-facing assembly, use the
[mission and engineering quality gates](../interactive-component-authoring/references/mission-engineering-quality.md)
as the system-level companion: ConOps, interface control, model-fidelity
limits, fault cases, progressive verification/validation, deterministic replay,
and configuration baselines are part of assembly readiness, not post-hoc
documentation.

## Non-negotiable boundaries

- Never hand-edit `.usd`/`.usda` text, paste a generated USDA replacement, or
  mutate ECS entities to make a preview look right. USD is authored through the
  document owner and its journal; ECS is only the projection.
- Use **Editor Perspective** for component and assembly edits. Build/View are
  for composing or operating the mounted Twin. Activate the editor with the
  typed `ActivatePerspective { id: "editor" }` command and keep the focused
  preview visible while iterating.
- Use exact `DocumentId`, `UsdPreviewId`, `UsdPreviewViewId`, edit target, USD
  paths, and generation returned by discovery. Never infer an id or target from
  a display name, a screenshot, or a stale file.
- Keep identity/topology/facts in USD, mission policy and requirements in
  Twin-owned Rhai/SysML, equations in Modelica, and generic substrate in Rust.
  A missing generic capability is a small typed-tool feature request, not a
  vehicle-specific Rust implementation.

## Plan a believable assembly before adding detail

1. Define the contract in SI metres (Y-up, right-handed, -Z-forward): root
   datum, envelope, mass/collision owner, attachment frames, interfaces,
   supported variants, and the evidence that will prove it.
2. Reuse the nearest existing component or assembly. One reusable or
   articulated component gets one explicit root and a separate Twin asset;
   the assembly owns references, instances, sockets, joints, placement and
   cross-component wiring.
3. Use named mount sockets/plugs and frame relationships. Place a part with
   `assembly_builder::place_or_attach_plan`, `align_frames_plan`, or the
   mount realignment planner. Do not copy guessed translations between parts.
4. Prefer standard USD schemas and primitive geometry for silhouette,
   mounting envelopes, collision and visible identity. Add detail only when it
   changes a contract, interface, physical envelope or recognizable shape.
5. Keep independent visual and physical roles explicit. A visual proxy may be
   visual-only, but never hide a missing body, joint, collider or reference.

## The live edit loop

Use this sequence for every component and then for the composed assembly:

```text
discover document + preview + generation
  -> inspect exact composed paths and target ownership
  -> pure Rhai *_plan (typed USD ops only)
  -> review/apply with assembly_edit::batch or proposal flow
  -> wait for projection_ready and matching projected_generation
  -> QueryUsdPrim / InspectUsdDocument / lint readback
  -> Editor screenshot and image inspection
  -> component test, then assembly integration test
  -> explicit save through the Editor document owner
```

Use `RegisterToolLibrary` to hot-reload a Rhai tool in the running process;
verify a real namespaced call, not just `ListToolLibraries`. Restart only when
the Rust binary changed. Every plan must be dry, generation-aware and
idempotent or reject a completed contract; applying a plan must never silently
flatten a referenced source.

## Make the result visually checkable

- Open one preview for the assembly and use separate preview views or
  `FrameUsdPreviewSelection` to inspect the body, landing gear, ramps, wheels,
  panels and engine/nozzle clusters independently. Do not judge placement from
  an overview in which one vehicle is clipped or occluded.
- Wait for `InspectUsdViewport.projection_ready == true` before framing or
  selecting. Correlate every screenshot with the exact document/view and
  projected generation; use `view_image` to inspect the saved PNG.
- Check silhouette first (body polygon, wheel count/orientation, legs,
  ramps, panels, engine skirt), then interfaces (no gaps, overlaps or floating
  mounts), then materials/colour and camera composition. A screenshot is
  evidence, not the model or a substitute for typed readback.
- For view-only presentation changes use `SetUsdPreviewProjection`, `Frame`,
  `Pan`, `Zoom`, and camera presets. These never author USD transforms.

## Requirements and dimensions

Store reusable facts and requirements in SysML files owned by the Twin; keep
requirements separated by owning subsystem when ownership differs. Use the
dedicated [`sysml-requirements`](../sysml-requirements/SKILL.md) runbook for
source-set configuration, qualified names, validation and verification
selection. Write generic Rhai tests with `sysml_requirements::evaluate` and
checks such as `exists`, `children`, `attribute`, `attribute_component`,
`extent_component`, `bounds_component`, and `relationship`.
Tests must fail closed on missing, stale or unavailable evidence and should
cover both structure and presentation (for example the declared wheel stations,
named mount sockets, ramp frame relationships, symmetry and metric units).

Use `QueryUsdPrim`/`InspectUsdDocument` for authoritative dimensions and
`authoring_measurements::requirement_report` for repeatable reports. Convert
standard primitive size plus local scale into full extents; do not trust
`extent`, a screenshot, or a hand-measured display box. Record the source,
units, path and generation for every dimensional result.

## Before handoff

Run the Twin's `--validate` and production `luncosim test` checks, inspect the
focused Editor screenshots, and save only after the visual and typed gates
agree. Capture the exact report and screenshot paths, run `git diff --check`,
and stage only the authored Twin/tool/docs changes. Preserve unrelated dirty
worktree files. If a standard USD operation is missing, document the smallest
generic typed operation needed and keep the workaround in Rhai until it is
implemented.
