# Implementation Plan: OpenMCT Telemetry Streaming

## Technology Stack
- **Networking:** Async runtime `tokio` (for sockets), and `tungstenite` or a standard WebSockets crate.
- **Serialization:** `serde_json` to format the packet structures.

## Architecture
- **Telemetry Broadcaster System:** A Bevy system that runs at a designated Hz (e.g., every 0.1s using a `Time::Timer`).
- It iterates over all entities that have a `Telemetry` component, packages their Bevy components into a `serde_json` payload, and fires it over the MPSC channel to the async `tokio` network sender.
