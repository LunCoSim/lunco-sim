# 55 — Building Vessels, Rovers, and Landers: Architectural Guide

> Status: Active · Audience: anyone assembling a rover, lander, or satellite

This document is the step-by-step architectural guide for assembling mission-grade space vessels (wheeled rovers, powered descent landers, spider-rovers, and satellites) in LunCoSim using the 3-plane modular architecture.

---

## 1. Core Architectural Principles

When building any vehicle assembly in LunCoSim:

1. **Decoupled Component References**:
   - Component files in `assets/components/` (`battery.usda`, `motor.usda`, `cryo_tank.usda`) contain **zero vehicle-specific paths** (`/SkidRover` or `/Rover`).
   - Component files define reusable part interfaces and nameplate defaults. The vehicle
     file references those parts and owns its actual topology.

2. **Assembly Roots and Standard Collections for Networks**:
   - Every independently solved physical network has one network-root prim applying
     `CollectionAPI:components`. When the network belongs to a vehicle assembly, the
     assembly root owns it; a domain-named child is unnecessary.
   - The collection includes the actual assembled part prims; it does not create proxy
     copies below the Scope.
   - Every included Modelica facet explicitly uses
     `info:implementationSource = "sourceAsset"` with a `.mo` source.
   - Runtime Rust projection reads and validates the composed stage; the selected synthesizer emits the transient Modelica wrapper. The existing `synth.<name>` Rhai policy seam may own the emitted source, unit merge, and diagram layout without changing Rust.
   - One collection is one runtime compilation boundary. The synthesizer partitions
     its composed program graph into explicit connected Modelica units, so independent
     acausal units remain separate equation subgraphs without duplicate Scopes or
     runtime entities.

3. **Vehicle Assembly Defines Topology**:
   - The vehicle assembly layer authors the Kirchhoff pin connections between components:
     ```usda
     def Xform "Battery" (
         prepend references = @lunco://components/power/battery.usda@</Battery>
     ) {}

     def Xform "Motor_FL" (
         prepend references = @lunco://components/mobility/motor.usda@</Motor>
     )
     {
         custom token connectors:p.connect = </SkidRover/Battery.connectors:p>
     }

     # The rover root owns the network collection and boundary.
     over "SkidRover" (
         prepend apiSchemas = ["CollectionAPI:components"]
     )
     {
         uniform token collection:components:expansionRule = "explicitOnly"
         prepend rel collection:components:includes = [
             </SkidRover/Battery>,
             </SkidRover/Motor_FL>,
         ]
     }
     ```

4. **Dynamic Mass Properties & Center of Mass (CoM) Shift**:
   - Components that consume or vent mass (`CryoTank.mo`, `PropellantTank.mo`) publish `output Real mass_kg`.
   - `lunco-cosim` copies `mass_kg` to Avian's generic runtime `Mass` port every step.
   - Avian3D automatically computes the composite Center of Mass ($\mathbf{R}_{\text{CoM}}$) and Moment of Inertia ($I$) shift via Steiner's Parallel Axis Theorem without any hardcoded Rust logic.

5. **Raw Physics vs. Sensor Telemetry Data Pipeline**:
   - **Modelica Physics Equations** read **raw ground-truth data** ($\mathbf{p}_{\text{true}}$, $\mathbf{v}_{\text{true}}$, $T_{\text{true}}$) to solve conservation laws ($\sum i = 0$, $\sum Q = 0$).
   - **Control Algorithms & Flight Software** read **sensor telemetry outputs ONLY** (`IMUSensor`, `ThermalSensor`, `ElectricalSensor`, `StarTracker`, `Altimeter`).

6. **Continuous Math in Modelica; Assembly Policy in Rhai**:
   - Rhai scenario scripts handle **high-level mission events and state switches ONLY** (`wait_for_from("lander_touchdown", "/Mission/Lander")`, `state = "SAFE_MODE"`).
   - Per-tick PID loops, numerical integration, matrix math, and thruster mapping run natively inside **Modelica** (`LunCo.GNC`) or **Rust**. Rhai may decide which authored members are merged into generated Modelica units and where those units/members are placed visually, but it does not read USD or replace continuous equations with a per-tick script.

### 1.1 Mounted parts have one physical owner

A fixed rover part — a battery, photovoltaic deck, lamp, motor housing, or
instrument box — is part of the rover's one rigid body. Reference its reusable
component and give the component mass and collision geometry, but do not apply a
second `PhysicsRigidBodyAPI` to the mounted child. Its colliders and mass belong
to the nearest host body. A part that must move relative to the rover is the
different case: it owns a rigid body **and** the joint that attaches it, in the
same reusable assembly.

This distinction is structural, not visual. A child can remain in the USD tree
while its unjointed body falls away in the first physics step. The
`nested-body-no-joint` lint catches the authoring error, while a drive test must
still verify that mounted descendants preserve their body-relative positions.

### 1.2 Separate asset frame from scene placement

The canonical vehicle frame is Y-up, right-handed, SI metres, with the vehicle
forward direction conventionally along −Z. The asset owns its local geometry and
declares that contract; the scene placement owns the initial heading for a
particular route or point of interest. Do not duplicate a heading in the asset,
a variant, and a wrapper layer.

If a placement can author a rotation, the base prim must declare that rotation
op in `xformOpOrder`; an attribute that is not listed is inert. Keep one
authoritative heading in the strongest scene layer and leave subsequent heading
changes to the vehicle controller. Verify the composed `xformOpOrder` and
`xformOp:rotateXYZ` after composition, not only the source layer.

### 1.3 Stability is an assembled-load property

On low-gravity, high-grip terrain, a high centre of mass can turn a normal
straight launch into a real pitch-over. Treat that as a load-transfer defect
before treating it as solver noise: inspect the wheel contact forces, friction
cone, contact plane, authored `physics:centerOfMass`, mass, and inertia together.
Do not hide a repeatable tip by loosening the acceptance limit or adding
rendering smoothing.

A vehicle acceptance scene should settle, drive under its real control path, and
assert all of the following: measurable travel, bounded tilt, no missing or
detached descendants, enough fixed-step samples, and the scenario's authored
verdict. The same scene can also assert generated electrical outputs, so a rover
that looks correct but has a disconnected panel cannot pass.

### 1.4 Fixed photovoltaic decks use the electrical component contract

Use the shared `components/power/solar_panel.usda` for a fixed panel. The
vehicle owns its area, placement and wiring; the component's fixed collecting
normal is +Y unless the vehicle explicitly overrides it. The component owns the
visual frame/cell surface, mass/collision facet, environment probe and
`SolarPanel.mo` source. A horizontal collecting face uses the component's +Y
normal. Include the panel, battery and driven loads in the same explicit
`CollectionAPI:components` collection on the rover root and connect the panel pin to the
battery pin.

The acceptance is a chain, not a mesh check: the rover-root network must compile,
the authored root boundary must publish `solar_power`/`solar_incidence`, and it
must publish `soc` while receiving current. Read the stable boundary names through
`ReadPorts`; generated child-unit and member-instance names are diagnostic only.

---

## 2. Step-by-Step Build Walkthrough

### Step 1: Compose Reusable Parts
Reference the smallest reusable part definitions. Keep topology in the vehicle assembly:
a six-motor rover references six motor parts and authors six connections. Do not compose a
four-motor network as a base for a six-motor one; topology is an assembly fact, not a
component type.

### Step 2: Group Network Members with CollectionAPI
Apply `CollectionAPI:components` to the network-root prim and explicitly include the actual
part paths. OpenUSD computes membership after references, variants, and other composition
arcs have been resolved.

### Step 3: Reference Generic Components
Reference generic components from `assets/components/`:
```usda
def Scope "Sensors"
{
    def Scope "IMU" (
        prepend apiSchemas = ["LunCoProgramAPI"]
        prepend references = @lunco://components/sensors/imu.usda@</Sensors>
    )
    {
    }
}
```

### Step 4: Wire Netlist Connections
Author `connect` statements connecting component input/output ports.

### Step 5: Add FSW Autopilot Behavior Tree Action
Attach the GNC control loop model from `LunCo.GNC` (`LanderPID.mo`, `ThrusterMapper.mo`, or `PoweredDescentGuidance.mo`).

---

## 3. Supported Vessel Modalities

- **Wheeled Rovers**: USD/Avian motor and reduction powertrain + `DCMotor.mo` electrical facet + `Battery.mo`/`PDU.mo` + Rocker-bogie joints.
- **Powered Descent Landers**: `BellNozzle.mo` + `RCSThruster.mo` + `CryoTank.mo` + `PoweredDescentGuidance.mo` + `LanderPID.mo`.
- **Spider-Rovers (Legged Quadrupeds/Hexapods)**: 12 $\times$ `ServoAxis.mo` + `EncoderSensor.mo` + `TouchdownSensor.mo` footpads + Trot/Crawl gait generator.
- **Lander-Jumpers**: `JumperSpring.mo` + `RCSThruster.mo` + `TouchdownSensor.mo` hopping state machine.
