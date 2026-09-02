# 04 — Attach a simulation program

> **Pair with:** the Build perspective / Models palette in the luncosim app.
> **Reference:** [USD Domain](../architecture/21-domain-usd.md) ·
> [Co-Simulation Domain](../architecture/22-domain-cosim.md) ·
> [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md)

This walkthrough teaches the complete authoring path for attaching a Modelica
or Python source to an existing USD body. The important distinction is that
choosing a source is not enough: the USD program prim also needs an explicit
scalar interface and native USD connections before it can exchange values with
the simulation.

## 1. Start with a real Twin

Open an existing Twin or create one with [00 — Create your first Twin](00-create-a-twin.md).
A loose USD file can be inspected, but the Models palette only edits a
document-backed scene. Promote a loose scene with `SaveAsTwin` first so the
authoring change has a USD layer to journal and save.

## 2. Attach from the palette

1. Open the **Build** perspective and show the **Models** panel.
2. Choose a discovered `.mo` or `.py` source.
3. Click the USD body that should own the program.
4. Inspect the scene tree. The program is a child `Scope` with
   `LunCoProgramAPI`, a typed `info:sourceAsset`, declared `inputs:` and
   `outputs:`, and any connections authored in the scene layer.
5. Save the USD document. The attachment is an authored USD edit, not an ECS
   marker, so it is journalled, undoable, and available after reload.

Python entries are disabled when the application was built without the Python
backend. That is a build capability, not a silent runtime fallback.

The shipped balloon entries include their scalar contract as a convenience.
For every other source, an empty contract means **source-only**: the program
can be inspected, but it is not a running scalar cosim participant. Add the
contract explicitly before expecting ports or force/torque exchange.

## 3. Author an arbitrary contract with Rhai

The same typed command is exposed by the `assembly_edit` tool. Obtain the USD
document id from `ListOpenDocuments`, then choose the edit layer deliberately:

```rhai
let doc = /* the id of the open USD document */;
assembly_edit::attach_program(doc, #{
    edit_target: "@root@",
    host_path: "/Vessel",
    name: "Guidance",
    source_asset: "lunco://models/Guidance.mo",
    inputs: [
        assembly_edit::program_input_connection("altitude", "/Vessel.outputs:position_y"),
        assembly_edit::program_input_default("gravity", 1.62),
    ],
    outputs: [assembly_edit::program_output("thrust", ["/Vessel.inputs:force_y"])],
    realtime_safe: true,
});
```

`@root@` persists authored scene content. `@runtime@` is for a live test or a
document-backed run overlay and must be chosen explicitly. Each input has
exactly one default or one connection. Each output declares its consumers.
The command rejects malformed paths, duplicate ports, unsupported types, and
connections to non-input properties before authoring anything.

## 4. Verify the participant, not just the prim

Use the API or the authored Rhai observer to check all three layers:

- `ListPorts` shows the declared input and output names.
- `CosimStatus` lists the projected program and its `SimComponent`.
- `GetBrokenConnections` is empty for the expected program-to-body wires.

The production gate
[`program_attach_command.usda`](../../assets/scenes/tests/program_attach_command.usda)
and
[`program_attach_command.rhai`](../../assets/scenarios/tests/program_attach_command.rhai)
exercise this exact command path without a Rust test or rebuild after Rhai
changes:

```bash
target/debug/luncosim test \
  --scene scenes/tests/program_attach_command.usda --max-ticks 3000
```

The observer waits for the authored program prim, its Modelica ports, and its
`CosimStatus` participant. A parse-only `--validate` result is not sufficient.

## Ownership rules to keep

- USD owns the program prim, interface, topology, connections, and parameters.
- Modelica owns continuous equations, state, and continuous control laws.
- Rust owns reusable runtime mechanisms, projection, port propagation, and
  physics hot paths.
- Rhai owns sequencing, mission policy, UI guidance, and test verdicts. It does
  not write force or throttle values every tick in production.

Delete the program prim to remove its behavior. Do not add a marker component,
parallel registry, source parser in the palette, or a vehicle-specific command.
