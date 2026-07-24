# LunCoSim Reusable Component & Model Index

This document is the canonical reference index for all reusable **Modelica physics models** (`assets/models/LunCo/`) and **USD component assets** (`assets/components/`) in LunCoSim.

---

## 1. Modelica Physics Package (`LunCo.*`)

All physical equations, conservation laws, and component dynamics live in `assets/models/LunCo/`.

### 1.1 Electrical Power Subsystem (`LunCo.Electrical`)
- **[Pin.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Electrical/Pin.mo)**: Acausal electrical pin connector (`Real v; flow Real i;`, enforcing Kirchhoff's Current Law $\sum i = 0$).
- **[Battery.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Electrical/Battery.mo)**: Pack capacity, internal resistance voltage sag ($V = V_{\text{nom}} - I R$), and State-of-Charge integration ($\frac{d(soc)}{dt}$).
- **[SolarPanel.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Electrical/SolarPanel.mo)**: Triple-junction solar cell array power generation ($P_{\text{solar}} = \text{area} \cdot \eta \cdot \Phi_{\text{sun}}$).
- **[DCMotor.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Electrical/DCMotor.mo)**: Hub drive motor ($P_{\text{mech}} = \tau \cdot \omega$, electrical current draw $I = \frac{P_{\text{mech}}}{\eta \cdot V_{\text{bus}}}$).
- **[PDU.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Electrical/PDU.mo)**: EPS Power Distribution Unit, 28V regulated main bus, and under-voltage load shedding.
- **[OnboardComputer.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Electrical/OnboardComputer.mo)**: Flight computer baseline power draw ($P_{\text{base}} = 12\text{ W}$) + active GNC processing load ($P_{\text{gnc}} = 8\text{ W}$).
- **[CameraPayload.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Electrical/CameraPayload.mo)**: Active camera capture streaming power draw ($4.5\text{ W}$) and data output rate ($15\text{ Mbps}$).

### 1.2 Thermal Control Subsystem (`LunCo.Thermal`)
- **[HeatPort.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Thermal/HeatPort.mo)**: Acausal thermal connector (`Real T; flow Real Q;`, enforcing $\sum Q = 0$).
- **[ThermalMass.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Thermal/ThermalMass.mo)**: Structural lumped thermal capacity ($C_{\text{th}} \frac{dT}{dt} = \sum Q$).
- **[Radiator.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Thermal/Radiator.mo)**: Vacuum radiative heat rejection ($Q_{\text{rad}} = \sigma \epsilon A (T^4 - T_{\text{sink}}^4)$).
- **[ThermalConductor.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Thermal/ThermalConductor.mo)**: Linear thermal conduction ($Q = G (T_1 - T_2)$).
- **[ThermostatHeater.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Thermal/ThermostatHeater.mo)**: Thermo-electrical survival heater drawing EPS bus power to keep optics/batteries warm ($T < 263\text{ K}$).

### 1.3 Sensor & Instrument Subsystem (`LunCo.Sensors`)
- **[IMUSensor.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Sensors/IMUSensor.mo)**: 3-axis accelerometer bias ($\mathbf{b}_a$), gyro drift ($\mathbf{b}_\omega$), scale factor error, and health status flag.
- **[ThermalSensor.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Sensors/ThermalSensor.mo)**: RTD/thermocouple response lag ($\tau = 2\text{ s}$), calibration offset ($\Delta T_{\text{cal}}$), and 12-bit ADC counts ($0..4095$).
- **[ElectricalSensor.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Sensors/ElectricalSensor.mo)**: Voltage divider attenuation, Hall-effect current transducer sensitivity ($0.05\text{ V/A}$), and 12-bit ADC counts.
- **[StarTracker.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Sensors/StarTracker.mo)**: Boresight attitude determination, Sun exclusion mask angle ($\ge 30^\circ$), rate blinding, and attitude lock flag.
- **[Altimeter.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Sensors/Altimeter.mo)**: Altimeter radar/laser rangefinder, mount offset ($1.2\text{ m}$), max range mask ($2500\text{ m}$), and out-of-range flag.
- **[EncoderSensor.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Sensors/EncoderSensor.mo)**: Rotary encoder pulses per revolution (4096 PPR), zero-point offset, and digital telemetry output.
- **[TouchdownSensor.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Sensors/TouchdownSensor.mo)**: Landing leg strut reaction force threshold switch ($F_{\text{thresh}} = 200\text{ N}$) triggering engine cutoff on touchdown.

### 1.4 Guidance, Navigation & Control (`LunCo.GNC`)
- **[PoweredDescentGuidance.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/GNC/PoweredDescentGuidance.mo)**: Apollo P63/P64 E-Guidance algorithm for precision powered landing trajectory generation.
- **[GravityTurnGuidance.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/GNC/GravityTurnGuidance.mo)**: Retrograde velocity vector alignment for high-speed atmospheric/orbital braking.
- **[ThrusterMapper.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/GNC/ThrusterMapper.mo)**: RCS thruster command allocation matrix translating 3D torque/force demands into PWM duty cycles.
- **[LanderPID.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/GNC/LanderPID.mo)**: Continuous attitude rate and vertical descent PID feedback controller in Modelica.

### 1.5 Propulsion & Pointing (`LunCo.Propulsion` / `LunCo.Pointing`)
- **[RCSThruster.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Propulsion/RCSThruster.mo)**: RCS attitude pulse thruster ($F = u \cdot F_{\text{nom}}$, mass flow rate $\dot{m} = \frac{F}{I_{\text{sp}} g_0}$).
- **[BellNozzle.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Propulsion/BellNozzle.mo)**: Main lander descent engine thrust and mass flow dynamics.
- **[ReactionWheel.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Pointing/ReactionWheel.mo)**: Reaction wheel angular momentum storage ($h = I \omega$), reaction torque, and electrical power draw.

### 1.6 Storage Subsystem (`LunCo.Storage`)
- **[CryoTank.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Storage/CryoTank.mo)**: Cryogenic propellant storage tank (boil-off rate $\dot{m}_{\text{boil}} = \frac{Q_{\text{in}}}{h_{\text{fg}}}$ and mass output `mass_kg` driving dynamic CoM and inertia tensor shifts).
- **[MassMemory.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Storage/MassMemory.mo)**: Solid-state flash science memory buffer (GB fill, write/read power draw).

### 1.7 Communications Subsystem (`LunCo.Comms`)
- **[Transmitter.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Comms/Transmitter.mo)**: RF transmitter power draw on EPS bus & radiated RF output.
- **[DataBuffer.mo](file:///home/rod/Documents/luncosim-workspace/tutorials/assets/models/LunCo/Comms/DataBuffer.mo)**: Telemetry data storage buffer dynamics.

---

## 2. USD Reusable Component Assets (`assets/components/`)

All component assets inside `assets/components/` are **decoupled, generic USD sub-layers** that carry no vehicle-specific hardcoded paths.

```
assets/components/
├── power/
│   ├── battery.usda              # Generic Traction & Auxiliary Battery Pack (LunCo.Electrical.Battery)
│   ├── solar_panel.usda          # Generic Solar Cell Array (LunCo.Electrical.SolarPanel)
│   └── power_bus.usda            # Generic EPS Power Distribution Unit (LunCo.Electrical.PDU)
├── mobility/
│   └── motor.usda                # Generic Hub Drive Motor (LunCo.Electrical.DCMotor)
├── thermal/
│   ├── radiator.usda             # Generic Vacuum Radiative Cooling (LunCo.Thermal.Radiator)
│   └── thermostat_heater.usda    # Generic Survival Heater (LunCo.Thermal.ThermostatHeater)
├── storage/
│   ├── cryo_tank.usda            # Generic Propellant Storage Tank (LunCo.Storage.CryoTank)
│   └── mass_memory.usda          # Generic NVRAM Science Flash Buffer (LunCo.Storage.MassMemory)
├── comms/
│   └── transmitter.usda          # Generic RF Telemetry Transmitter (LunCo.Comms.Transmitter)
├── pointing/
│   └── reaction_wheel.usda       # Generic Attitude Reaction Wheel (LunCo.Pointing.ReactionWheel)
├── propulsion/
│   └── rcs_thruster.usda         # Generic RCS Attitude Pulse Thruster (LunCo.Propulsion.RCSThruster)
└── gnc/
    └── powered_descent_guidance.usda # Generic Precision EDL Guidance (LunCo.GNC.PoweredDescentGuidance)
```
