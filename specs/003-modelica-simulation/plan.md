# Implementation Plan: Modelica Subsystem Simulation

## Technology Stack
- **Modelica Engine:** `rumoca` (native Rust Modelica compiler and runtime).
- **Engine:** Bevy ECS

## Architecture
- **Rumoca Loader Component:** A Bevy component storing the active Modelica model instance compiled via `rumoca`.
- **Subsystem Step System:** A Bevy update system running exclusively in the `FixedUpdate` schedule. It steps the rumoca simulation forward by `Time<Fixed>::delta_seconds()`, passing in Bevy state (like `speed`) as inputs to the Modelica model, and mapping the outputs (like `battery_level`) back onto the Bevy components.
- **Determinism:** By running in `FixedUpdate`, we guarantee that the math remains deterministic and decoupled from the rendering framerate.
