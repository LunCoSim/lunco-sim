# Feature Specification: WASM & Multiplayer Networking

## Problem Statement
The LunCoSim final application must be highly accessible. We need to compile the Bevy engine to WebAssembly to run in browsers, and we need to implement rollback or client-server networking to allow multi-user interactions in the same instance.

## User Stories

### Story 1: Web Browser Execution
As a simulation participant
I want to load the engine from a URL without installing client software
So that we can rapidly share the simulation context.

**Acceptance Criteria:**
- The application compiles reliably to WASM and hits 30 FPS in standard browsers.

### Story 2: Multiplayer Sync
As a rover operator
I want to see rovers controlled by other users
So that joint missions and collaborative testing become possible.

**Acceptance Criteria:**
- Two users connecting to the same server will see both of their rovers.
- Movement commands sent by one user are physically replicated on the other user's browser.
