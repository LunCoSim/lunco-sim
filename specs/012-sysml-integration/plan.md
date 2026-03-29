# Implementation Plan: SysML v2 Integration

## Technology Stack
- **Parser:** Custom Rust SysML v2 parser, or utilizing the official SysML v2 API/JSON exports to deserialize into Rust structs via `serde`.
- **Engine:** Bevy ECS

## Architecture
- **SysmlLoader System:** Reads the `.sysml` models on startup (or via a drag-and-drop event).
- **Entity Factory:** Iterates through the parsed SysML part definitions and maps them to Bevy `Bundle`s (e.g., mapping a SysML `mass` property to `ColliderMassProperties::Mass(...)`).
