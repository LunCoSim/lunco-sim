# Feature Specification: 016-asset-pipeline

**Feature Branch**: `016-asset-pipeline`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Parameteric CAD to Bevy conversion optimization.

## Problem Statement
Spacecraft engineers design in SolidWorks, FreeCAD, or NX. These systems export parameteric solids (.STEP or .IGES files) which are mathematically dense, unoptimized, and incompatible with real-time Game Engine rendering. The simulation needs an automated pipeline to ingest these files without forcing engineers to learn Blender.

## User Scenarios

### User Story 1 - Automated Geometry Decimation (Priority: P2)
As a mechanical engineer, I want to upload a `.step` file of a newly designed wheel assembly, so that the simulation can generate an optimized visual layout for Bevy rendering.

**Acceptance Criteria:**
- An external script or service (e.g., FreeCAD python macro) translates the parameteric `.step` file into polygons.
- Polygons are decimated to a constrained polycount and exported as standard `.gltf` / `.glb` files.

### User Story 2 - Automated Collision Hull Generation (Priority: P2)
As a simulation developer, I need physics colliders that accurately map to the visual mesh without being overly complex, otherwise `avian` will experience severe performance bottlenecks.

**Acceptance Criteria:**
- The pipeline generates optimized Convex Hull meshes (specifically tuned for Avian components) and encodes them alongside the glTF visual data.
