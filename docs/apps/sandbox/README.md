# Sandbox

> **Try it live:** [**sandbox.lunco.space**](https://sandbox.lunco.space) — runs in the browser. Early preview build; expect rough edges.

The **Sandbox** is a standalone LunCoSim application designed for rapid testing of ground mobility, physics interactions, and scene composition. It is the primary tool for validating rover chassis, suspension behavior, and environment collision.

## What it does

- **Physics Validation**: Test Avian3D physics in a controlled environment.
- **Mobility Testing**: Drive rovers with different wheel types (raycast vs. physical).
- **Scene Preview**: Load and inspect USD scenes synchronously.
- **Modeling & Cosim**: The full Modelica IDE — the same workbench the standalone
  *lunica* app provides — is embedded as the **Design workspace**. Open, edit,
  compile, and run `.mo` models with live plots. Models can be co-simulated with
  the physics: a Modelica model holds the control law, a `SimConnection` pipes its
  outputs to a physics body, and state flows back in. The bundled lander flies
  this way; see the [Cosim walkthrough](../../tutorials/03-cosim.md).
- **Networked Play**: Can act as a listen-server or client for multiplayer testing.

## Workspaces

The sandbox has two workspaces, switched via the tabs at the top of the window:

- **View** (default) — the 3D scene: viewport, spawn palette, inspector, rover
  driving, telemetry. Covered by the *Sandbox Intro → First Drive → Lander & Rover
  Mission* tutorials and the *Build a Scene → Script a Rover → Inspect the
  Simulation → Cosim* authoring track.
- **Design** — the embedded Modelica IDE: source / diagram / icon / docs views,
  a component palette, compile & run (interactive and fast), live inputs, and
  plots. Backed by the [Modelica Standard Library](https://github.com/modelica/ModelicaStandardLibrary)
  plus the bundled models in `assets/models/` (`Lander.mo`, `Battery.mo`,
  `QuarterCar.mo`, …). Open `Lander.mo` here to see the law the lander flies.

## CLI Usage

```bash
cargo run --bin sandbox -- [FLAGS]
```

### Flags

| Flag | Description |
|---|---|
| `--api [PORT]` | Enable the HTTP API server. Default port is 4101. |
| `--scene <PATH>` | Load a specific USD scene. Path is relative to `assets/`. Default: `scenes/sandbox/sandbox_scene.usda`. |
| `--no-vsync` | Disable VSync. FPS will not be capped by the display refresh rate. |
| `--no-throttle` | Disable background throttling. The window will update at full rate even when unfocused. |
| `--log-diag` | Enable Bevy's `LogDiagnosticsPlugin` to print FPS, FrameTime, and physics stats to the console. |
| `--window-pos <SPEC>` | Force the OS window to a specific screen region (e.g., `1920x1080+0+0`). |
| `--host [PORT]` | Start a networked listen-server. |
| `--connect <ADDR>` | Connect to a networked server via WebTransport. |

## Interactive Controls

- **WASD / Arrow Keys**: Drive the possessed rover.
- **Space**: Brake.
- **G key**: Translate mode (3-axis arrows) for selected objects.
- **R key**: Rotate mode (3-axis rings) for selected objects.
- **Ctrl+Z**: Undo spawns and transform changes.
- **Escape**: Cancel current operation / Deselect.

## See also

- [**USD Domain Architecture**](../../architecture/21-domain-usd.md) — how scenes are loaded and mapped to physics.
- [**Co-Simulation Domain**](../../architecture/22-domain-cosim.md) — how Modelica models and physics share a timestep.
- [**Cosim walkthrough**](../../tutorials/03-cosim.md) — build and observe a Modelica↔physics vessel.
- [**View & Intent Architecture**](../../architecture/17-view-and-intent.md) — how camera control and possession work.
