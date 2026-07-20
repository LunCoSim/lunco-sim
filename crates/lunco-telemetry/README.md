# lunco-telemetry

Reflection-based data extraction and telemetry monitoring for LunCoSim.

## What This Crate Does

This crate implements the simulation's **"Optical Fibers"**—a generic, "No-Code" telemetry bridge for extracting data out of the simulation world.

- **Automated Sampling** — High-frequency sampling of internal physics and software values.
- **Reflection-Based Extraction** — Uses Bevy's `Reflect` capabilities to drill into components via string paths (e.g., `"Port.value"`).
- **Unified Transport Format** — Maps heterogeneous Rust types into a standardized `TelemetryValue` (F64, I64, Bool, String).
- **Headless Monitoring** — Provides the primary "eyes-and-ears" for simulations running without a GPU.
- **Mission Control Bridge** — Facilitates broadcasting data to external Mission Control systems (YAMCS, XTCE).

## Architecture

The telemetry layer leverages Bevy's `AppTypeRegistry` to discover and extract data at runtime.

```
lunco-telemetry/
  ├── Parameter                — Component tagging a field for telemetry extraction
  ├── SampledParameter         — Event triggered when a value is captured
  └── sample_parameters_system — The core reflect-and-extract engine
```

### Telemetry Tagging

By tagging a component with a `Parameter`, any field can be monitored without manual coding:

```rust
commands.spawn((
    Port { value: 42.0 },
    Parameter {
        name: "motor_current".to_string(),
        unit: "Amps".to_string(),
        path: "Port.value".to_string(),
    }
));
```

## Usage

```rust
app.add_plugins(LunCoTelemetryPlugin);

// Subscribe to telemetry events
app.add_observer(|trigger: On<SampledParameter>| {
    println!("Telemetry: {} = {:?}", trigger.event().name, trigger.event().value);
});
```

## See Also

- `lunco-attributes` — The counterpart for injecting data INTO the simulation.
- `lunco-core` — Defines the `TelemetryValue` transport types.
- `lunco-api` — Consumes telemetry events for HTTP/WebSocket broadcast.
