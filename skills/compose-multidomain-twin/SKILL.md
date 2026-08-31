---
name: compose-multidomain-twin
description: >
  Compose complete multi-domain LunCoSim Twins: USD structure and wiring,
  Modelica subsystem dynamics, cosimulation, and Rhai mission behavior. Use when
  building a lander, rover, spacecraft, or fleet; wiring a Modelica model to a
  USD body; connecting GNC, propulsion, power, or thermal domains; or packaging
  a scenario as a Twin. Before designing, audit existing capabilities and the
  closest production exemplar. The skill covers Twin manifests, one program
  prim per domain, generated network roots, native USD connections, port wiring,
  scenario orchestration, and the rule that vehicles are authored USD assets,
  not Rust structs. It also covers PortRegistry ownership and environment-owned
  gravity. See docs/architecture/33-spacecraft-modeling.md and
  docs/architecture/34-scenario-and-multidomain.md.
---

# Composing a multi-domain Twin

A full mission layers cleanly — never blur the layers:

| Layer ("…") | Owns | Lives in |
|---|---|---|
| **Structure + wiring** ("what") | bodies, colliders, mass/inertia, joints, topology, program prims, port connections | **USD** (authored) |
| **Subsystem dynamics** ("how a part behaves") | thrust, propellant, battery, thermal, controllers | **Modelica / rhai** (cosim) |
| **Substrate + behavior library** ("the laws") | solver, force/joint/port plumbing, parameterized wheel/suspension/friction | **Rust** (reusable, never bespoke) |

> **Rust ships parameterized behaviors; it never hardcodes a vehicle.** A 6-wheel
> rover is a USD file, not a Rust struct — the physics/materials philosophy applied
> to whole vehicles.

**Who computes what.** Rust owns rigid-body kinematics and dynamics — bodies,
colliders, contacts, joints. Modelica owns everything else that evolves: thermal,
electrical, propulsion, structural. Modelica reaches physics through cosim ports,
and may also carry GNC or flight-software math (an equation is an equation); what it
must never become is a second physics engine. rhai stays logic.

## Start with a capability and exemplar audit

Before authoring a new vehicle, domain, controller, or Rust mechanism, inspect the
target checkout and the repositories that supplied the request. Separate what is
already implemented from what is merely not yet composed for this mission.

1. Identify the actual runtime checkout and the mission/research repository. Do
   not transfer a gap from one repository to the other without checking the
   target checkout.
2. Inventory `assets/vessels/`, `assets/components/`, `assets/models/`,
   `assets/scenes/`, and `assets/scenarios/` with `rg --files`.
3. Select the closest **production** exemplar: read its reusable asset, its
   composing scene, and its authored test. A tutorial or demo may explain an
   idea but is not automatically the production contract.
4. Write a small private matrix before editing:

   | Question | Answer |
   |---|---|
   | Generic mechanism already exists? | yes/no + source path |
   | Closest reusable exemplar? | asset + scene + test |
   | Mission-specific authoring missing? | USD/Modelica/Rhai/data |
   | External dependency required? | yes/no + explicit scope |
   | Runtime evidence available? | parse/compile/readiness/behavior/visual |

5. Classify the work as an engine substrate, reusable asset, mission assembly,
   external dependency, or evidence gap. Only an engine-substrate result justifies
   a Rust change. If a generic mechanism and exemplar already exist, compose and
   parameterize them first.

This audit is a decision aid, not a new persistent runtime registry. Keep the
final ownership and parameters in the existing USD, Modelica, Rhai, and Twin
contracts.

### Physics ports vs sensors — pick the wrong one and you author a bug

| | Physics ports | Sensors |
|---|---|---|
| There because | the body/collider EXISTS | you AUTHORED an instrument in USD |
| Ports | `position_*`, `velocity_*`, `contact`, `contact_force` | `range`, `accel_*`, `spec_force_*`, `contact` |
| Adds | nothing — ground truth | mount offset, range limits, out-of-range mode, noise, failure |
| Wire to | **physical parts** — struts, dampers, structure | **flight software** — GNC, OBC, autopilot |

A physical part reads PHYSICS: a strut's glow takes the `force` port off its own
prismatic joint, because a leg carries load when the ground pushes on it — not when
an instrument says so. Flight software reads SENSORS, because a computer knows only
what its instruments report: `DescentGuidance` reads the altimeter *with* its mount
offset and `rangeMax`, not the true height.

Backwards costs real bugs. An altimeter's datum sits above the pads, so gating a strut
on it forces a hand-copied constant to restate that offset, and the legs light before
touchdown. **When a constant in a `.mo` exists only to translate between two prims'
positions, the wire is wrong.**

**A sprung mechanism belongs to the SOLVER, not to a domain model.** A landing
leg's passive shock absorber is a `PhysicsPrismaticJoint` with standard
`PhysicsDriveAPI:linear` coefficients and the narrow
`PhysicsDriveAPI:linear` coefficients. The native Avian prismatic joint is the
sole axial mechanism in the existing substep solver, and its stroke and
reaction are the joint's own `displacement` and output-only `force` ports. Do
not write a `.mo` for the spring and do not animate it from Rhai: Modelica earns
its keep where a domain equation has no solver already integrating it.

Sign is a property of the joint. `force = stiffness * (targetPosition -
displacement)` makes the reaction opposite in sign to the displacement, so a
compressed strut reads NEGATIVE displacement and POSITIVE force, and the axis
points the way the strut extends. Author it once in `physics:localRot0`; never
correct it downstream in a wire's `lunco:factor:`, which is a unit conversion.

**A sensor reads physics; it never re-derives it.** The touchdown switch and the
collider contact ports share one computation (`avian::contact_of`). Two copies are
free to drift, and nothing in the log says which one you are looking at.

**No per-tick computation.** Prefer an on-demand port read over a mirror component
kept in step by a sync system, and `Changed<T>` over an unfiltered system. Never
per-tick work in rhai — except in a rhai *test*, where stepping is the point.

## Attach a program through the shared contract

Use `AttachProgram { doc, spec }` for a new Modelica or Python participant.
The command validates the source path and explicit scalar interface, then
authors the `LunCoProgramAPI` child, defaults, and native USD connections as one
journal/undo change set. The Models palette and Rhai prelude call this same
command; do not add a marker component or a source-specific Rust path.

```rhai
attach_program(doc, "@root@", "/Lander", "Guidance",
    "lunco://models/DescentGuidance.mo",
    [program_input_connection("altitude", "/Lander.outputs:position_y")],
    [program_output("force_y", ["/Lander.inputs:force_y"])], true);
```

Every input has one authored default or one USD connection. Every output names
its consuming USD input properties. An empty contract is source-only and must
be completed before `ListPorts`, `CosimStatus`, or force exchange can prove a
running participant. Use `@root@` for persistent scene content and
`@runtime@` only when a live overlay is the explicit intent.

This skill is the *assembly* layer over the single-domain skills:
[`build-usd-scene`](../build-usd-scene/SKILL.md) (author the scene),
[`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md) (a vessel's GNC),
[`author-scenario`](../author-scenario/SKILL.md) (behaviour),
[`run-modelica`](../run-modelica/SKILL.md) (the `.mo` models),
[`inspect-simulation`](../inspect-simulation/SKILL.md) (verify the chain).
For architecture decisions and standard-schema checks, read
[`luncosim-architecture`](../luncosim-architecture/SKILL.md). It is the gate
against special-case Rust, duplicate USD vocabulary, and compatibility paths.

## The Twin (on-disk mission unit)

A **Twin** = a folder + a `twin.toml` manifest that owns a default USD scene:

```toml
name = "luncosim"
version = "0.1.0"
description = "…"
[usd]
default_scene = "sandbox_scene.usda"   # loaded as the active stage on open; other .usda here are a referenceable library
```

Everything the mission needs lives under that folder: the scene, referenced
vehicle assets, `.mo` models, `.rhai` scenarios. Open the Twin and the default
scene loads.

### Reuse the closest working composition

Copy the *composition shape* through USD references; do not copy a whole example
into a new bespoke implementation. Preserve the exemplar's ownership, network
roots, input/output contract, lifecycle, and test pattern. Override geometry,
parameters, initial conditions, and mission policy in the new asset or scene.

For Modelica, first compose existing component classes through USD and the
generated network wrapper. A new vehicle-level `.mo` is justified only when the
required equations or public interface are absent. Do not create one file per
mission domain merely to rename existing `LunCo.*` classes.

## Decision 1 — a multi-domain vehicle is one program prim per domain + connections (SSP)

A program is a PRIM, not an attribute on the thing it drives — the same reason a
`UsdShade` shader is a prim: it has typed ports, and ports connect. Model each physical
domain as its own a `Scope` applying `LunCoProgramAPI` under the vehicle Xform, each naming its own `.mo`,
wired through the port surface. This *is* FMI/SSP — no new machinery.

```usda
def Xform "Lander" (PhysicsRigidBodyAPI …, CollectionAPI:components) # body + network root
{
    float inputs:force_local_y.connect = </Lander/GNC.outputs:thrust>   # GNC thrust → body force

    def Xform "Battery" (
        prepend references = @lunco://components/power/battery.usda@</Battery>
    ) {}
    uniform token collection:components:expansionRule = "explicitOnly"
    prepend rel collection:components:includes = [</Lander/Battery>]

    def Scope "GNC" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @lunco://models/DescentGuidance.mo@
        uniform bool  lunco:program:realtimeSafe = true                 # it drives a force
        float inputs:altitude.connect     = </Lander.outputs:position_y>
        float inputs:descent_rate.connect = </Lander.outputs:velocity_y>
        # Consume the battery output at its authored USD path.
        float inputs:engine_enable.connect = </Lander/Battery.outputs:soc_out>
        float inputs:g = 1.62                                           # only if the Modelica contract declares a runtime input
    }
    def Scope "Therm" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @lunco://models/LunCo/Thermal/ThermalMass.mo@
    }
}
```

The electrical network is the explicit `CollectionAPI:components` collection on
the assembly root. It does not need a domain-named child or an `inputs:load` wire. `Battery.mo` exposes a
`Pin`, and a pin carries a `flow` variable — the current is the circuit's answer,
not a number anyone writes in. Wire a physical bus by `connect()`-ing pins inside
one Modelica model; only *signals* (a throttle, a setpoint, a temperature reading)
cross as USD port connections. See
[54 — The Electrical Domain](../../docs/architecture/54-electrical-domain-and-modelica-libraries.md).

A wire is a native USD connection, authored on the prim that CONSUMES the value. **Do
not** host N solvers on one entity — that forces `SimComponent` to a multi-instance map
and touches the propagation core. One program prim per domain gives the same composition
with zero core change and clean per-domain telemetry.

### Acausal networks and solver boundaries

There are two different meanings of multidomain:

```text
acausal components inside one generated Modelica root       supported
acausal edges between separate runtime solver participants  require fusion
```

One `CollectionAPI:components` root is one generated Modelica participant and
one public boundary. The synthesizer may emit several connected units below
that root; those units are Modelica structure, not extra ECS entities. Keep
`Pin`, `HeatPort`, `FluidPort`, `Flange`, and other conservation connectors
inside the root that solves them.

Across separate roots, use typed scalar causal USD connections. A one-step or
macro-step boundary is an explicit coupling choice, not an acausal `connect()`.
If zero-delay bidirectional feedback is physically required, synthesize the
coupled components into one Modelica root and solve one DAE. Do not fake a
cross-root algebraic connection with a Rhai loop, a duplicated state, or a
hidden fallback. Automatic island fusion is a future capability, not a reason
to redesign a working MVP.

A vessel's OWN flight-control system is the one exception to the child prim: it is not
separable from the airframe (its `inputs:` are what the stick talks to), so the vessel
prim applies `LunCoProgramAPI` in place — see
[`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md).

## Decision 2 — the PortRegistry is the ONE input-write path

`SetModelInput`, `SetPorts`, rhai `set(id,name,v)`, Python, and wires all use the
shared `PortRegistry`. On a generated Modelica root that also carries
`InputPorts`, `InputPorts` is the authored public command boundary and therefore
wins for names it declares; the generic bridge mirrors those accepted values
into `ModelicaModel.inputs`. Other propagated model inputs remain on
`SimComponent.inputs`. A direct `ModelicaModel.inputs` write is clobbered within
one frame. Always write through a port (`SetPorts`, `set_input`, rhai `set()`),
never bypass to the model.

Battery empty events use the authored 0.1% usable-storage reserve in
`Battery.mo`; do not replace that physical boundary with Rust actuator policy or
a solver-epsilon comparison.

## Decision 3 — the scenario is a first-class USD concept

A *scenario* is the scene that bundles vehicles (referenced assets) + cosim wiring
+ per-vehicle behaviour + **one orchestration script**:

```usda
def Scope "Scenario" ( kind = "component" )
{
    custom string lunco:scenario = "rover-surface-ops"

    def Scope "Mission" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @scenarios/rover_surface_ops.rhai@
        # Keep production mission logic in the referenced source asset.
    }
}
```

- **Orchestration** (rhai): phases via the sequencer (`descend → touchdown →
  deploy → handover → task_1…`), advancing on **port-read predicates** (altitude,
  joint presence, SoC, distance, temp).
- **Per-vehicle** scripts: a a `Scope` applying `LunCoProgramAPI` child prim on the vehicle for local
  behaviour (flight assist, autonomy helpers) — delete the prim and the behaviour goes
  with it.
- **Objectives / scoring**: rhai predicates over ports — no new engine.

## Recipe

1. **Audit:** inventory the target checkout, choose the closest production
   exemplar, and classify existing capability versus mission authoring.
2. **Twin:** create/pick a folder with `twin.toml` (`[usd] default_scene`).
3. **Reference vehicles:** pull authored assets into the scene (e.g.
   `assets/vessels/rovers/{skid,ackermann}_rover.usda`, a lander) — wheel count,
   params, joints, drive type all come from USD; nothing hardcoded.
4. **Add subsystems per vehicle:** apply `LunCoProgramAPI` to each standalone
   program prim and apply `CollectionAPI:components` to each assembled network
   root. The body carries `PhysicsRigidBodyAPI` plus the force connections. Reuse
   existing component classes and generated-network patterns before adding a new
   `.mo` source.
5. **Wire cross-domain ports** with connections on the consumer
   (`inputs:load.connect = </Lander/GNC.outputs:thrust>`,
   `inputs:engine_enable.connect = </Lander/Battery.outputs:soc_out>`, …).
6. **Add the Scenario prim:** a `LunCoProgramAPI` child naming the orchestration script
   (phases + objectives as port predicates), plus a `LunCoProgramAPI` child on each
   vehicle for its own behaviour.
7. **Open + verify:** load the Twin, then use
   [`inspect-simulation`](../inspect-simulation/SKILL.md) — `cosim_status` to see the
   whole Modelica→physics chain, `read_ports` for specific values, a screenshot to
   confirm motion. Iterate on the `.usda`/`.mo`/`.rhai` (all hot-editable).

Build vertically and keep the last passing stage:

```text
static Twin/readiness
  -> rigid body/contact
  -> one closed control/actuation loop
  -> additional physical networks
  -> deployment/mobility
  -> mission policy and evidence
```

Do not add terrain, several new domains, and a long scenario to an unproven
vehicle at once. At every stage record whether the result is only parsed,
compiled, ready, numerically observed, or visually accepted.

## Gotchas

- **Don't apply gravity in `.mo`** — `lunco-environment` applies it separately; doing both double-counts.
- **Don't `SetModelInput` directly on a cosim'd entity** — clobbered every tick (Decision 2). Write the port.
- **`set_input(me,…)` is not a rhai verb** — inside a scenario use `set(me, "port", v)` (routes through `write_port`).
- **A vehicle is a USD file** — spawn/param it in USD; if you're writing a Rust struct for a specific rover, stop.
- **Unwired algebraic Modelica inputs fold to their default** — see [`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md) for the `der`-feed / wiring fix.
- **Per-domain identity is the point** — one `LunCoProgramAPI` prim per subsystem gives clean per-domain telemetry; don't collapse them onto the body.
- **Do not infer a simulator gap from a mission repository** — check the target
  asset library, source, and production test first.
- **Do not fork a tutorial model as a production controller** — reuse the nearest
  shipped controller and composition scene, then override authored facts.
- **Do not create a monolithic domain model by default** — compose reusable
  Modelica classes through USD and let the generated wrapper own the network.
- **Acausal is not the same as cross-participant** — use `connect()` inside one
  solving root and typed causal signals between roots.
- **Do not add a Rhai sleep after wiring a generated domain island.** USD composition
  defers its boundary edges until the island's `ModelicaModel` publishes the
  runtime contract. Use the readiness query/policy to wait for that stage; a
  compiler-pending endpoint is assembly progress, while an unknown port after the
  contract is terminal is a real authoring failure.

### Declared topology is not a sample

Keep the set of connectable output names separate from the values sampled this
tick. `DeclaredOutputPorts` records topology; `SimComponent.outputs` records
current samples. An empty codeless `LunCoEnvironmentProbeAPI` asset is valid:
the projection supplies the authoritative environment names, while the
environment domain removes absent samples instead of fabricating zeroes or
retaining stale values. Do not add dummy USD properties or an EarthTracker
alias to hide a missing declaration. See
[`tutorial-autopilot-and-port-contracts`](../../docs/architecture/tutorial-autopilot-and-port-contracts.md).

For a generated Modelica domain, the complete parsed wrapper output interface—including
topology-derived member aliases selected by the authored synthesizer—is published before USD
telemetry declarations are projected. Keep this lifecycle ordering intact: a compile-pending
wrapper is assembly state, not a missing-port fallback, and live values still come only from
`SimComponent.outputs`.

## Handoff report

When passing a composed Twin to another agent, record the target checkout and
revision, reused exemplar paths, authored files, network roots and boundaries,
assumption status, exact checks, runtime/API evidence, visual evidence, and the
next blocker. State explicitly which claims are source inspection only. Keep the
handoff factual and scoped; do not promote mission-specific assumptions into
general skill rules.
