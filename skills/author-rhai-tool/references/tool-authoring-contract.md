# Rhai tool authoring contract

This reference is for implementing or reviewing a tool library. It is not a
replacement for the live Editor or API runbooks.

## Function shapes

Use explicit arguments. Rhai functions cannot read a top-level `let` from the
caller or borrow another script's mutable state. Pass the document identity,
paths, generation, parameters, and state explicitly.

### Pure typed-operation planner

An authoring planner should only construct data consumed by the existing
`assembly_edit` command owner. It may call generic planner helpers such as
`assembly_edit::rigid_body_plan`, `assembly_edit::revolute_joint_plan`,
`assembly_builder::frame_plan`, or
`assembly_builder::referenced_instance_targeted_plan`.

```rhai
fn panel_plan(edit_target, parent_path, name, position, scale) {
    let ops = [];
    // Append only typed operations returned by generic helpers.
    // Do not call cmd(), write files, or mutate ECS here.
    #{ ok: true, ops: ops, facts: #{ name: name, position: position, scale: scale } }
}
```

The exact operation map shape belongs to the existing generic helper. Do not
invent a private `UsdOp` encoding when a helper already returns one. Reject
malformed parameters before proposing a change. A plan that cannot express a
needed operation should return a structured capability error or `()` rather
than emitting a partially valid batch.

The caller then applies the reviewed result to the inspected document:

```rhai
let planned = component_builder::panel_plan("@root@", "/Lander", "SolarArray", [0.0, 2.0, 0.0], [1.0, 1.0, 1.0]);
if planned.ok {
    assembly_edit::batch(doc_id, "add solar array", planned.ops, parent_generation);
}
```

The `doc_id`, target, and generation are not inferred by the tool. If the
document changes between inspection and apply, the stale batch must fail
atomically and the caller must inspect again.

### Read-only report/lint

A report should query composed facts and return a stable structure such as:

```rhai
#{
    ok: errors.len() == 0,
    errors: errors,
    warnings: warnings,
    facts: facts,
    provenance: ["https://…"]
}
```

It must distinguish these states:

- absent: the required prim/attribute/port is not composed;
- malformed: it exists but has the wrong type, value, frame, or topology;
- pending: the runtime participant has not published its contract yet;
- valid: the requirement is measured and satisfied;
- assumption: the value is a Twin design choice, not a public fact.

Never make `lint()` call a repair plan. A failing report is useful evidence for
the next edit; a self-healing report makes a bad model impossible to audit.

### Runtime test observer

An authored test under the Twin's `scenarios/tests/` should sample the live
composed model in a bounded `on_tick` hook and finish with `report_verdict`.
Use `t_present`, `t_range`, `t_rel`, `t_bounded`, `t_moved`, and the other shared
assertions from `prelude/auto_tests.rhai`. Keep test state on `this`, and pass
state explicitly to helpers that reduce a sample. A test must print enough
rows to prove that it measured the intended prims/ports.

Do not use a test observer to edit USD, clear contacts, clamp a pose, reseat a
body, or hide a missing port. Those are model defects or capability gaps.

## Tool registration and scope

`assets/scripting/tools/*.rhai` are shared built-in libraries. A Twin's
`tools/*.rhai` are scoped to the active Twin and are installed when that Twin
opens. `RegisterToolLibrary` hot-replaces a named Rhai library and, with an
active Twin, persists it under the Twin's `tools/` directory. The registry is
process-global but its ownership is scoped so closing a Twin restores the
previous definitions.

Use `ListToolLibraries` to see the current registry and `GetToolLibrary` to
inspect one source-defined library. Then call a tiny function in the same
execution path that will consume it. A listed library can still be absent from
an already-created Rhai engine until its tool-generation maintenance rebuilds
the static module set.

For live work, this registration plus a real namespaced call is the tool's
compile/callability check. Do not launch a separate binary in `--validate` mode
and call that runtime evidence; parse-only validation can miss module binding,
active-Twin scope, world access, or typed-command behavior.

Do not use `import` for a registered tool library. Registered tools are static
module namespaces and are called as `name::function(...)`. `import` is for
source assets resolved by the scoped asset resolver and is a different
mechanism.

## Live edit transaction

For an Editor component or assembly:

1. inspect `ListOpenDocuments`, `ResolveUsdTarget`, `InspectUsdDocument`, and
   the focused preview; record the exact document id, edit target, and
   generation;
2. call the pure planner with those explicit identities;
3. review the operation list for duplicate paths, invalid schemas, missing
   parents, unintended visibility, and reference URI scope;
4. apply one atomic `assembly_edit::batch` or proposal commit;
5. wait for the same document generation to project, query the composed result,
   and capture/inspect a screenshot;
6. only after the visual and typed checkpoint passes, save the document through
   `save_document` or `save_as_document`.

For a separate component, save it as its own Twin file under `components/`,
then reference it from the assembly. The assembly owns placement, mount
datums, host-facing joints, variants, and cross-component connections; the
component owns local geometry, mass/collision envelope, and its own internal
requirements.

## Generic capability-gap report

When the typed owner is insufficient, record:

```text
Capability: exact missing generic operation
Evidence: command/query and returned error, plus source owner inspected
Impact: what canonical USD/component workflow cannot be completed
Proper owner: generic Rust typed USD/document/physics/projection layer
Rejected workaround: the fake or unsafe alternative not used
```

In the current editor surface, known examples include creating variant sets
and variant blocks, replacing/clearing a USD reference list, authoring
`kind`/`defaultPrim` metadata through a typed op, and deleting an inherited
prim rather than an authored prim in the target layer. The tool bridge also
needs an atomic runtime compile/check result: registration should not leave a
persisted library with an empty discovery surface and no compile diagnostic.
Use the capability report when one of these blocks a real workflow; do not
simulate it with duplicate hidden parts, raw USDA text, or a second authoring
API.
