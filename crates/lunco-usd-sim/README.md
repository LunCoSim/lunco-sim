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

The component-network/Modelica projection is owned by the render-free
`lunco-usd-sim-domain` package. USD co-simulation and wiring are installed by
the separate `lunco-usd-sim-cosim::UsdSimCosimPlugin`; this crate owns vehicle
realization and its simulation-specific scene projection. The complete runtime
bundle installs vehicle, shader, celestial, and telemetry projectors as
separate plugins; add `UsdSimPlugin` directly when a host needs only vehicle
realization.

### 1. `UsdSimPlugin`
The main plugin that observes USD prims and injects simulation-specific behaviors.

### 2. Applied USD vehicle schemas
The crate identifies specialized prims from their applied schema APIs (for example,
`PhysxVehicleWheelAPI`), then reads composed standard attributes from those
contracts. It does not infer a wheel from a stray attribute.
*   **Wheel Intercept**: When a `PhysxVehicleWheelAPI` is detected, the crate injects a `WheelRaycast` component from `lunco-mobility`.
*   **Tire and suspension mappings**: `PhysxVehicleTireAPI` and
    `PhysxVehicleSuspensionAPI` are projected through standard wheel-attachment
    relationships or the standard direct-API form.

### 3. Priority & Overrides
Simulation-specific behaviors applied by this crate are intended to take priority over standard collision physics. If an object is marked as a Wheel, its standard collider logic should be bypassed in favor of raycast-based ground interaction.

### 4. Physics initialization is an authored contract

Every USD dynamic rigid body starts kinematic and crosses into Avian only after
its composed pose has been read in the active physics frame. The default
`strict-authored` policy accepts a finite pose without changing it. On a scene
with terrain, an authored body or support probe that penetrates the live
surface produces a persistent `physics-initialization-terrain-penetration`
error and stays held; the runtime does not lift, reseat, zero, or otherwise
repair the body.

The default needs no custom USD property. A Twin that needs a different
initialization rule applies `LunCoPhysicsInitializationAPI` to the rigid-body
prim and authors a selector. One declared deterministic
`physics.initialization(facts) -> String` hook receives that selector; Rhai owns
its interpretation and the accept/reject decision:

```usda
prepend apiSchemas = ["LunCoPhysicsInitializationAPI"]
token lunco:physics:initializationPolicy = "my-policy"

def LunCoPolicy "PhysicsInitialization"
{
    string lunco:policy:seam = "physics.initialization"
    string lunco:policy:entry = "initialize"
    string info:sourceCode = '''
        fn initialize(facts) {
            let context = runtime_context;
            if context.scope != "twin" || context.cycle != "lifecycle"
                || context.phase != "preparation" || context.clock != "none"
                || context.generation == () {
                throw "physics initialization received the wrong runtime context";
            }
            if facts.policy != "my-policy" {
                throw "unknown physics initialization selector";
            }
            "accept"
        }
    '''
    bool lunco:policy:deterministic = true
}
```

The hook receives the stable USD subject path, selector, finite pose, and
articulated assembly member count. It never receives process-local ECS entity
ids. It must return exactly `"accept"` or `"reject"`. Missing schema or
selector, missing policy, a non-deterministic registration, malformed result,
or rejection leaves the body held and publishes a runtime diagnostic; there is
no engine fallback or implicit pose repair.

## Implementation Status
*   [x] Basic `PhysxVehicleWheelAPI` intercept.
*   [x] `PhysxVehicleTireAPI` mapping.
*   [x] `PhysxVehicleSuspensionAPI` mapping.
*   [x] Intercepted vehicle wheels use the authored raycast realization instead
    of a second standard collider path.

## Co-simulation boundary

USD-driven cosim wiring is installed by
`lunco-usd-sim-cosim::UsdSimCosimPlugin`, which translates declarative
simulation metadata into
[`lunco-cosim`](../lunco-cosim/README.md) components without any Rust
glue per scene. This is the authoritative path for USD-defined cosim
entities; `lunco-cosim` itself stays engine-agnostic. It is not part of
`UsdSimPlugin`.

### A program is a prim

A model is not an attribute on the body it drives: it is a program with typed ports,
and ports connect. A body that IS its own model authors the `info:*` properties in
place; a model that is bolted on is a a child `Scope` applying `LunCoProgramAPI`, so deleting the prim
removes the behaviour.

```usda
def Sphere "RedBalloon" (
    prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"]
)
{
    double radius = 1.0
    float physics:mass = 4.5

    uniform asset info:sourceAsset = @models/Balloon.mo@
    uniform bool lunco:program:realtimeSafe = true

    # Self-loop: the model's outputs are the body's inputs, and back again.
    float inputs:force_y.connect = </SandboxScene/RedBalloon.outputs:netForce>
    float inputs:height.connect  = </SandboxScene/RedBalloon.outputs:position_y>
    float inputs:velocity.connect = </SandboxScene/RedBalloon.outputs:velocity_y>
}
```

| Property | Purpose |
|---|---|
| `uniform token info:implementationSource` | Selects exactly one implementation arm: `id`, `sourceAsset`, or `sourceCode`. A populated non-selected arm is invalid. |
| `uniform asset info:sourceAsset` | The program's file. The ENGINE that runs it comes from the extension — `.mo` dispatches `ModelicaCommand::Compile` to the worker and (once `model.variables` populates) wraps the result in a `SimComponent`; `.py` registers a `ScriptDocument` + attaches `ScriptedModel` + `SimComponent` immediately (no compile step). `asset`, never `string`: only an `asset` is visible to USD's resolver and travels with the scene. |
| `uniform token info:sourceAsset:subIdentifier` | Which definition inside the source, when the file declares more than one. |
| `uniform string info:sourceCode` | The program's text, authored in place instead of in a file. |
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

### USD cosim companion

The `UsdSimCosimPlugin` owns connection derivation, scene transitions, and
the cosim status query. It is installed separately by the application bundle;
the vehicle plugin above does not pull that implementation crate into its
normal dependency closure.

### Runtime reload

```bash
curl -X POST http://127.0.0.1:4101/api/commands \
  -H 'Content-Type: application/json' \
  -d '{"type":"ExecuteCommand","command":"LoadScene","params":{"path":"lunco://scenes/luncosim/sandbox_scene.usda","root_prim":""}}'
```

`LoadScene` (registered by `lunco-usd-sim-cosim`) despawns every entity carrying `UsdPrimPath` plus every
`SimConnection`, force-reads the asset from disk, and spawns a fresh
root parented directly under the canonical `WorldGrid`. Use during authoring to iterate on a
USD scene without restarting the binary. `root_prim: ""` reads the stage's authored
`defaultPrim` after the asset loads. A stage without `defaultPrim` is rejected
as an invalid scene mount; the runtime never mounts the whole stage at `/`.
Worker-side Modelica state is cleaned up with the scene transition.

### Live status

`CosimStatus` (registered by `lunco-usd-sim-cosim-api`) returns one row per
`lunco-cosim::UsdSourcedCosim` entity with position, velocity, Modelica timing, and
the value currently flowing through `SimComponent.inputs["force_y"]`. Each row
also has a `status` string (`Unbound`, `Compiling`, `Running`, `Paused`, or
`Error: …`), so a source-only program or failed source load is visible instead
of looking like a missing participant. `modelica_step_diagnostics` reports
bounded per-session solver-step service time, dispatch-to-response latency, and
native pending-task count at solver start; it is absent until a step completes.
Response latency includes queueing, transport, and owner response handling, and
the browser worker does not currently report its internal queue depth.
Pass `include_values: false` for a bounded fleet view that omits input/output
maps and verbose model/error details; the default retains all named values and
full error reasons. Rhai fleet profilers can also pass `include_entities: false`
to receive `entity_count` and an aggregate `modelica_step_profile` without
building participant rows.
Useful for confirming a chain works end-to-end without polling logs.

### See also

- [`../lunco-cosim/README.md`](../lunco-cosim/README.md) — engine-agnostic cosim master loop, `SimConnection` semantics
- [`../../docs/architecture/22-domain-cosim.md`](../../docs/architecture/22-domain-cosim.md) — architecture overview
- [`../../assets/scenes/tests/cosim_chain.usda`](../../assets/scenes/tests/cosim_chain.usda) — authored Modelica → Python → Avian regression scene
