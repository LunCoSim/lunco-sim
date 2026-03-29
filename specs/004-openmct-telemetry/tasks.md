# Tasks: OpenMCT Telemetry Streaming

- [ ] 4.1 Set up a basic local OpenMCT telemetry server (e.g., using a nodejs scaffold or standard HTTP OpenMCT tutorial setup).
- [ ] 4.2 Add `tokio`, `tungstenite`, and `serde_json` to the Rust project.
- [ ] 4.3 Create a Bevy system that gathers Rover state (Transforms, Battery level) and serializes it.
- [ ] 4.4 Hook the system to an async task that streams the JSON to OpenMCT over WebSockets.
