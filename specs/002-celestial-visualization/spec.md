# Feature Specification: Celestial Visualization

**Feature Branch**: `002-celestial-visualization`
**Created**: 2026-03-31
**Status**: Draft
**Input**: User description: "I want to have model of Earth/Moon/Sun system, kind of like kerbal. It has to be simple. I want to use exponential camera. I want to be able to rotate it. Ideally I want to visualise position/trajectory of Artemis 2 mission there. Earth and Moon should be simple spheres. If there are exising svg or similar lightweight format that we could use to draw contours (max 2MB) we could use it. Position of three bodies must be real based on real data. We should be able to set time when it happens. If we get close we should be able to drive rovers. We can split asses by 10x10 km or so positions when we are close for physics and drive rovers there so it must be clickable."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Macroscopic Celestial Navigation (Priority: P1)

As a mission observer, I want to view the Earth, Moon, and Sun system from a detached camera, so I can understand their relative positions and movements at a specific point in time.

**Why this priority**: This is the core of the visualization feature, establishing the scene and context for all other interactions.

**Independent Test**: The user can load the simulation, see the three celestial bodies correctly positioned for a given date, and navigate the camera around them.

**Acceptance Scenarios**:

1.  **Given** the simulation is started with a specific UTC timestamp, **When** the observer camera is active, **Then** the Earth, Moon, and Sun are rendered as spheres at positions corresponding to that timestamp, fetched from the `018-astronomical-environment` backend.
2.  **Given** the observer is viewing the celestial system, **When** they use mouse/keyboard inputs, **Then** the camera smoothly rotates around the focused celestial body (or barycenter) without being attached to any object.
3.  **Given** the Earth and Moon are rendered, **When** a low-detail texture or SVG-based contour map is available, **Then** it is applied to the spheres to provide basic surface reference marks (e.g., continents).

### User Story 2 - Exponential Camera Zoom & Scaling (Priority: P1)

As a user, I want to seamlessly zoom from a view of the entire Earth-Moon system down to a few kilometers above the lunar surface, so that I can transition from strategic overview to local exploration without loading screens.

**Why this priority**: This provides the "Kerbal-like" feel the user requested and is critical for the user experience of a multi-scale simulation.

**Independent Test**: The user can use the mouse wheel or keyboard shortcuts to zoom the camera in and out, observing a smooth, exponential change in distance from the target, from thousands of kilometers down to near the surface.

**Acceptance Scenarios**:

1.  **Given** the camera is focused on the Moon from a distance, **When** the user continuously zooms in, **Then** the camera speed and dolly sensitivity adjust dynamically, allowing for fine control both far away and up close.
2.  **Given** the camera is near the lunar surface, **When** the user zooms out, **Then** the view transitions smoothly back to an orbital perspective without jitter or sudden speed changes.

### User Story 3 - Mission Trajectory Visualization (Priority: P2)

As a space enthusiast, I want to visualize the flight path of the Artemis 2 mission relative to the Earth and Moon, so I can understand the trajectory of this historic mission.

**Why this priority**: This adds a compelling, real-world use case to the visualization tool.

**Independent Test**: The user can enable a toggle in the UI that draws the Artemis 2 trajectory in the 3D scene.

**Acceptance Scenarios**:

1.  **Given** the celestial scene is active, **When** the user enables the "Artemis 2 Trajectory" overlay, **Then** a line is rendered in 3D space representing the mission's path, using the `ScientificOverlay` capability from spec `016-scientific-render-pipelines`.
2.  **Given** the trajectory is visible, **When** the simulation time changes, **Then** an icon or marker representing the spacecraft moves along the pre-defined path to the corresponding position.

### User Story 4 - Surface Tile Interaction (Priority: P3)

As a mission planner, I want to be able to click on a specific region of the lunar surface when zoomed in, to prepare for a rover driving session.

**Why this priority**: This connects the high-level visualization to the core rover simulation gameplay.

**Independent Test**: When close to the Moon, the user can click on the surface, and the UI will indicate the selected tile.

**Acceptance Scenarios**:

1.  **Given** the camera is within 100km of the lunar surface, **When** a grid overlay is activated, **Then** the surface is visually divided into sectors (e.g., 10x10 km).
2.  **Given** the grid is visible, **When** the user clicks on a grid sector, **Then** the application registers the selection and presents an option to "Initiate Rover Drop" or "Load Terrain" for that area, which would trigger logic from `025-terramechanics` and `018-astronomical-environment`'s terrain streaming story.

## Requirements *(mandatory)*

### Functional Requirements

-   **FR-001**: The system MUST provide a free-floating camera not attached to any single physics object.
-   **FR-002**: Camera controls MUST support rotation (orbiting) and exponential zooming (dollying).
-   **FR-003**: The visualization MUST source celestial body positions and orientations from the (now renumbered) `018-astronomical-environment` specification's systems.
-   **FR-004**: The system MUST allow rendering of pre-defined 3D paths (trajectories) as overlays, using capabilities from `016-scientific-render-pipelines`.
-   **FR-005**: The system MUST support raycasting from the camera to the surface of celestial bodies to identify a point of interest for interaction.

### Key Entities

-   **ObserverCamera**: A Bevy `Camera` entity with a controller script that implements the exponential zoom and orbital rotation logic.
-   **TrajectoryVisual**: A renderable entity representing a path, composed of a series of points.
-   **SurfaceTile**: A logical and potentially clickable area on a celestial body's surface, used for transitioning to ground-level scenarios.

## Success Criteria *(mandatory)*

### Measurable Outcomes

-   **SC-001**: Users can smoothly transition from viewing the full Earth-Moon system (approx. 400,000 km scale) to a 10km view of the lunar surface in under 5 seconds of continuous zooming.
-   **SC-002**: Trajectory overlay rendering has a performance impact of less than 5% on frame rate.
-   **SC-003**: Celestial body positions on screen are accurate to within 0.1% of their predicted positions from a trusted external source (like NASA's Horizons).

## Assumptions

-   The backend data for celestial positions (`018-astronomical-environment`) is available and provides the necessary `f64` precision.
-   A method for rendering line overlays (`016-scientific-render-pipelines`) is available.
-   The core simulation supports the time-setting functionality described in `006-time-and-integrators`.
-   The user is okay with simple spheres for celestial bodies, with contour maps (e.g., SVG-based texture) being a "nice-to-have" enhancement and not a blocker.
-   High-resolution terrain for rover driving is out of scope for this specific visualization spec and is handled by other systems (`018`, `025`), which this system will simply "trigger".
