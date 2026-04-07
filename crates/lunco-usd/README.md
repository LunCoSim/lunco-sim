# lunco-usd

The **High-Level Orchestrator and Engineering Metadata** bridge for USD.

## Rationale
This crate acts as the central integration hub for all USD-related modules. It provides the `UsdPlugins` bundle, which registers the visual, physics, and simulation layers in the correct order.

Additionally, this crate maps **LunCo-specific Engineering Metadata** (`lunco:*` namespace). While standard USD schemas handle physics and visuals, LunCo rovers require simulation-only metadata like Ephemeris IDs, hit radii for sensors, and telemetry port mappings that aren't defined in standard OpenUSD.

## Key Functions & Features

### 1. `UsdPlugins`
A convenience bundle that adds all modular USD layers:
*   `UsdBevyPlugin`: Visuals and Transforms.
*   `UsdAvianPlugin`: Standard OpenUSD Physics.
*   `UsdSimPlugin`: NVIDIA vehicle schemas and simulation behavior intercepts.
*   `UsdLunCoPlugin`: Engineering metadata mapping.

### 2. `UsdLunCoPlugin` (Metadata Mapping)
Maps attributes in the `lunco:` namespace to Bevy components:
*   `lunco:name` -> `Spacecraft::name`
*   `lunco:ephemeris_id` -> `Spacecraft::ephemeris_id`
*   `lunco:hit_radius_m` -> `Spacecraft::hit_radius_m`

## Architecture
This crate ensures that standard-compliant USD models are enriched with the engineering data required for mission-critical lunar simulation without polluting standard visual or physics schemas.
