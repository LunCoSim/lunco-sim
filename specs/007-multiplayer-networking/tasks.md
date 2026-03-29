# Tasks: WASM & Multiplayer Networking


## Phase 1: WASM compilation
- [ ] 1.1 Update CI or write a local verification script that fails if `cargo check --target wasm32-unknown-unknown` fails.
- [ ] 1.2 Clean up strictly blocking I/O (like file loading or sockets) and implement `trunk` fallbacks to pass the WASM compilation check.

## Phase 2: Networking Sync
- [ ] 2.1 Write failing integration tests `test_server_syncs_rover_position` where a mocked network frame updates the local Bevy physics World.
- [ ] 2.2 Add networking boilerplate (`bevy_renet` or `ggrs`) and establish a local signaling server.
- [ ] 2.3 Write the synchronization systems to safely send and receive deterministic positional updates from the server to pass the netcode tests.
