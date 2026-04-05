# lunco-usd-composer

The **USD Composition and Path Resolution** layer for LunCo.

## Rationale
Complex USD assets (like full rover models or lunar missions) are rarely contained in a single file. They rely on "Composition"—the ability to reference external files, add sublayers, or define payloads. 

Standard USDA files use relative paths. This crate provides a high-level `UsdComposer` that can:
1. **Resolve Asset Paths**: Correctly locate referenced files relative to the root asset directory.
2. **Flatten Stages**: Combine multiple layers into a single, unified data map that can be parsed by the simulation stage loader.

By isolating this logic, we ensure that the core stage readers (`lunco-usd-bevy`) can remain simple, focusing only on parsing the final flattened data.

## Key Functions & Features

### 1. `UsdComposer::flatten`
A recursive path resolution engine. It walks through a USD stage's references and sublayers, locating them on the filesystem and merging their data into a primary stage map.

### 2. Path Anchoring
Automatically anchors relative USD references to the correct Bevy asset directory (e.g., `assets/vessels/rovers/...`), ensuring assets load correctly regardless of where the simulation is launched from.

## Usage
Used internally by `UsdLoader` in `lunco-usd-bevy` to resolve references before the stage is fully initialized.
