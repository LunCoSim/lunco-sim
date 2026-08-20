# lunco-controller

Input mapping and controller translation for LunCoSim vessels.

## What This Crate Does

This crate translates raw user input (Keyboard, Gamepad, Mouse) into typed command events that the Flight Software (FSW) can consume.

- **Input Mapping** — Standard WASD + F/Space mapping for rovers and astronauts via `leafwing-input-manager`; F is autopilot action and Space is a vehicle brake where authored.
- **Intent Translation** — Translates abstract human actions (e.g., `DriveForward`) into the shared `SetPorts` command surface.
- **Control Latching** — Implements "cruise control" behavior (`Shift + Axis`) to toggle sticky setpoints for hands-free operation.
- **Context Awareness** — Supports modifier-gated input (e.g., `Ctrl` for free-look camera mode) to prevent command flow during inspection.

## Architecture

The controller acts as the **Human-Machine Interface (HMI)** layer, decoupling raw HID events from simulation logic.

```
lunco-controller/
  ├── VesselIntent      — Enum of abstract actions (Drive, Steer, Brake)
  ├── VesselIntentState — Action state resource for polling
  ├── ControllerLink    — Component linking a controller entity to a vessel
  └── systems.rs        — Translation logic and latching state
```

### The Latch Pattern

To facilitate long-distance surface travel, the controller supports latched axis setpoints:
- `Shift + W/S`: Toggles forward/reverse throttle latch.
- `Shift + A/D`: Toggles steering lock.
- `Space (Brake)`: Drives the possessed vehicle's authored brake input; it does not toggle autopilot.

## Usage

```rust
app.add_plugins(LunCoControllerPlugin);

// Assign a controller to a rover
commands.spawn((
    InputManagerBundle::<VesselIntent> {
        action_state: ActionState::default(),
        input_map: get_default_input_map(),
    },
    ControllerLink { vessel_entity: rover_id },
));
```

## See Also

- `lunco-mobility` — Consumes the bound `SetPorts` inputs and projects them onto the authored actuator ports.
- `lunco-core` — Defines standard `UserIntent` used for avatar navigation.
