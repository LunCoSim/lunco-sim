# Feature Specification: 007-coordinate-frame-tree

**Feature Branch**: `007-coordinate-frame-tree`
**Created**: 2026-03-29
**Status**: Draft
**Input**: f64/f32 precision split, planetary-scale coordinates, robotic joint hierarchies.

## Problem Statement
Aerospace simulations involve deeply nested coordinate frames: a robotic arm (local) sits on a rover (local) which is on a crater (planetary) on the Moon (celestial). Bevy's default `Transform` uses `f32` and a single parent-child hierarchy, which causes catastrophic precision loss at planetary scales. We need a **Hierarchical Frame Tree** that integrates with our `f64` truth positions and `big_space` origin rebasing.

## User Stories

### User Story 1 - Multi-Scale Nesting (Priority: P0)
As a systems engineer, I want to attach a sensor to a robotic arm and have its position correctly calculated in both "Rover-Local" and "Lunar-Global" coordinates.

**Acceptance Criteria:**
- The engine supports a `HighPrecisionTransform` (`f64`) that mirrors Bevy's hierarchical parent-child relationship.
- Local offsets (e.g., sensor relative to arm) are stored as `f64`.
- Global truth is calculated by traversing the tree: `Local_Offset * Parent_Global_Transform`.

### User Story 2 - Robotic Joint Kinematics (Priority: P1)
As a robotics engineer, I want to rotate a rover's mast and have the attached camera's view frustum update accordingly without "jitter."

**Acceptance Criteria:**
- Rotation is stored as `DQuat` (f64 quaternions).
- The `SyncTransformSystem` (from ontology) correctly propagates parent rotations to children every physics tick before rendering.

### User Story 3 - Planetary Surface Anchoring (Priority: P0)
As a mission planner, I want to place a base at a specific Latitude/Longitude on the Moon and have it stay "pinned" as the Moon rotates.

**Acceptance Criteria:**
- The engine provides a `PlanetarySurfaceAnchor` component.
- Anchors convert `Lat/Long/Alt` into `f64` Cartesian coordinates relative to the planet's center.
- Entities anchored to a planet are children of the planet's `f64` frame.

## Implementation Details: The Link-Joint-Model Hierarchy

LunCoSim adopts the industry-standard robotics hierarchy used by Gazebo (SDF) and Isaac Sim (USD), ensuring compatibility with URDF and aerospace modeling tools.

### 1. The Coordinate Stack (Ladder of Frames)
To maintain `f64` precision across solar-system distances, transforms are calculated as a "ladder" of relative offsets:

1.  **Solar System Frame (f64 Global)**: Absolute origin at the solar system barycenter.
2.  **Body-Fixed Frame (f64 Global)**: Centered on and rotating with a celestial body (e.g., Moon-Fixed).
3.  **Model Root (Link)**: The `base_link` of a Vessel (e.g., the Rover chassis).
4.  **Child Link**: A sub-part connected via a **Joint** (e.g., a robotic arm segment or a wheel).
5.  **Port Offset**: The fixed point on a Link where a `Wire` connects.

### 2. Links and Joints (The TF Tree)
- **Link**: A Bevy entity representing a rigid coordinate frame. It carries a `HighPrecisionTransform` (f64).
- **Joint**: The logical relationship between a **Parent Link** and a **Child Link**. 
    - Joints define the **Degrees of Freedom (DoF)**: Fixed, Revolute (rotation), Prismatic (sliding).
    - The transformation `Child.Local_Transform` is updated by the Joint's state (e.g., "rotate by 0.5 rad").

### 3. Dual-Hierarchy Propagation
- **Simulation Truth (L1-L3)**: All physics, FSW, and Modelica logic operate on the `f64` Link-Joint tree.
- **Render Proxy (L5)**: The `SyncTransformSystem` maps the `f64` hierarchy into Bevy's `f32` `Transform` components relative to the current **Floating Origin** (from `big_space`).

## Requirements

### Functional Requirements
- **FR-001**: **Link-Joint Hierarchy**: The engine MUST support a tree of `f64` Links connected by logical Joints.
- **FR-002**: **Body-Fixed Conversion**: The engine MUST provide an "Anchor" system to pin Model Roots to a rotating celestial body (Body-Fixed Frame).
- **FR-003**: **Direct Reference Update**: Child Links MUST update their global `f64` state by directly referencing their Parent Link's `f64` state (fast traversal).
- **FR-004**: **Angular Precision**: All Joint rotations and Link orientations MUST use `DQuat` (f64) to prevent "quaternion drift" over long mission durations.
