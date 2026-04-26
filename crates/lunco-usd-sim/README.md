# lunco-usd-sim

The **Simulation-Specific Metadata and Logic** bridge for OpenUSD.

## Rationale
While `lunco-usd-avian` handles standard `UsdPhysics` for generic rigid bodies, high-fidelity robotics and vehicle assets (especially those authored in NVIDIA Omniverse or Isaac Sim) use specialized schemas like `PhysxVehicleWheelAPI`. 

Instead of implementing the full, computationally heavy NVIDIA PhysX vehicle math, this crate adopts the NVIDIA schemas strictly as a **Data Contract**. It "intercepts" these tags during the USD parsing phase and "substitutes" them with LunCo's optimized, lightweight simulation models (like Raycast Suspension).

This approach provides:
*   **Interoperability**: Rover models authored for industry-standard simulators work natively in LunCo.
*   **Performance**: Lightweight ConOps physics instead of heavy iterative solvers.
*   **Decoupling**: Keeps the core Avian bridge pure while handling proprietary or complex industry extensions here.

## Key Functions & Features

### 1. `UsdSimPlugin`
The main plugin that observes USD prims and injects simulation-specific behaviors.

### 2. "Duck Typing" USD Physics
The crate identifies specialized prims by looking for specific schema attributes (e.g., `physxVehicleWheel:radius`).
*   **Wheel Intercept**: When a `PhysxVehicleWheelAPI` is detected, the crate injects a `WheelRaycast` component from `lunco-mobility`.
*   **Future Mappings**: Will include `PhysxVehicleTireAPI` for friction parameters and `PhysxVehicleSuspensionAPI` for spring dynamics.

### 3. Priority & Overrides
Simulation-specific behaviors applied by this crate are intended to take priority over standard collision physics. If an object is marked as a Wheel, its standard collider logic should be bypassed in favor of raycast-based ground interaction.

## Implementation Status
*   [x] Basic `PhysxVehicleWheelAPI` intercept.
*   [ ] `PhysxVehicleTireAPI` mapping.
*   [ ] `PhysxVehicleSuspensionAPI` mapping.
*   [ ] Automatic removal/replacement of standard `UsdPhysics` colliders on intercepted prims.

## Co-simulation translator (`cosim` module)

USD-driven cosim wiring — translates declarative simulation metadata into
[`lunco-cosim`](../lunco-cosim/README.md) components without any Rust
glue per scene. This is the authoritative path for USD-defined cosim
entities; `lunco-cosim` itself stays engine-agnostic.

### Per-prim attributes

```usda
def Sphere "RedBalloon" (
    prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"]
)
{
    double radius = 1.0
    float physics:mass = 4.5

    string lunco:modelicaModel = "models/Balloon.mo"
    string lunco:simWires      = "netForce:force_y,volume:collider,height:height,velocity_y:velocity"
}
```

| Attribute | Purpose |
|---|---|
| `string lunco:modelicaModel` | Path (relative to `assets/`) of a Modelica source. The translator opens it, dispatches `ModelicaCommand::Compile` to the worker, and (once `model.variables` populates) wraps the result in a `SimComponent`. |
| `string lunco:pythonModel` | Path of a Python script. Registers a `ScriptDocument` + attaches `ScriptedModel` + `SimComponent` immediately (no compile step). |
| `string lunco:simWires` | Comma-separated `from:to` (or `from:to:scale`) entries. Each spawns one **self-loop** `SimConnection` on the same entity (Modelica/Python ports ↔ AvianSim ports). Encoded as a single string because `string[]` arrays don't compose across `references` in the current openusd parser. |

### Cross-entity wires

For wiring values *between* entities (e.g. Modelica generator → Python
amplifier → Avian sphere), define a typeless prim with rels:

```usda
def "OscToAmp"
{
    rel lunco:wireFrom    = </SandboxScene/Oscillator>
    string lunco:fromPort = "signal"
    rel lunco:wireTo      = </SandboxScene/Amplifier>
    string lunco:toPort   = "signal"
    double lunco:scale    = 1.0
}
```

`process_usd_cosim_wires` resolves the rels to ECS entities each tick
(asset-loading-aware — defers when an endpoint hasn't spawned yet) and
spawns one `SimConnection` per resolved wire. The participant prims
themselves only need `lunco:simWires = ""` if they have no self-loops;
they don't need to know about each other.

### Runtime reload

```bash
curl -X POST http://127.0.0.1:3000/api/commands \
  -H 'Content-Type: application/json' \
  -d '{"command":"LoadScene","params":{"path":"scenes/sandbox/sandbox_scene.usda","root_prim":""}}'
```

`LoadScene` despawns every entity carrying `UsdPrimPath` plus every
`SimConnection`, force-reads the asset from disk, and spawns a fresh
root parented to the first `Grid`. Use during authoring to iterate on a
USD scene without restarting the binary. `root_prim: ""` auto-derives
`/PascalCaseFromFilename`. Note: leaks Modelica steppers in the worker
for entities that were despawned (acceptable for authoring; durable
cleanup needs a `ModelicaCommand::Despawn` per stepper, follow-up).

### Live status

`curl … {"command":"CosimStatus","params":{}}` returns one row per
`UsdSourcedCosim` entity with position, velocity, Modelica timing, and
the value currently flowing through `SimComponent.inputs["force_y"]`.
Useful for confirming a chain works end-to-end without polling logs.

### See also

- [`../lunco-cosim/README.md`](../lunco-cosim/README.md) — engine-agnostic cosim master loop, `SimConnection` semantics
- [`../../docs/architecture/22-domain-cosim.md`](../../docs/architecture/22-domain-cosim.md) — architecture overview
- [`../../crates/lunco-cosim/tests/cross_entity_cosim_test.rs`](../lunco-cosim/tests/cross_entity_cosim_test.rs) — Modelica → Python → Avian regression test
