# Design Specification: Unified Scripting & REPL Bridge

**Date**: 2026-04-16
**Status**: Draft
**Topic**: Integration of Python (PyO3) and Lua (mlua) for internal simulation logic, REPL, and remote control.

## 1. Problem Statement
The LunCo simulation requires a flexible, high-performance scripting system that allows engineers and developers to:
1.  **Interact** with the live simulation via a command-line REPL (Quake-style console or terminal stdin).
2.  **Implement** Flight Software (FSW) and autonomous logic in hot-reloadable Python/Lua scripts.
3.  **Bridge** deterministic 1D physics (Modelica) with 3D environment data using scripts as "glue".
4.  **Expose** the engine's internal state to external AI agents (MCP) and CLI tools using a consistent memory-reflected schema.

## 2. Architecture Overview
The system is built on a "Shared Memory Schema" philosophy. Instead of manually binding every struct, we leverage Bevy's **Reflection System** to expose Rust memory directly to Python and Lua.

### 2.1 The `lunco-scripting` Crate
A new standalone crate that hosts the language interpreters and the reflection bridge.
- **Python Backend**: Powered by `PyO3`.
- **Lua Backend**: Powered by `mlua`.
- **Reflection Bridge**: Uses `AppTypeRegistry` to map Python/Lua attribute access (e.g., `entity.Transform.x`) to Rust struct fields.

### 2.2 Dual Execution Models
- **Sync (Deterministic)**: Scripts run inside the Bevy `FixedUpdate` (physics) loop. Used for "Virtual Plants" and FSW. Follows an IO-mapping pattern (Inputs -> Logic -> Outputs).
- **Async (Interactive)**: Scripts run in a background thread or at the end of a frame. Used for the REPL, CLI commands, and MCP requests.

## 3. Key Components

### 3.1 The Reflected Proxy
Generic wrappers for Python and Lua that allow scripts to:
- **Read/Write Components**: Access any component registered in the `TypeRegistry`.
- **Call Commands**: Trigger any event marked with `#[derive(Command)]` (leveraging `lunco-api`'s command dispatcher logic).
- **Query Entities**: Search for entities by name, ID, or component.

### 3.2 The CLI REPL (Async Stdin)
- **Host**: A background thread launched by the `ScriptingPlugin`.
- **Mechanism**: Listens to `stdin` and pushes code snippets into a thread-safe channel.
- **Lifecycle**: Snippets are drained and executed at the start of each frame in the main thread to ensure ECS safety.

### 3.3 Remote API Integration
- `lunco-api` will be extended with an `ExecuteScript` request.
- This allows external MCP agents to send Python/Lua code which is then executed by the same internal bridge.

## 4. User Experience

### 4.1 CLI REPL Example
```bash
# User types in terminal where simulation is running:
> world.get_entity("Zhurong").Transform.translation.y += 10.0
> world.spawn_rover("NewRover", pos=(10, 0, 10))
```

### 4.2 Scripted FSW (Lua)
```lua
-- fsw_drive.lua
function update(inputs, outputs)
    if inputs.battery > 0.2 then
        outputs.drive_power = 1.0
    end
end
```

## 5. Implementation Strategy
1.  **Phase 1**: Implement `lunco-scripting` crate with basic Python/Lua setup.
2.  **Phase 2**: Implement the `ReflectedProxy` to allow reading/writing `f32` fields from Rust to Python.
3.  **Phase 3**: Implement the `stdin` background thread for the CLI REPL.
4.  **Phase 4**: Add the `ExecuteScript` endpoint to `lunco-api`.

## 6. Success Criteria
- [ ] User can launch the simulation and type Python code in the terminal to move an entity.
- [ ] Python scripts can read and write `Reflect`-enabled components without manual glue code.
- [ ] High-frequency physics scripts can run at 60Hz without significant frame-time spikes.
- [ ] MCP agents can execute the same script snippets remotely.
