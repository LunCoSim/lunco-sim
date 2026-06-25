# Sandbox

The **Sandbox** is a standalone LunCoSim application designed for rapid testing of ground mobility, physics interactions, and scene composition. It is the primary tool for validating rover chassis, suspension behavior, and environment collision.

## What it does

- **Physics Validation**: Test Avian3D physics in a controlled environment.
- **Mobility Testing**: Drive rovers with different wheel types (raycast vs. physical).
- **Scene Preview**: Load and inspect USD scenes synchronously.
- **Networked Play**: Can act as a listen-server or client for multiplayer testing.

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
- [**View & Intent Architecture**](../../architecture/17-view-and-intent.md) — how camera control and possession work.
