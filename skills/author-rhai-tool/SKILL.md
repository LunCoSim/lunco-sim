---
name: author-rhai-tool
description: >
  Create, extend, register, or debug a reusable LunCoSim Rhai tool library for
  live USD authoring, component linting, inspection, or test support. Use when
  a task needs a new `name::function(...)` tool, a typed USD plan builder, a
  read-only requirement report, or a same-session tool registration. Use
  author-scenario for mission policy and edit-usd-assembly for the assembly
  workflow that consumes these tools.
---

# Author a reusable Rhai tool library

A tool library is named, reusable Rhai policy. It is not a second simulator
core, a hidden USD writer, or a vehicle-specific Rust feature. A good tool
turns a repeated authoring or inspection decision into a deterministic,
discoverable function while leaving USD, the document journal, projection,
physics, and solver ownership in their existing typed owners.

Read the focused contract when implementing one:
[`references/tool-authoring-contract.md`](references/tool-authoring-contract.md).

## Choose the tool's home

- Put a reusable engine/editor policy in `assets/scripting/tools/<name>.rhai`.
  The built-in source is scanned and embedded by the normal asset path; it is
  still a Rhai tool, not a Rust vehicle implementation.
- Put a Twin-specific builder, component lint, or requirement helper in
  `<twin>/tools/<name>.rhai`. It is persisted with that Twin and must not leak
  into unrelated Twins.
- Put a one-off mission sequence in a scenario, not a tool library. Use
  [`author-scenario`](../author-scenario/SKILL.md).
- Put continuous control equations in Modelica and generic substrate in Rust.
  A missing generic typed USD, physics, projection, or solver capability is a
  capability report, not permission to add a Griffin-specific Rust path.

Name the library after its reusable boundary (`assembly_builder`,
`egress_ramp`, `griffin_requirements`), and name functions by intent:
`*_plan`, `*_report`, `*_lint`, `*_test`, or `*_query`. Avoid names that encode
an implementation detail or a temporary failure.

## Separate the four responsibilities

Keep these responsibilities distinct even when they live in one small file:

1. **Plan** — pure calculation of a typed operation list from explicit inputs.
   A plan must not call `cmd`, mutate a document, mutate ECS, or repair a
   failed result. It accepts the exact `doc_id`, `edit_target`, paths, and
   generation needed by its caller.
2. **Apply** — the caller sends the reviewed plan to the existing document
   owner, normally `assembly_edit::batch` or the proposal/review/commit flow.
   A domain tool may provide a thin apply convenience only if it still routes
   through that owner and exposes the acknowledgement and new generation.
3. **Inspect/lint** — read-only queries over composed USD or live runtime state.
   Return structured facts, errors, warnings, and an `ok` value. A lint must
   never hide a missing prim, invent a default, or modify the model to make
   itself pass.
4. **Test** — an authored Rhai observer that samples the live model and emits
   one bounded verdict. Keep component tests separate from assembly integration
   tests; use the shared `auto_tests.rhai` assertions rather than private
   copies.

The normal authoring shape is:

```text
inspect exact target and generation
  -> pure component/assembly plan
  -> typed document batch or reviewed proposal
  -> projection/readback
  -> screenshot in the same headful session
  -> component lint/test
  -> assembly integration test
  -> save through the document owner
```

## Use an existing tool first

Before creating a library, query the live surface with `DiscoverSchema`,
`ListToolLibraries`, and `GetToolLibrary`. Search the existing
`assets/scripting/tools/` sources and the relevant Twin `tools/` directory.
Prefer composing `assembly_edit`, `assembly_builder`, `assembly_audit`,
`assembly_ui`, `nurbs`, or an existing generic prelude helper. A new tool is
justified when it adds a reusable contract, a missing generic operation
composition, or a repeatable requirement/report boundary—not when it merely
shortens one call site or hides a rejected command.

For a new component, create its requirement contract and test alongside its
tool. The assembly tool may orchestrate components, but it must not duplicate
their internal geometry or silently become the only place their requirements
are checked.

## Register and call a tool

Edit the `.rhai` source normally, then register the source in the existing
session:

```json
{
  "type": "ExecuteCommand",
  "command": "RegisterToolLibrary",
  "params": { "name": "component_builder", "source": "..." }
}
```

With an active Twin this command persists the source to `<twin>/tools/` and
publishes the named module. On the next normal engine-maintenance pass the
module is callable as `component_builder::function(...)`; no Rust rebuild is
needed for Rhai source changes.

Verify all three layers, in order:

1. `RegisterToolLibrary` acknowledged the exact name and source.
2. `ListToolLibraries`/`GetToolLibrary` show the expected backend and function
   surface.
3. A minimal call succeeds from the actual execution context (`RunRhai` for a
   one-shot check or `RunScenario` for a persistent hook).

Discovery is not invocation proof. If the name is listed but a call reports
`Module not found`, first allow the same process one update/maintenance pass
and confirm the active Twin scope. If it still fails, record it as a bridge
capability gap with the command response and do not work around it by pasting
the library into every scenario or by adding Rust-specific dispatch.

Keep one existing production process for live work. A second binary launch is
not a tool test. For a user-visible assembly, the process must remain headful;
use `RunRhai`/`RunScenario` against that process and capture the focused Editor
preview after the typed operation commits.

## Live USD rules

`.rhai` source may be edited and hot-registered. `.usda` source must not be
hand-edited for a live Editor task. Do not use `sed`, a generated USDA
replacement, `SetDocumentSource`, a raw file writer, or direct ECS mutation to
create or repair geometry. Build typed `UsdOp` plans and let the document owner
maintain transforms, journal entries, undo/redo, projection, and save output.

Use the exact `doc_id`, `edit_target`, absolute prim paths, and inspected
`parent_gen`. Group facts that must change together into one atomic batch or
reviewed proposal. Let standard schemas own standard concepts; if the typed
surface cannot author a required reference list, variant set/block, metadata,
or inherited-prim deletion, report the missing generic Rust capability instead
of faking it with hidden duplicate geometry or a compatibility alias.

A successful command acknowledgement is not visual proof. After projection,
query the composed prims and capture/inspect a project-local screenshot. A
preflight parse or lint is not runtime proof, and a screenshot is not physics
proof. Keep these evidence classes separate in the handover.

## Tool quality gate

Before handing off a library, verify:

- it parses with the repository's skill/tool validation path;
- its public functions have explicit inputs, units, return shapes, and `()` or
  structured error behavior for unavailable data;
- repeated calls are deterministic and either idempotent or explicitly reject
  an already-complete topology;
- it uses canonical USD paths and root-qualified `lunco://`/`twin://` asset
  references, never name-prefix discovery or local FreeCAD authority;
- it uses standard USD schemas and typed operations, with no hidden placeholders
  standing in for deleted parts;
- its lints are read-only and its tests assert that data was measured, not only
  that a hook happened to run;
- component tests and assembly tests run in the same requested live session;
- public/reference-backed values are labelled separately from Twin assumptions;
- any missing Rust capability is documented with evidence, impact, and the
  proper generic owner.

