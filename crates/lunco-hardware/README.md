# lunco-hardware

Physical actuator and sensor implementations for LunCoSim.

## What This Crate Does

This crate provides generic steering and sensor interfaces between authored
ports and the `avian3d` physics engine. Electrical and mechanical motor
equations are authored in USD/Modelica and their solved torque is projected by
`lunco-cosim`.

- **Brake Actuators** — Emulates frictional braking by applying velocity damping.
- **Sensors** — Measures physical properties (e.g., Angular Velocity) and writes them back to ports for software consumption.
- **Physics Integration** — Directly interfaces with `avian3d` components (`Forces`, `AngularVelocity`, `LinearVelocity`).

## Architecture

The hardware layer operates in the `FixedUpdate` schedule to ensure deterministic physics interaction.

```
lunco-hardware/
  ├── AngularVelocitySensor   — Rotation-measurement component
  └── systems.rs              — Bridge logic between Ports and Avian3D
```

## Usage

```rust
app.add_plugins(LunCoHardwarePlugin);

// Hardware components are generic port/sensor boundaries. A mechanical
// actuator is authored by the connected Modelica/Rhai network.
commands.spawn((AngularVelocitySensor::default(), RigidBody::Dynamic));
```

## See Also

- `lunco-core` — Defines the `Port` primitive.
- `lunco-cosim` — Propagates values between ports along `SimConnection`s, applying the SSP factor/offset.
- `avian3d` — The underlying physics engine.
