# Feature Specification: 016-scientific-visualization

**Feature Branch**: `016-scientific-visualization`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Visual diagnostic overlays, spatial data rendering, and thermal gradient mapping for engineers natively in Bevy.

## Problem Statement
Standard simulation graphics use physically based rendering (PBR) to make the game look realistic. However, test engineers need a "Predator Vision" mode. They need to visualize invisible 1D mathematical states (like Modelica thermal outputs or avian joint stress limits) mapped directly onto the 3D meshes to diagnose issues visually inside the `006-unified-editor`.

## User Scenarios

### User Story 1 - Sensor Frustum Debugging (Priority: P1)
As an optics engineer, I want the engine to draw the invisible FoV (Field of View) cones of my cameras and the individual raycast lines of my LIDAR, so that I can visually verify why a rover didn't see a rock.

**Acceptance Criteria:**
- The engine can toggle a `ScientificOverlay` mode which renders neon wireframe geometry for active spatial capabilities (LIDAR strikes, camera frustums, communication line-of-sight beams).

### User Story 2 - Modelica Thermal Gradients (Priority: P2)
As a thermal engineer, I want the surface of the rover to shift dynamically from Blue to Red based on its mathematical temperature, so that I can see exactly which solar panel is overheating due to the sun angle.

**Acceptance Criteria:**
- The engine supports a custom shader pipeline that intercepts PBR materials and replaces them with a normalized Heatmap gradient.
- The visual gradient dynamically scales according to the real-time scalar data outputted by the `013-modelica-simulation` runtime.
