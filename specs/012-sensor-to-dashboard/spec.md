# Feature Specification: 011-sensor-to-dashboard

**Feature Branch**: `011-sensor-to-dashboard`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Advanced sensor modeling, telemetry streaming to OpenMCT/Grafana, AI agent data feeds.
**Consolidates**: Former specs `011-telemetry-bridge`, `012-sensor-simulation`.

## Problem Statement
End-to-end sensor data flow — from simulated physics to mission control dashboards — is a single pipeline. The engine must: (1) simulate high-fidelity perceptual sensors (LIDAR, Depth Cameras, RGB) by extracting data from Bevy's render pipeline, (2) route all sensor data through a unified telemetry bridge, and (3) stream to professional visualization tools (OpenMCT, Grafana) and AI agent APIs simultaneously.

These are two halves of one system: sensors produce the data, the bridge delivers it.

## User Stories

### Sensor Simulation

#### Story 1: Scalar Sensor Pipeline (IMU, Encoders, Temperature) (Priority: P0)
As a controls engineer, I want the rover's IMU and wheel encoders to produce realistic readings routed through the OBC Port pipeline.

**Acceptance Criteria:**
- `Sensor` components produce scalar values (acceleration, angular velocity, encoder ticks, temperature).
- Sensor data flows: Level 1 (Sensor) → Level 2 (OBC Input Ports) → Level 3 (FSW Logic).
- Sensors support configurable noise models (Gaussian noise, bias drift).

#### Story 2: Depth Camera & Point Clouds (Priority: P1)
As an autonomy engineer, I want a simulated stereo/depth camera that generates depth maps for obstacle detection.

**Acceptance Criteria:**
- A `DepthCamera` Sensor component can be attached to a space system.
- The engine leverages an additional Render Pass or Compute Shader to extract Z-buffer depth data.
- Depth data is serialized and streamed via the telemetry bridge at ≥15 Hz.

#### Story 3: RGB Camera Streaming (Priority: P2)
As a remote operator, I want to see a live visual feed from the rover's mast cameras.

**Acceptance Criteria:**
- Render Targets capture the viewpoint of a `RoverCamera` component.
- The pixel buffer is compressed (e.g., H.264 or JPEG) and passed to the bridge.

---

### Telemetry Bridge

#### Story 4: Multi-Sink Data Streaming (Priority: P1)
As a mission operator, I want to stream live telemetry to OpenMCT and Grafana simultaneously.

**Acceptance Criteria:**
- The `TelemetryBridge` plugin supports multiple concurrent `DataSink` targets.
- Users can configure sinks via the Developer Experience editor (`006`) or CLI config.
- Data from all `Sensor` components is serialized into a generic format before transmission.

#### Story 5: Real-Time Dashboard Integration (Priority: P1)
As a mission controller, I want to see live solar power draw and battery voltage on an aerospace dashboard.

**Acceptance Criteria:**
- `Sensor` component values are broadcast via the telemetry bridge.
- OpenMCT displays live charts under 500ms latency.

#### Story 6: AI Agent / MCP Data Feed (Priority: P2)
As an AI Agent (e.g., Claude via MCP), I want a cleanly formatted JSON stream of telemetry for autonomous diagnostics.

**Acceptance Criteria:**
- The bridge exposes a dedicated JSON-formatted WebSocket or MCP endpoint.
- Rigid data structures specifying transforms, actuator loads, and battery states are emitted continuously.

## Requirements

### Functional Requirements
- **FR-001**: **Headless Sensor Rendering**: The sensor pipeline MUST work in headless mode via off-screen rendering (EGL/Vulkan).
- **FR-002**: **Async GPU Readback**: Reading textures from GPU to CPU for the bridge MUST be done asynchronously to avoid stalling the Bevy thread.
- **FR-003**: **Wire Formats**: Scalar telemetry uses JSON over WebSocket for dashboard sinks. High-bandwidth sensor data (LIDAR, depth) uses FlatBuffers. ROS2 interop uses CDR/XCDR2.
- **FR-004**: **Configurable Update Rates**: Each sensor and sink MUST support independent update frequencies (e.g., IMU at 100Hz, cameras at 30Hz, dashboard at 10Hz).

### Key Entities
- **Sensor**: ECS component producing data from the physical plant (Level 1).
- **DepthCamera / RGBCamera**: Specialized Sensor components leveraging the render pipeline.
- **TelemetryBridge**: Plugin routing sensor data to external sinks.
- **DataSink**: Configurable output target (OpenMCT adapter, Grafana adapter, JSON WebSocket, MCAP file).
