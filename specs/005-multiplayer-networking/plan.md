# Implementation Plan: WASM & Multiplayer Networking

## Technology Stack
- **Web Build:** `wasm-bindgen` and `trunk` for compiling and serving the Bevy web app.
- **Networking Library:** `bevy_renet` or `bevy_ggrs` depending on whether we need client-server authoritative physics or peer-to-peer rollback.

## Architecture
- **Environment Parity System:** Ensures that the Rust logic can safely compile to the `wasm32-unknown-unknown` target (meaning no internal blocking sockets or multithreading outside of WASM worker scopes).
- **Network Sync System:** Captures the local inputs (WASD), dispatches them to the server/network layer, and sets the physics engine's state from the authoritative network frame.
