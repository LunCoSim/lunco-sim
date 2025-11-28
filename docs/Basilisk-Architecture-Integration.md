# Basilisk Architecture Integration Guide

## Table of Contents

- [Introduction](#introduction)
- [What is Basilisk?](#what-is-basilisk)
- [Core Basilisk Concepts](#core-basilisk-concepts)
- [LunCoSim's Basilisk-Inspired Architecture](#luncosims-basilisk-inspired-architecture)
- [Effector System Design](#effector-system-design)
- [Integration Roadmap](#integration-roadmap)
- [Best Practices](#best-practices)
- [References](#references)

---

## Introduction

This document describes how LunCoSim's architecture is inspired by NASA's **Basilisk** astrodynamics simulation framework. Understanding this relationship is crucial for future development, especially when integrating LunCoSim (built in Godot) with Basilisk's high-fidelity physics simulations.

**Purpose**: Guide developers in maintaining architectural consistency and facilitate future Basilisk integration.

### Architecture Overview

![Basilisk Architecture Diagram](images/basilisk-architecture-diagram.svg)

*Figure 1: LunCoSim's Basilisk-inspired effector architecture. The Vehicle Hub aggregates contributions from State Effectors (mass, power, inertia) and applies forces from Dynamic Effectors, while Sensors provide measurements for control and telemetry.*

---

## What is Basilisk?

**Basilisk** is an open-source astrodynamics simulation framework developed at the University of Colorado Boulder's Autonomous Vehicle Systems (AVS) Lab. It is designed for spacecraft mission analysis, attitude control, and flight software development.

### Key Characteristics

- **Modular Architecture**: Component-based design with message-passing
- **High-Fidelity Physics**: Accurate orbital mechanics, attitude dynamics, and environmental models
- **Flight Software Integration**: Can run actual flight software algorithms
- **Python + C++**: Python interface with C++ performance-critical modules
- **Effector-Based Design**: Separates state contributions from dynamic effects

### Primary Use Cases

1. Spacecraft mission design and analysis
2. Attitude Determination and Control Systems (ADCS) development
3. Flight software validation
4. Multi-spacecraft simulations
5. Lunar and planetary mission planning

**Official Repository**: [https://github.com/AVSLab/basilisk](https://github.com/AVSLab/basilisk)

---

## Core Basilisk Concepts

### 1. Message-Passing Architecture

Basilisk uses a **message-passing system** where modules communicate through typed messages rather than direct function calls.

```
┌─────────────┐      Message      ┌─────────────┐
│   Module A  │ ───────────────▶  │   Module B  │
│  (Producer) │                   │  (Consumer) │
└─────────────┘                   └─────────────┘
```

**Benefits**:
- Loose coupling between modules
- Easy to add/remove components
- Supports distributed simulations
- Facilitates testing and validation

### 2. State Effectors vs. Dynamic Effectors

Basilisk distinguishes between two types of effectors:

#### State Effectors
- **Passive** components that contribute to vehicle properties
- Provide mass, inertia, center of mass
- Examples: fuel tanks, batteries, solar panels, structural components
- **Do not** actively apply forces/torques

#### Dynamic Effectors
- **Active** components that apply forces and torques
- Compute force/torque contributions each timestep
- Examples: thrusters, reaction wheels, gravity gradient torques
- Modify vehicle motion

### 3. Spacecraft Hub + Effectors Model

```
┌─────────────────────────────────────────┐
│         Spacecraft Hub                  │
│  - Central rigid body                   │
│  - Aggregates mass properties           │
│  - Integrates equations of motion       │
└─────────────────────────────────────────┘
           │
           ├─── State Effector: Fuel Tank
           ├─── State Effector: Battery
           ├─── State Effector: Solar Panel
           ├─── Dynamic Effector: Thruster
           ├─── Dynamic Effector: Reaction Wheel
           └─── Dynamic Effector: Gravity Gradient
```

### 4. Hierarchical State Machines

Basilisk supports hierarchical Finite State Machines (FSMs) for mission sequencing and mode management.

### 5. XTCE Telemetry Standard

Basilisk can export telemetry using the **XML Telemetric and Command Exchange (XTCE)** standard, enabling interoperability with ground systems.

---

## LunCoSim's Basilisk-Inspired Architecture

LunCoSim adapts Basilisk's core concepts to the Godot engine while maintaining compatibility for future integration.

### Architecture Mapping

| Basilisk Concept | LunCoSim Implementation | Location |
|------------------|-------------------------|----------|
| State Effector | `LCStateEffector` | `core/effectors/state-effector.gd` |
| Dynamic Effector | `LCDynamicEffector` | `core/effectors/dynamic-effector.gd` |
| Spacecraft Hub | `LCVehicle` | `core/base/vehicle.gd` |
| Message Passing | Godot Signals + Resource Network | `core/resources/` |
| Telemetry | XTCE-compatible `Telemetry` dict | `LCSpaceSystem.Telemetry` |
| State Machines | `SystemState` + Behaviour Trees | `LCSpaceSystem.SystemState` |

---

## Effector System Design

### State Effector Base Class

**File**: [`core/effectors/state-effector.gd`](../core/effectors/state-effector.gd)

```gdscript
class_name LCStateEffector
extends LCComponent

## Passive components that contribute state properties

signal mass_changed

func get_mass_contribution() -> float:
    return mass

func get_inertia_contribution() -> Vector3:
    return Vector3.ZERO  # Point mass approximation

func get_center_of_mass_offset() -> Vector3:
    return position

func get_power_consumption() -> float:
    return power_consumption

func get_power_production() -> float:
    return power_production
```

**Implementations**:
- `LCFuelTankEffector` - Depleting mass from fuel consumption
- `LCBatteryEffector` - Energy storage and power management
- `LCSolarPanelEffector` - Power generation based on sun angle
- `LCReactionWheelEffector` - Momentum storage (hybrid state/dynamic)
- `LCResourceTankEffector` - Generic resource storage

### Dynamic Effector Base Class

**File**: [`core/effectors/dynamic-effector.gd`](../core/effectors/dynamic-effector.gd)

```gdscript
class_name LCDynamicEffector
extends LCComponent

## Active components that apply forces and torques

func compute_force_torque(delta: float) -> Dictionary:
    return {
        "force": Vector3.ZERO,      # Global frame
        "torque": Vector3.ZERO,     # Global frame
        "position": global_position # Application point
    }

func local_to_global_force(local_force: Vector3) -> Vector3:
    return global_transform.basis * local_force

func local_to_global_torque(local_torque: Vector3) -> Vector3:
    return global_transform.basis * local_torque
```

**Implementations**:
- `LCThrusterEffector` - Thrust forces with fuel depletion
- `LCWheelEffector` - Ground vehicle wheel torques (hybrid)

### Vehicle Hub

**File**: [`core/base/vehicle.gd`](../core/base/vehicle.gd)

The `LCVehicle` class acts as the **Spacecraft Hub**, aggregating effector contributions:

```gdscript
class_name LCVehicle
extends VehicleBody3D

# Effector lists (auto-discovered)
var state_effectors: Array = []
var dynamic_effectors: Array = []

# Aggregated properties
var total_mass: float = 0.0
var power_consumption: float = 0.0
var power_production: float = 0.0

func _ready():
    _discover_effectors()
    _update_mass_properties()
    _manage_power_system(0.0)

func _physics_process(delta):
    if mass_properties_dirty:
        _update_mass_properties()
    
    _manage_power_system(delta)
    _apply_effector_forces(delta)
    _apply_reaction_wheel_torques()
```

**Key Methods**:

1. **`_discover_effectors()`**: Automatically finds all effector children
2. **`_update_mass_properties()`**: Aggregates mass and center of mass
3. **`_manage_power_system()`**: Handles power production/consumption
4. **`_apply_effector_forces()`**: Applies forces from dynamic effectors
5. **`_update_telemetry()`**: Updates XTCE-compatible telemetry

### Sensor Effectors

**File**: [`core/effectors/sensor-effector.gd`](../core/effectors/sensor-effector.gd)

Sensors are specialized state effectors that provide measurements:

```gdscript
class_name LCSensorEffector
extends LCStateEffector

@export var update_rate: float = 10.0  # Hz
@export var add_noise: bool = true
@export var add_bias: bool = false

var measurement: Variant = null
var is_valid: bool = false

func _update_measurement():
    pass  # Override in subclasses
```

**Implementations**:
- `LCIMUEffector` - Inertial Measurement Unit (accelerometer + gyroscope)
- `LCLidarEffector` - Lidar range finder
- `LCCameraEffector` - Vision sensor with object detection
- `LCGPSEffector` - Global positioning

---

## Integration Roadmap

### Phase 1: Current State ✅

- [x] Effector-based architecture implemented
- [x] State and dynamic effectors separated
- [x] Vehicle hub aggregates mass properties
- [x] Power system with batteries and solar panels
- [x] XTCE-compatible telemetry structure
- [x] Resource network for material flow

### Phase 2: Enhanced Compatibility 🚧

- [ ] **Message-Passing Layer**: Implement Basilisk-style message system
  - Create `LCMessage` base class
  - Add message queues to effectors
  - Support typed message passing between modules

- [ ] **Basilisk Export/Import**: 
  - Export LunCoSim vehicle configurations to Basilisk JSON
  - Import Basilisk simulation results for visualization
  - Bidirectional data exchange format

- [ ] **Coordinate Frame Consistency**:
  - Align coordinate frames (Basilisk uses N-frame, B-frame, etc.)
  - Implement frame transformation utilities
  - Document frame conventions

### Phase 3: Direct Integration 🔮

- [ ] **Basilisk Co-Simulation**:
  - Run Basilisk as external process
  - Exchange state via IPC or sockets
  - LunCoSim provides visualization, Basilisk provides physics

- [ ] **GDExtension Bridge**:
  - Create GDExtension wrapper for Basilisk C++ modules
  - Call Basilisk physics directly from Godot
  - Real-time high-fidelity simulation

- [ ] **Flight Software Testing**:
  - Run actual flight software algorithms in LunCoSim
  - Validate FSW using Basilisk's proven models
  - Hardware-in-the-loop (HIL) simulation support

### Phase 4: Advanced Features 🌟

- [ ] **Multi-Vehicle Simulations**:
  - Spacecraft formations
  - Rendezvous and docking
  - Distributed mission scenarios

- [ ] **Environmental Models**:
  - Gravity harmonics (J2, J3, etc.)
  - Atmospheric drag
  - Solar radiation pressure
  - Third-body perturbations

- [ ] **Attitude Control Algorithms**:
  - PID controllers
  - MRP (Modified Rodrigues Parameters) feedback
  - Momentum management strategies

---

## Best Practices

### 1. Maintain Effector Separation

> [!IMPORTANT]
> Always distinguish between **state** and **dynamic** effectors. A component should inherit from one or the other, not both (except for special cases like `LCWheelEffector`).

**State Effector**: Contributes properties (mass, power, inertia)
**Dynamic Effector**: Applies forces/torques

### 2. Use Signals for State Changes

When an effector's state changes (e.g., fuel depleted, battery charged), emit signals:

```gdscript
signal mass_changed
signal power_state_changed
signal resource_depleted

func deplete_fuel(amount: float):
    current_fuel -= amount
    mass_changed.emit()
```

This allows the vehicle hub to update aggregated properties efficiently.

### 3. Implement Telemetry Consistently

All components should follow the XTCE telemetry pattern:

```gdscript
@export_category("XTCE")
@export var Telemetry: Dictionary = {}
@export var Parameters: Dictionary = {}
@export var Commands: Dictionary = {}

func _initialize_telemetry():
    Telemetry = {
        "property1": value1,
        "property2": value2,
    }

func _update_telemetry():
    Telemetry["property1"] = current_value1
    Telemetry["property2"] = current_value2
```

### 4. Use Dirty Flags for Performance

Avoid recalculating properties every frame:

```gdscript
var mass_properties_dirty: bool = true

func _on_effector_mass_changed():
    mass_properties_dirty = true

func _physics_process(delta):
    if mass_properties_dirty:
        _update_mass_properties()
```

### 5. Document Frame Conventions

Always document which coordinate frame you're using:

```gdscript
## Returns thrust force in GLOBAL frame
func compute_force_torque(delta: float) -> Dictionary:
    var local_thrust = thrust_direction * current_thrust
    var global_thrust = local_to_global_force(local_thrust)
    
    return {
        "force": global_thrust,  # Global frame
        "position": global_position
    }
```

### 6. Design for Modularity

Components should be **self-contained** and **reusable**:

- Minimal dependencies on other components
- Clear interfaces (methods, signals)
- Configurable via exported properties
- Work in isolation for testing

---

## References

### Basilisk Resources

- **Official Website**: [https://hanspeterschaub.info/basilisk/](https://hanspeterschaub.info/basilisk/)
- **GitHub Repository**: [https://github.com/AVSLab/basilisk](https://github.com/AVSLab/basilisk)
- **Documentation**: [https://avslab.github.io/basilisk/](https://avslab.github.io/basilisk/)
- **Academic Papers**: Search "Basilisk astrodynamics" on Google Scholar

### LunCoSim Architecture

- [LunCo Architecture](LunCo-Architecture.md)
- [Model Based Systems Engineering](SubSystems/Model%20Based%20Systems%20Engineering.md)

### Key Files

- [`core/effectors/state-effector.gd`](../core/effectors/state-effector.gd)
- [`core/effectors/dynamic-effector.gd`](../core/effectors/dynamic-effector.gd)
- [`core/base/vehicle.gd`](../core/base/vehicle.gd)
- [`core/base/space-system.gd`](../core/base/space-system.gd)

### Standards

- **XTCE**: [XML Telemetric and Command Exchange](https://www.omg.org/spec/XTCE/)
- **CCSDS**: [Consultative Committee for Space Data Systems](https://public.ccsds.org/)

---

## Appendix: Effector Comparison Table

| Feature | Basilisk | LunCoSim | Notes |
|---------|----------|----------|-------|
| State Effectors | ✅ | ✅ | Mass, inertia, power contributions |
| Dynamic Effectors | ✅ | ✅ | Force/torque application |
| Message Passing | ✅ | 🚧 | Using Godot signals currently |
| XTCE Telemetry | ✅ | ✅ | Compatible dictionary structure |
| Resource Network | ❌ | ✅ | LunCoSim extension for ISRU |
| Sensor Models | ✅ | ✅ | IMU, GPS, Camera, Lidar |
| Reaction Wheels | ✅ | ✅ | Momentum storage and saturation |
| Thrusters | ✅ | ✅ | Fuel depletion and thrust vectoring |
| Solar Panels | ✅ | ✅ | Sun angle-dependent power |
| Batteries | ✅ | ✅ | Charge/discharge cycles |
| Gravity Models | ✅ | 🚧 | Planned for Phase 3 |
| Atmospheric Drag | ✅ | 🚧 | Planned for Phase 3 |
| Multi-body Dynamics | ✅ | 🚧 | Planned for Phase 4 |

**Legend**: ✅ Implemented | 🚧 Planned | ❌ Not applicable

---

## Contributing

When adding new effectors or modifying the architecture:

1. **Follow the effector pattern**: Inherit from `LCStateEffector` or `LCDynamicEffector`
2. **Implement required methods**: `get_mass_contribution()`, `compute_force_torque()`, etc.
3. **Add telemetry**: Follow XTCE conventions
4. **Document frame conventions**: Specify local vs. global frames
5. **Test in isolation**: Create test scenes for new components
6. **Update this document**: Keep the integration roadmap current

---

**Last Updated**: 2025-11-28  
**Maintainer**: LunCoSim Development Team  
**License**: MIT
