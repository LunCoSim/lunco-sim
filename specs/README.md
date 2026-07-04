# Feature Spec Status Index

Status legend: **Implemented** (built, in tree) · **Partial** (core built, gaps remain) · **Not built** (roadmap, not started) · **Superseded** (design replaced).

| Spec | Title | Status | Notes |
|------|-------|--------|-------|
| 000 | Testing Framework | Implemented (historical) | |
| 001 | Vessel Control Architecture | Implemented (historical) | 5-layer action-to-actuator |
| 002 | Celestial Visualization & World Foundation | Implemented | "Implemented & Stable" |
| 003 | Simple Astronaut Character | Partial | avatar = camera/intents/recording only; no IK/skeleton/anim |
| 004 | Technical Logging | Partial | bevy `tracing` only; no OpenTelemetry/Jaeger |
| 005 | Multiplayer Core | Implemented (historical) | |
| 007 | Scenario Orchestration | Superseded | RON/BSN + Verifier replaced by rhai ScenarioDriver/ScenarioRuntime + MCP `run_scenario` (see `docs/scripting-guide.md`) |
| 008 | Developer Experience | Partial | scripting is rhai, not Lua/Python |
| 009 | Coordinate Frame Tree | Partial | `big_space` floating-origin built; named TF-tree types absent |
| 010 | Authority / RBAC | Implemented | |
| 011 | Interactive Tutorials | Partial | tutor mode built; no objective/goal-eval framework |
| 012 | Sensor-to-Dashboard | Partial | telemetry bridge + scalar sensing; no camera/RGB sensors, no OpenMCT/Grafana |
| 013 | SysML Integration | Not built | overlaps 017 (interop/model mapping) |
| 014 | Modelica Simulation | Implemented | rumoca runtime |
| 015 | Realtime Assembly | Partial | spawn/catalog/gizmo built; no snap-assembly/auto-wiring; overlaps 031 |
| 017 | Advanced Interop (Robotics/Kinematics) | Partial | USD export built; URDF/ROS2/DDS planned; overlaps 013 |
| 018 | Astronomical Environment | Implemented | |
| 020 | World State & Replay | Partial | doc journal + snapshot + replay; no ECS WorldSnapshot/MCAP |
| 021 | Asset Pipeline | Partial | sourcing/caching/MSL built; no decimation/KTX/GIS |
| 025 | Terramechanics | Partial | wheel kinematics built; Bekker-Wong soil absent |
| 030 | USD Scene Integration | Implemented | |
| 031 | Sandbox Editing Tools | Implemented | overlaps 015 (spawn/assembly) |
| 032 | Model Source Listing & Unified Open | Implemented (historical) | |
| 033 | Agent-Driven Simulation Loop | Implemented (historical) | |
| 034 | Control Authority: Autopilot as a User | Not built | proposal (rev 2); autopilot = an `AiAgent` session; possession is the arbiter, `rbac.authorize` (rhai) decides stealing — reuses 010, no per-frame arbiter |

## Overlap notes

- **015 ↔ 031** — sandbox spawn/gizmo tooling (031, implemented) overlaps realtime-assembly (015); assembly auto-wiring still pending.
- **013 ↔ 017** — SysML model mapping (013) and robotics/interop standards (017) share the model-to-ECS / export surface.

## Removed specs

Deleted as obsolete/superseded or never-started wishlist (recoverable via git history):

- **006** time-and-integrators — obsolete (no Integrator trait; modelica solvers + rhai cover it).
- **007** scenario-orchestration — superseded (RON/BSN + Verifier replaced by rhai ScenarioDriver + MCP `run_scenario`).
- **016** scientific-render-pipelines, **019** comm-degradation, **022** fmu-gmat-integration,
  **023** spatial-audio, **024** isru-resource-economy, **026** eclss-human-factors,
  **027** hil-sil-integration, **028** dust-degradation, **029** power-systems — not started; removed to keep the backlog honest. Resurrect from git history if/when picked up.
