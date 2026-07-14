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

### A program is a prim

A model is not an attribute on the body it drives: it is a program with typed ports,
and ports connect. A body that IS its own model applies `LuncoProgramAPI` in place; a
model that is bolted on is a `LuncoProgram` child prim, so deleting the prim removes
the behaviour.

```usda
def Sphere "RedBalloon" (
    prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI", "LuncoProgramAPI"]
)
{
    double radius = 1.0
    float physics:mass = 4.5

    uniform asset lunco:program:sourceAsset = @models/Balloon.mo@
    uniform bool lunco:program:realtimeSafe = true

    # Self-loop: the model's outputs are the body's inputs, and back again.
    float inputs:force_y.connect = </SandboxScene/RedBalloon.outputs:netForce>
    float inputs:height.connect  = </SandboxScene/RedBalloon.outputs:height>
    float inputs:velocity.connect = </SandboxScene/RedBalloon.outputs:velocity_y>
}
```

| Property | Purpose |
|---|---|
| `uniform asset lunco:program:sourceAsset` | The program's file. The ENGINE that runs it comes from the extension — `.mo` dispatches `ModelicaCommand::Compile` to the worker and (once `model.variables` populates) wraps the result in a `SimComponent`; `.py` registers a `ScriptDocument` + attaches `ScriptedModel` + `SimComponent` immediately (no compile step). `asset`, never `string`: only an `asset` is visible to USD's resolver and travels with the scene. |
| `uniform token lunco:program:sourceAsset:subIdentifier` | Which definition inside the source, when the file declares more than one. |
| `uniform string lunco:program:sourceCode` | The program's text, authored in place instead of in a file. |
| `uniform bool lunco:program:realtimeSafe` | The author's promise that the program steps fast enough to be trusted with a FORCE. Absent ⇒ not promised, and the wiring pass refuses it a `force_*`/`torque_*` port on a client-predicted body. |
| `float inputs:<port>` | An input port. With a `.connect` it is a wire; with a constant it is a parameter — `float inputs:kv = 1.2`. |

A prim is stepped iff it BOTH binds a program AND declares connectable ports. A model
with no ports is a documentation-only reference; ports with no model are a pure physics
sink driven through its backend.

### Wires

A wire is a native USD connection — `inputs:x.connect = </Path/To/Prim.outputs:y>` —
authored on the prim that CONSUMES the value, exactly as `UsdShade` wires a shader
network. `rewire_usd_connections` derives one `SimConnection` per connection. The same
form wires values *between* entities (Modelica generator → Python amplifier → Avian
sphere) as within one: the target path simply names another prim.

```usda
def "Amplifier"
{
    float inputs:signal.connect = </SandboxScene/Oscillator.outputs:signal>
}
```

Resolution is asset-loading-aware — a connection whose endpoint has not spawned yet is
deferred, not dropped. Neither participant has to know about the other: the consumer
names the producer, and nothing else changes.

### Runtime reload

```bash
curl -X POST http://127.0.0.1:4101/api/commands \
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
