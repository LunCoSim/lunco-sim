# USD standard boundary

This is the decision table for new LunCoSim authoring. It is deliberately
conservative: retain a custom schema only when the standard has no matching
semantic owner.

| Need | Use the standard | Keep custom LunCo semantics only for |
|---|---|---|
| Prim identity and hierarchy | `kind`, `doc`, `Usd.ModelAPI`, `UsdGeom.Xformable`, `assetInfo` | LunCo catalog admission and runtime ownership, if the standard does not express it |
| Units and transforms | layer `upAxis`, `metersPerUnit`, `UsdGeom.Xformable` | none |
| Geometry and visibility | `UsdGeom` typed prims, `UsdGeom.Imageable`, `purpose`, `visibility`, `Usd.CollectionAPI` | terrain-generation policy that is not geometry itself |
| Materials and node graphs | `UsdShade.Material`, `Shader`, `NodeGraph`, `ConnectableAPI`, `MaterialBindingAPI` | engine-specific shader implementation parameters only when the shader has no standard schema |
| Lights and shadows | `UsdLux.LightAPI`, concrete light types, `UsdLux.ShadowAPI`, light-list APIs | renderer-specific local-light influence cutoff where UsdLux has no equivalent; engine-specific environment or earthshine semantics |
| Cameras | `UsdGeom.Camera` and authored transforms | camera-follow/path-time semantics that USD does not define |
| Bodies, collision, mass, joints, limits, drives | `UsdPhysics` and the maintained PhysX schema when the runtime needs PhysX-specific behavior | vehicle-domain concepts such as wheel/tire/suspension authoring where no standard vehicle schema is present |
| Graph topology | typed `inputs:`/`outputs:` attributes and native `connectionPaths`; `UsdShade` connectable conventions for graph nodes | domain-specific port meaning and legality, expressed as open domain data/rules |
| Graph editor layout | `UsdUI` node-graph APIs when supported by the reader | no new `lunco:*pos` duplicate |
| Modelica program allocation | USD `asset` on a `LunCoProgramAPI` prim is a LunCo runtime allocation; Modelica source remains standard Modelica | no second backend-name field or special Rust model registry |
| Raw ray observation | Avian query result and generic runtime ports | raycast configuration required by LunCo; semantic conversion belongs in Modelica |
| Mission, celestial, orbit, geodetic anchor | no equivalent in core USD | narrow LunCo schemas are appropriate |
| Telemetry and control-session ownership | no equivalent in core USD | narrow runtime schemas are appropriate; do not put sampled values into topology |

## Rules for a custom schema

- Do not repeat a standard property under `lunco:*`.
- Do not create a schema only to make a reader's `if` branch convenient.
- Prefer a generic applied schema plus authored role/token data over a schema for
  every component role.
- Keep the schema source in `crates/lunco-usd/schema/schema.usda`; regenerate
  `generatedSchema.usda` and `plugInfo.json` with `scripts/gen_schema.py`.
- Add a composition test proving the authored property is present and a runtime
  test proving a real reader consumes it.
- If no reader consumes the property, delete it instead of documenting it as a
  future extension.
