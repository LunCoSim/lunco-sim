# LunCoSim Reusable Component & Model Index

This document is the canonical reference index for all reusable **Modelica physics models** (`assets/models/LunCo/`) and **USD component assets** (`assets/components/`) in LunCoSim.

---

## 1. Modelica Physics Package (`LunCo.*`)

All physical equations, conservation laws, and component dynamics live in `assets/models/LunCo/`.

### 1.1 Electrical Power Subsystem (`LunCo.Electrical`)
- **[Pin.mo](../assets/models/LunCo/Electrical/Pin.mo)**: Acausal electrical pin connector (`Real v; flow Real i;`, enforcing Kirchhoff's Current Law $\sum i = 0$).
- **[Battery.mo](../assets/models/LunCo/Electrical/Battery.mo)**: Pack capacity, fixed authored initial state, bounded State-of-Charge integration, internal-resistance voltage sag ($V = V_{\text{nom}} + I R$ with the signed pin-current convention), and a 0.1% usable-storage reserve signal for the empty-boundary event.
- **[SolarPanel.mo](../assets/models/LunCo/Electrical/SolarPanel.mo)**: Triple-junction solar cell array power generation ($P_{\text{solar}} = \text{area} \cdot \eta \cdot \Phi_{\text{sun}}$).
- **[DCMotor.mo](../assets/models/LunCo/Electrical/DCMotor.mo)**: Electrical facet of a USD/Avian hub drive; demand-controlled bus current, electrical draw, and winding heat. The shaft torque, speed, reduction, and contact mechanics remain owned by the USD/Avian drivetrain.
- **[PDU.mo](../assets/models/LunCo/Electrical/PDU.mo)**: EPS Power Distribution Unit, 28V regulated main bus, and under-voltage load shedding.
- **[OnboardComputer.mo](../assets/models/LunCo/Electrical/OnboardComputer.mo)**: Flight computer baseline power draw ($P_{\text{base}} = 12\text{ W}$) + active GNC processing load ($P_{\text{gnc}} = 8\text{ W}$).
- **[CameraPayload.mo](../assets/models/LunCo/Electrical/CameraPayload.mo)**: Active camera capture streaming power draw ($4.5\text{ W}$) and data output rate ($15\text{ Mbps}$).
- **[HeadlightController.mo](../assets/models/LunCo/Electrical/HeadlightController.mo)**: Rhai-commanded enable clamp, USD-facing luminous output, and solved electrical load for a reusable vehicle headlight.

### 1.2 Thermal Control Subsystem (`LunCo.Thermal`)
- **[HeatPort.mo](../assets/models/LunCo/Thermal/HeatPort.mo)**: Acausal thermal connector (`Real T; flow Real Q;`, enforcing $\sum Q = 0$).
- **[ThermalMass.mo](../assets/models/LunCo/Thermal/ThermalMass.mo)**: Structural lumped thermal capacity ($C_{\text{th}} \frac{dT}{dt} = \sum Q$).
- **[Radiator.mo](../assets/models/LunCo/Thermal/Radiator.mo)**: Vacuum radiative heat rejection ($Q_{\text{rad}} = \sigma \epsilon A (T^4 - T_{\text{sink}}^4)$).
- **[ThermalConductor.mo](../assets/models/LunCo/Thermal/ThermalConductor.mo)**: Linear thermal conduction ($Q = G (T_1 - T_2)$).
- **[ThermostatHeater.mo](../assets/models/LunCo/Thermal/ThermostatHeater.mo)**: Thermo-electrical survival heater drawing EPS bus power to keep optics/batteries warm ($T < 263\text{ K}$).

### 1.3 Sensor & Instrument Subsystem (`LunCo.Sensors`)
- **[IMUSensor.mo](../assets/models/LunCo/Sensors/IMUSensor.mo)**: 3-axis accelerometer bias ($\mathbf{b}_a$), gyro drift ($\mathbf{b}_\omega$), scale factor error, and health status flag.
- **[ThermalSensor.mo](../assets/models/LunCo/Sensors/ThermalSensor.mo)**: RTD/thermocouple response lag ($\tau = 2\text{ s}$), calibration offset ($\Delta T_{\text{cal}}$), and 12-bit ADC counts ($0..4095$).
- **[ElectricalSensor.mo](../assets/models/LunCo/Sensors/ElectricalSensor.mo)**: Voltage divider attenuation, Hall-effect current transducer sensitivity ($0.05\text{ V/A}$), and 12-bit ADC counts.
- **[StarTracker.mo](../assets/models/LunCo/Sensors/StarTracker.mo)**: Boresight attitude determination, Sun exclusion mask angle ($\ge 30^\circ$), rate blinding, and attitude lock flag.
- **[Altimeter.mo](../assets/models/LunCo/Sensors/Altimeter.mo)**: Altimeter radar/laser rangefinder, mount offset ($1.2\text{ m}$), max range mask ($2500\text{ m}$), and out-of-range flag.
- **[EncoderSensor.mo](../assets/models/LunCo/Sensors/EncoderSensor.mo)**: Rotary encoder pulses per revolution (4096 PPR), zero-point offset, and digital telemetry output.
- **[TouchdownSensor.mo](../assets/models/LunCo/Sensors/TouchdownSensor.mo)**: Landing leg strut reaction force threshold switch ($F_{\text{thresh}} = 200\text{ N}$) triggering engine cutoff on touchdown.

### 1.4 Guidance, Navigation & Control (`LunCo.GNC`)
- **[PoweredDescentGuidance.mo](../assets/models/LunCo/GNC/PoweredDescentGuidance.mo)**: Apollo P63/P64 E-Guidance algorithm for precision powered landing trajectory generation.
- **[GravityTurnGuidance.mo](../assets/models/LunCo/GNC/GravityTurnGuidance.mo)**: Retrograde velocity vector alignment for high-speed atmospheric/orbital braking.
- **[ThrusterMapper.mo](../assets/models/LunCo/GNC/ThrusterMapper.mo)**: RCS thruster command allocation matrix translating 3D torque/force demands into PWM duty cycles.
- **[LanderPID.mo](../assets/models/LunCo/GNC/LanderPID.mo)**: Continuous attitude rate and vertical descent PID feedback controller in Modelica.

### 1.5 Propulsion & Pointing (`LunCo.Propulsion` / `LunCo.Pointing`)
- **[RCSThruster.mo](../assets/models/LunCo/Propulsion/RCSThruster.mo)**: RCS attitude pulse thruster ($F = u \cdot F_{\text{nom}}$, mass flow rate $\dot{m} = \frac{F}{I_{\text{sp}} g_0}$).
- **[BellNozzle.mo](../assets/models/LunCo/Propulsion/BellNozzle.mo)**: Main lander descent engine thrust and mass flow dynamics.
- **[ReactionWheel.mo](../assets/models/LunCo/Pointing/ReactionWheel.mo)**: Reaction wheel angular momentum storage ($h = I \omega$), reaction torque, and electrical power draw.

### 1.6 Storage Subsystem (`LunCo.Storage`)
- **[CryoTank.mo](../assets/models/LunCo/Storage/CryoTank.mo)**: Cryogenic propellant storage tank (boil-off rate $\dot{m}_{\text{boil}} = \frac{Q_{\text{in}}}{h_{\text{fg}}}$ and mass output `mass_kg` driving dynamic CoM and inertia tensor shifts).
- **[MassMemory.mo](../assets/models/LunCo/Storage/MassMemory.mo)**: Solid-state flash science memory buffer (GB fill, write/read power draw).

### 1.7 Communications Subsystem (`LunCo.Comms`)
- **[Transmitter.mo](../assets/models/LunCo/Comms/Transmitter.mo)**: RF transmitter power draw on EPS bus & radiated RF output.
- **[DataBuffer.mo](../assets/models/LunCo/Comms/DataBuffer.mo)**: Telemetry data storage buffer dynamics.

---

## 2. USD Reusable Component Assets (`assets/components/`)

This is the shipped tree. A Modelica class in section 1 is not automatically a
USD component: it becomes placeable/composable only when a file below projects
its program, ports, mass/geometry and mounting contract. Do not infer a wrapper
from the existence of a `.mo` file.

```
assets/components/
├── avionics/
│   ├── obc.usda
│   └── obc_lander.usda
├── cameras/
│   ├── lunar_surface_camera.usda
│   ├── rover_front_camera.usda
│   └── visual_review_camera.usda
├── comms/
│   ├── antenna.usda
│   ├── ground_station.usda
│   ├── link_beam.usda
│   ├── radio.usda
│   ├── transmitter_power.usda
│   └── wifi_radio.usda
├── environment/
│   ├── probe.usda
│   └── starfield_sky.usda
├── gnc/
│   ├── descent_guidance.usda
│   ├── landing_target.usda
│   └── position_pid_guidance.usda
├── lights/
│   ├── headlight.usda
│   └── headlight_controller.usda
├── mobility/
│   ├── chassis/box_chassis.usda
│   ├── drive_laws/{modelica_ackermann,modelica_six_independent,modelica_skid}.usda
│   ├── gearbox.usda
│   ├── motor.usda
│   ├── motors/{fast,lunokhod,standard,torque}.usda
│   ├── physical_drivetrain.usda
│   ├── suspensions/{rigid,rocker,standard}.usda
│   ├── tires/{bald,cleated,hard,regolith,worn}.usda
│   └── wheel.usda
├── mounting/
│   └── demo_probe.usda
├── payload/
│   └── science_camera.usda
├── power/
│   ├── battery.usda
│   ├── ideal_voltage_source.usda
│   ├── power_bus.usda
│   └── solar_panel.usda
├── terrain/
│   └── rocker_bogie_articulation_course.usda
├── thermal/
│   └── motor_thermal.usda
```

`power_bus.usda` is a passive USD bus/membership node; it is not a wrapper for
`LunCo.Electrical.PDU`. `descent_guidance.usda` is the shipped powered-descent
guidance overlay; there is no duplicate `powered_descent_guidance.usda` spelling.

The following library models currently have no standalone USD component wrapper:
`LunCo.Thermal.Radiator`, `LunCo.Thermal.ThermostatHeater`,
`LunCo.Storage.CryoTank`, `LunCo.Storage.MassMemory`,
`LunCo.Comms.Transmitter`, `LunCo.Pointing.ReactionWheel`, and
`LunCo.Propulsion.RCSThruster`. Some are instantiated inside larger authored
assemblies. They must not be advertised as independently placeable parts until
their USD mounting, network membership, physics projection and production
verdicts exist.
