# Ship a task perspective with a Twin

A perspective assembles registered engineering views around a task. It shares
its Twin's documents, signals and simulation. Personal rearrangements are saved
in the existing local workspace store; the Rhai defaults travel with the Twin.

## Discover the available views

In a production session with API access, query `InspectWorkbench`. Its `panels`
list gives exact singleton ids; `instance_panels` gives instance kinds and any
canonical instance published by their owner. Do not guess a document id or
allocate a visualization by inventing a number. The same facts are supplied to
the perspective policy whenever capabilities, policy bindings, active document,
Twin, scene generation or mounted payload change. `scene_mounted` distinguishes
a mounted USD payload from a committed Clear with an empty viewport.

## Add a Twin policy

Create `policy/index.toml` inside the Twin:

```toml
kind = "lunco.policy.v1"
scope = "twin"

[startup]
source = "perspectives.rhai"
entry = "install"

[[policies]]
hook = "workbench.perspectives.twin"
source = "perspectives.rhai"
entry = "assemble"
skip_when_hook_unavailable = true
```

Create `policy/perspectives.rhai`:

```rhai
fn install(policies) {
    let order = [];
    for policy in policies {
        order.push(install_manifest_policy(policy.hook, policy.entry, policy.source,
            policy.policy_file, policy.deterministic, policy.required));
    }
    order
}

fn assemble(facts) {
    let available = [];
    for panel in facts.panels { available.push(panel.id); }
    if !("telemetry_browser" in available) || !("port_inspector" in available) {
        return [];
    }
    let plot = ();
    for panel in facts.instance_panels {
        if panel.id == "modelica_plot" && panel.canonical_instance != () {
            let kind = panel.id;
            let instance = panel.canonical_instance;
            plot = #{ kind: kind, instance: instance, slot: "center" };
        }
    }
    if plot == () { return []; }
    let study = #{
        id: "power-study", title: "Power study", interaction: "editor",
        description: "Compare this Twin's power signals in Graphs and inspect their current values in Ports.",
        left: ["telemetry_browser"], right: ["port_inspector"],
        instances: [plot],
    };
    [study]
}
```

The policy decides which capabilities its task needs. Return no definition when
those capabilities are unavailable. Referencing an unknown panel rejects that
scope's complete plan and reports the exact missing id; it does not create a
placeholder panel or crash the simulation.

The owner prefixes local ids with `twin/`: select this task using
`ActivatePerspective { id: "twin/power-study" }`, or **View → Perspectives**.
To make it the authored starting task, set this existing field in `twin.toml`:

```toml
default_perspective = "twin/power-study"
```

A personal saved perspective or explicit launch selection takes precedence.
**View → Reset Layout** reapplies the current admitted policy definition.
**Help → Perspective help** uses its description. Opening another Twin retires
its definitions, cached docks and unfinished policy work before reusing names.

For same-session iteration, use the existing `bind_policy(id, entry, source)`
API from the correct application or Twin execution scope. The installed hook
revision triggers recomposition. Source/manifest reload and policy retirement
use the existing policy lifecycle; changing a file alone is not evidence that
a new implementation was installed. Check `policy_status()`, `list_hooks()`
and `InspectWorkbench`.

The [workbench contract](../architecture/11-workbench.md#dynamic-assembly-and-twin-policies)
lists fields, defaults, bounds, worker context and validation. Application
layouts use the same schema in
`assets/scripting/policy/workbench_perspectives.rhai`.

## Verify the task

Open the Twin in an owned production session. Require its exact perspective id
in `InspectWorkbench`, activate it, and inspect the screenshot and current
docked panels. Use `OpenTab` / `CloseTab` with the discovered `kind` and
`instance` to open or retire an instance through the public command bus.
Rearrange a panel, switch away and back, and verify preservation;
then reset to check the new defaults. Exercise a missing-panel negative policy
and Twin A → B → A. A policy return value alone does not prove materialization.
