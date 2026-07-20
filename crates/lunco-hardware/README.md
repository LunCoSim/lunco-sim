# lunco-hardware

Physical actuator and sensor implementations for LunCoSim.

## What This Crate Does

This crate provides concrete implementations of the hardware described in SysML models, bridging the gap between `Port` values and the `avian3d` physics engine.

- **Motor Actuators** — Applies torque to rigid bodies along local axes based on port inputs.
- **Brake Actuators** — Emulates frictional braking by applying velocity damping.
- **Sensors** — Measures physical properties (e.g., Angular Velocity) and writes them back to ports for software consumption.
- **Physics Integration** — Directly interfaces with `avian3d` components (`Forces`, `AngularVelocity`, `LinearVelocity`).

## Architecture

The hardware layer operates in the `FixedUpdate` schedule to ensure deterministic physics interaction.

```
lunco-hardware/
  ├── MotorActuator           — Torque-application component
  ├── BrakeActuator           — Velocity-damping component
  ├── AngularVelocitySensor   — Rotation-measurement component
  └── systems.rs              — Bridge logic between Ports and Avian3D
```

### Hardware-to-Port Linkage

Every hardware component holds an `Entity` reference to its corresponding `Port`. This creates a clean signal layer in the ECS:

```rust
// Software writes to Port -> Hardware reads from Port -> Physics applies Force
commands.spawn((
    MotorActuator {
        port_entity: my_torque_port,
        axis: DVec3::Y,
    },
    Forces::default(),
));
```

## Usage

```rust
app.add_plugins(LunCoHardwarePlugin);

// Spawning a motor
commands.spawn((
    MotorActuator {
        port_entity: torque_port,
        axis: DVec3::X,
    },
    RigidBody::Dynamic,
));
```

## See Also

- `lunco-core` — Defines the `Port` primitive.
- `lunco-cosim` — Propagates values between ports along `SimConnection`s, applying the SSP factor/offset.
- `avian3d` — The underlying physics engine.
