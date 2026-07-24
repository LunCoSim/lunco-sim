# 55. Building Vessels, Rovers, and Landers: Architectural Guide

This document is the step-by-step architectural guide for assembling mission-grade space vessels (wheeled rovers, powered descent landers, spider-rovers, and satellites) in LunCoSim using the 3-plane modular architecture.

---

## 1. Core Architectural Principles

When building any vehicle assembly in LunCoSim:

1. **Decoupled Component References**:
   - Component files in `assets/components/` (`battery.usda`, `motor.usda`, `cryo_tank.usda`) contain **zero vehicle-specific paths** (`/SkidRover` or `/Power`).
   - Component files define reusable part interfaces and nameplate defaults. The vehicle
     file references those parts and owns its actual topology.

2. **Ordinary USD Scopes and Standard Collections for Networks**:
   - Every independently solved physical network has a named `Scope` applying
     `CollectionAPI:components`.
   - The collection includes the actual assembled part prims; it does not create proxy
     copies below the Scope.
   - Runtime Rust projection reads the composed stage and emits the transient Modelica wrapper. Rhai does not synthesize equations.
   - One collection must contain one connected acausal island. A disconnected island
     is another compilation and failure domain, so it gets another named Scope.

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

     def Scope "Electrical" (
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

6. **Zero Math in Rhai**:
   - Rhai scenario scripts handle **high-level mission events and state switches ONLY** (`wait_for("touchdown")`, `state = "SAFE_MODE"`).
   - Per-tick PID loops, numerical integration, matrix math, and thruster mapping run natively inside **Modelica** (`LunCo.GNC`) or **Rust**.

---

## 2. Step-by-Step Build Walkthrough

### Step 1: Compose Reusable Parts
Reference the smallest reusable part definitions. Keep topology in the vehicle assembly:
a six-motor rover references six motor parts and authors six connections. Do not compose a
four-motor network as a base for a six-motor one; topology is an assembly fact, not a
component type.

### Step 2: Group Network Members with CollectionAPI
Apply `CollectionAPI:components` to the network Scope and explicitly include the actual
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

- **Wheeled Rovers**: `DCMotor.mo` + `Battery.mo` + `PDU.mo` + Rocker-bogie joints.
- **Powered Descent Landers**: `BellNozzle.mo` + `RCSThruster.mo` + `CryoTank.mo` + `PoweredDescentGuidance.mo` + `LanderPID.mo`.
- **Spider-Rovers (Legged Quadrupeds/Hexapods)**: 12 $\times$ `ServoAxis.mo` + `EncoderSensor.mo` + `TouchdownSensor.mo` footpads + Trot/Crawl gait generator.
- **Lander-Jumpers**: `JumperSpring.mo` + `RCSThruster.mo` + `TouchdownSensor.mo` hopping state machine.
