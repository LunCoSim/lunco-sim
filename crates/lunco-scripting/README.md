# lunco-scripting

High-performance scripting bridge for the LunCo Digital Twin simulation. Integrates Python (`PyO3`) and Lua (`mlua`) as first-class citizens for simulation logic, REPL, and remote control.

## Overview

`lunco-scripting` provides a unified interface for executing dynamic code within the Bevy ECS. It leverages Bevy's **Reflection System** to allow scripts to read and write Rust struct fields with zero manual glue code.

### Key Features

*   **Reflected Memory Bridge:** Scripts can access any component marked with `#[reflect(Component)]`.
*   **Dual-Mode Execution:**
    *   **Sync (Deterministic):** Scripts run in `FixedUpdate` (physics tick) via the `ScriptedModel` component. Used for high-fidelity physics plants and FSW.
    *   **Async (Interactive):** Scripts run in the REPL or via remote API requests for live debugging and automation.
*   **Digital Twin Integration:** Scripts are managed as `lunco-doc` documents, supporting versioning, hot-reloading, and co-simulation wires.
*   **CLI REPL:** Direct interactive terminal access to the running simulation.

## Architecture

The crate acts as a host for multiple language interpreters:

1.  **Python Host:** Embedded `PyO3` runtime.
2.  **Lua Host:** Embedded `mlua` runtime (implementation pending).
3.  **Reflected Proxy:** A generic bridge that maps script attribute access (e.g., `entity.Transform.x`) to Rust memory offsets using the `AppTypeRegistry`.

## Usage

### 1. The ScriptedModel Component

To attach logic to an entity, add a `ScriptedModel` component:

```rust
commands.spawn((
    Name::new("MySubsystem"),
    ScriptedModel {
        document_id: Some(123), // Link to a ScriptDocument
        language: Some(ScriptLanguage::Python),
        inputs: [("voltage".to_string(), 12.0)].into(),
        ..default()
    }
));
```

### 2. CLI REPL

When running the simulation, use the terminal to interact with the world:

```python
>>> rover = world.get_entity("Zhurong")
>>> rover.Battery.level = 1.0
>>> world.spawn_rover("NewRover", pos=(0, 5, 0))
```

### 3. Remote Execution (MCP)

AI agents can trigger scripts via the `lunco-api`:

```json
{
  "command": "ExecuteScript",
  "params": {
    "language": "python",
    "code": "world.get_entity('GreenBalloon').ScriptedModel.paused = True"
  }
}
```

## Dependencies

*   **Python:** Requires Python 3.12 shared libraries installed on the system.
*   **Linker:** On Linux, ensure `libpython3.12.so` is in your library path. The project `.cargo/config.toml` includes helpers for standard Ubuntu paths.

## Testing

Run the scripting tests (requires Python 3.12):

```bash
cargo test -p lunco-scripting
```

To test the full physics integration:

```bash
cargo test -p lunco-scripting --test green_balloon_test
```
