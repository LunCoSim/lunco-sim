# lunco-usd

The **Engineering Ontology** bridge for LunCoSim, mapping custom OpenUSD attributes to simulation-specific components.

## Rationale
While `lunco-usd-avian` handles standard physics, this crate focuses on the **Engineering Metadata** required for LunCoSim. It allows us to use OpenUSD as the "Single Source of Truth" for vehicle configurations (Orion, Rovers, etc.). By authoring attributes like `ephemeris_id` or `battery_capacity` in a `.usda` file, the simulation automatically populates the correct Bevy components on load.

## Key Functions & Features

### 1. `LunCoUsdPlugin`
The main plugin that registers observers for custom LunCo-specific USD attributes.

### 2. Spacecraft Metadata Mapping
Automatically maps attributes in the `lunco:` namespace to the `Spacecraft` component in `lunco-core`:
*   `lunco:name` -> `Spacecraft.name`
*   `lunco:ephemeris_id` -> `Spacecraft.ephemeris_id`
*   `lunco:reference_id` -> `Spacecraft.reference_id`
*   `lunco:hit_radius_m` -> `Spacecraft.hit_radius_m`
*   `lunco:user_visible` -> `Spacecraft.user_visible`

### 3. Modular Integration
This crate depends on `lunco-usd-bevy` for the core `UsdPrimPath` component, ensuring that metadata mapping and physics mapping happen in parallel through the same observer pattern.

## Usage Example (USDA)
```usda
def Xform "Orion" {
    custom string lunco:name = "Orion Capsule"
    custom int lunco:ephemeris_id = -1024
    custom float lunco:hit_radius_m = 1000.0
}
```
When this USD Prim is loaded into Bevy and tagged with `UsdPrimPath`, the `LunCoUsdPlugin` will automatically insert a `Spacecraft` component with the values above.
