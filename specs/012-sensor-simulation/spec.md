# Feature Specification: 012-sensor-simulation

**Feature Branch**: `012-sensor-simulation`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Advanced sensor modeling (LIDAR, Depth Cameras, RGB Cameras, Star Trackers) for realistic telemetry and autonomous navigation.

## Problem Statement
Standard rovers rely on more than just scalar sensors (IMU, Encoders). To support autonomous navigation stacks (via ROS2 or Fprime), the LunCoSim Bevy engine must provide high-fidelity simulated perceptual sensors. This involves extracting rendering data natively from Bevy's PBR pipeline without tanking performance.

## User Scenarios

### User Story 1 - Depth Camera & Point Clouds (Priority: P1)
As an autonomy engineer, I want the rover to have a simulated stereo/depth camera that generates depth maps, so that my autonomy algorithms can detect obstacles in real-time.

**Acceptance Criteria:**
- A `Sensor` component representing a Depth Camera can be attached to a vessel.
- The Engine leverages an additional Render Pass or Compute Shader in Bevy to extract Z-buffer depth data.
- The depth data is serialized and streamed out via the Telemetry Bridge at least 15 Hz.

### User Story 2 - RGB Camera Streaming (Priority: P2)
As a remote operator, I want to see a live visual feed from the rover's mast cameras.

**Acceptance Criteria:**
- Render Targets are used to capture the viewpoint of a specific `RoverCamera` component.
- The pixel buffer is compressed (e.g., H.264 or JPEG) and passed to the Bridge for OpenMCT/ROS transmission.

## Requirements

### Functional Requirements
- **FR-001**: **Headless Mode Support**: The sensor rendering pipeline MUST work even when Bevy is running headlessly via off-screen rendering (EGL/Vulkan).
- **FR-002**: **Performance Strictness**: Reading back textures from the GPU to the CPU (for the Bridge interface) MUST be done asynchronously to avoid stalling the main Bevy thread.
