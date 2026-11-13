# Twin source URI stability — 2026-11-12

## Current contract

Display name, stable source identity, private mount lease and workspace
`TwinId` have separate owners and lifetimes. User scene addresses use
`twin://<source-identity>/<relative-path>`. Asset reads use a fresh opaque
mount authority for each mount lifetime. Reload and runtime reset retain the
source identity; reopening the Twin creates a new mount lease without changing
the scene URI.

`LoadScene` resolves the stable URI at admission and rejects live or retired
mount leases. Composed USD dependencies may retain stable source identities;
scene dataset admission resolves them to the current mount and never rebinds a
retired lease. Scene progress reasons remain generic, while `/api/ready`
`scene_load` labels retain the stable URI. Workbench status and Modelica
console messages, scene-source navigation, resource-preview, Connections and
SysML source labels resolve mounted Twin sources to the display name and
relative path.

Twin-scoped Rhai facts, execution context and lifecycle status omit the
workspace `TwinId` and private mount lease. Typed Rust command dispatch binds
the owner Twin to command fields that require app-level selection. Stable
source identity remains available for authored URIs and source-set keys.
Twin command discovery and completion omit workspace selectors, and a Twin
script cannot submit its own workspace `twin_id`. Application-level selectors
retain that identity because they own Twin selection and routing. Scene-time
selection sends only scene facts to Rhai; the source path stays in Rust and is
projected through the stable URI owner for diagnostics.

## Verification

- Production `luncosim` build and the celestial owner check passed with the
  workspace-pinned Rumoca Git revision
  `e325c2c12d2115e78f481f40701784b510dcca3c`; the ignored local Cargo config
  contains no sibling checkout override.
- The authored production API regression passed three cycles of scene load,
  runtime reset, source restart, scenario-menu load and Twin remount. It checked
  the stable URI, fresh remount leases, rejection of live/retired leases,
  stable readiness labels, filtered Twin command discovery, and rejection of a
  caller-supplied `OpenTwinScene.twin_id` from Twin scope.
- The owned runtime log contained no 32-character opaque `twin://` authority.
- The exact Griffin Twin published
  `twin://astrobotic-griffin-1/scenes/griffin_1_surface_ops.usda` in its
  `/api/ready` scene-load label. The full scene did not reach
  readiness because the LROC Nobile 03 terrain dataset is unavailable in this
  environment.
- The regression session exited through the API and released its port.
