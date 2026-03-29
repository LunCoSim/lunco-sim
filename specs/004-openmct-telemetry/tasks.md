# Tasks: OpenMCT Telemetry Streaming


## Phase 1: Telemetry Serialization
- [ ] 1.1 Write failing unit tests for serializing the Rover's `Transform` and `BatteryLevel` states into OpenMCT's expected JSON dictionary format using `serde_json`.
- [ ] 1.2 Implement the parsing logic and `Serialize` traits to pass the serialization tests.

## Phase 2: Async Streaming
- [ ] 2.1 Write a mocked failing test `test_telemetry_broadcasts_to_socket` verifying the async task effectively intercepts the telemetry packets via channel receivers.
- [ ] 2.2 Add `tokio`, `tungstenite`, and channel dependencies to `Cargo.toml`.
- [ ] 2.3 Implement the Bevy system and the `tokio` async background task to seamlessly stream the JSON strings over a WebSocket connection to pass the test.
- [ ] 2.4 Test connection manually against a local OpenMCT node server.
