# Feature Specification: 018-astronomical-environment

**Feature Branch**: `018-astronomical-environment`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Macroscopic celestial mechanics, Ephemeris data integration, and VRAM-optimized surface rendering.

## Problem Statement
A flat plane map is insufficient for an aerospace digital twin. The simulation must understand that a rover interacts with a massive sphere (the Moon) which orbits another sphere (Earth), both rotating relative to a light source (the Sun). Without establishing macroscopic mechanics first, tracking line-of-sight dropouts (`019`) and multi-day thermal limits (`014`) is impossible. 

**This is for Built-in Celestial Computation:** The engine MUST provide a native, high-performance source of truth for solar angles and planetary rotations using JPL SPICE or Ephemeris data. This ensures consistent lighting and geometry for all players and internal solvers without requiring external 3rd-party dependencies for core simulation loops.

## User Scenarios

### User Story 1 - Ephemeris & Celestial Mechanics (Priority: P1)
As an optics and thermal engineer, I want the sun angle and Earth position accurately plotted mathematically based on a specific calendar date and lunar latitude, so that shadows and thermal inputs behave precisely as they would in real life.

**Acceptance Criteria:**
- The engine computes planetary positions dynamically using generic Ephemeris data or JPL SPICE kernels.
- The `005` Scenario loader sets a `Timestamp` and a coordinate (e.g., `Lat/Lon` at the Lunar South Pole), and the Bevy lighting systems (Directional Light properties) are calculated automatically by the celestial mechanics module, not hard-coded floats.

### User Story 2 - Spherical Occlusion (Priority: P2)
As a communications engineer, I need to know when an orbiting relay satellite passes behind the horizon of the Moon relative to the surface rover, instantly cutting the signal.

**Acceptance Criteria:**
- The math engine models celestial bodies as macroscopic `f64` spheres in the Large World Coordinate grid out to billions of meters.
- The communication subsystem (`019`) performs raycasts against these celestial spheres to calculate line-of-sight dropouts, treating the Moon as a physical blocking object rather than just a floor plane.

### User Story 3 - Planetary Terrain Streaming & DEM Loader (Priority: P4 - Future Phase)
As a scenario designer, I want to load specific APOLLO `.tif` heightmaps (DEMs) for the rover to physically drive on, but loading an entire regional map will crash standard GPUs.

**Acceptance Criteria:**
- **Future Optimization**: The engine will implement or interface with a Chunked QuadTree LOD system (Terrain Streaming). As the rover drives, the engine seamlessly pulls high-resolution `.tif` heightmap tiles from SSD to VRAM for the local rendering frustum, whilst drastically decimating geometry for distant mountains.
- **TBD - Soil Properties**: Planetary entities (Moon, Mars) will eventually require **Regolith Property Maps** (density, cohesion, grain size) to support high-fidelity Terramechanics (027) once implemented.
