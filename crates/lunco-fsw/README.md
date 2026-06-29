# lunco-fsw

Flight Software (FSW) scaffolding for LunCoSim vessels.

> **Status: early stub (~100 lines).** The "Command Fabric" / decentralized
> control architecture below is the *intended* design, not what is built today.
> What actually exists is a plugin, two components, and a fallback observer.

## What exists today

- **`LunCoFswPlugin`** — registers a single fallback observer
  (`unrecognized_command_handler`) for the `UnrecognizedCommand` event.
- **`FlightSoftware`** — a per-vessel component holding:
  - `port_map: HashMap<String, Entity>` — maps mnemonic strings
    (e.g. `"thruster_main"`) to the ECS entity that represents the hardware,
    so software refers to ports by SysML name instead of hardcoded entity ids.
  - `brake_active: bool` — global override flag for drive commands.
- **`VesselSubsystem`** — marker component for an autonomous functional unit.
- **`UnrecognizedCommand`** — event captured by the fallback handler (NACK
  telemetry is a TODO).

The commented-out test module and centralized NACK logging are not implemented.

## Usage

```rust
app.add_plugins(LunCoFswPlugin);

commands.spawn((
    FlightSoftware {
        port_map: [("drive_left".into(), port_entity)].into(),
        ..default()
    },
    VesselSubsystem,
));
```

## Roadmap (aspirational)

The goal is a **decentralized subsystem** architecture mirroring real aerospace
hardware: subsystems (GNC, Power, Mobility) as independent ECS entities that
register their own observers, communicating via asynchronous command messages,
with the `port_map` decoupling semantic logic from physical hardware for digital
twin mirroring across vehicle manifests. Most of this is not yet wired.

## See Also

- `lunco-obc` — signal processing (DAC/ADC) between FSW and hardware.
- `lunco-mobility` — high-level mobility observers (`DriveRover`, `BrakeRover`).
