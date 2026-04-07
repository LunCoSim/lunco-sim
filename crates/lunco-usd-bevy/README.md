# lunco-usd-bevy

The core **OpenUSD Hierarchy and Visuals** bridge for Bevy.

## Rationale
This crate provides the foundational integration between OpenUSD and Bevy. It handles the mapping of USD Prims to Bevy Entities and automatically synchronizes visual properties (shapes and transforms) from USDA files. 

By separating visuals into this crate, we keep the core integration lightweight and allow physics (`lunco-usd-avian`) or simulation metadata (`lunco-usd`) to be added as modular layers.

## Key Functions & Features

### 1. `LunCoUsdBevyPlugin`
The main plugin that sets up the USDA visual synchronization system.

### 2. Automatic Visual Mapping
Maps standard USD types to Bevy primitives:
*   `Cube` -> `Cuboid`
*   `Sphere` -> `Sphere`
*   `Cylinder` -> `Cylinder`

### 3. Data-Driven Transforms
Automatically synchronizes the following USD attributes to Bevy `Transform`:
*   `xformOp:translate` or `translate`
*   `xformOp:scale` or `scale`

### 4. Color Support
Automatically maps the standard USD `primvars:displayColor` attribute to Bevy `StandardMaterial`.

### 5. Components
*   **`UsdPrimPath`**: Links a Bevy entity to its source in the USD Stage.
*   **`UsdStageResource`**: Stores the `openusd` stage reader for lookups.

## Usage
Simply register the plugin and spawn entities with `UsdPrimPath` pointing to a valid `UsdStageResource`.
