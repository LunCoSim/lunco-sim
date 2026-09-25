# LunCoSim asset library

This directory is the authored runtime product. Start here before adding code or
creating another asset: most vehicles, scenes, equations, rules, and behaviors
already have a reusable source below.

## Responsibility map

| Concern | Author it in | Owns |
|---|---|---|
| Assembly | USD (`.usda`) | prim hierarchy, references, variants, transforms, connectors, collections |
| Continuous math | Modelica (`.mo`) | physical equations, state, rates, domain calculations |
| Scenario logic | Rhai (`.rhai`) | orchestration, rules, scoring, assertions, user-facing outcomes |
| Reusable behavior | Rhai (`.rhai`) | inspectable stateful decisions and behavior composition |
| Heavy runtime capability | Rust | general high-cost algorithms and engine bridges, with control exposed to Rhai |

Rust must not encode a particular rover, lesson, mission, or assembly. Rhai must
not integrate physics. Modelica must not decide mission policy. Connections
between independently valid parts belong in the USD assembly that uses them.

## Find the right asset

| Folder | Contract |
|---|---|
| `components/<domain>/` | independently reusable parts and infrastructure |
| `vessels/<class>/` | composed mobile assemblies |
| `structures/<class>/` | composed fixed installations |
| `props/` | simple scene objects |
| `scenes/base/` | reusable scene foundations |
| `scenes/luncosim/` | interactive example/application scenes |
| `scenes/tests/` | focused executable regression scenes |
| `models/LunCo/<domain>/` | packaged reusable Modelica library |
| `models/` | standalone examples and compatibility-independent demo models |
| `scenarios/` | reusable Rhai scenario programs |
| `scenarios/` | reusable Rhai task programs |
| `scripting/lib/` | importable Rhai helpers |
| `scripting/policy/` | policy hooks and the application policy manifest |
| `tutorials/` | lesson Rhai, optional lesson worlds, and authored lesson data |
| `missions/` | mission data and orchestration entrypoints |
| `shaders/` | WGSL materials |
| `lighting/` | global lighting rigs |
| `celestial/` | celestial assemblies |
| `manifests/` | downloadable dataset manifests, not an authored-asset index |

Use `rg` before creating:

```sh
rg -n "concept|port_name|LunCo.*API" assets/components assets/vessels assets/models
rg -l "info:sourceAsset|references|payload" assets -g '*.usda'
rg --files assets -g '*.usda' -g '*.mo' -g '*.rhai'
```

The filesystem is the discovery manifest. Do not add a second hand-maintained
registry. A concise README beside a domain is for navigation; runtime identity
stays in the asset and its `defaultPrim`.

The application policy manifest is identified by
`kind = "lunco.policy.v1"`. Its `scripting.source.classify` policy assigns
authored Rhai sources to the prelude, standard tool libraries, or ordinary
scenario content. Rust loads the source tree through the asset/storage layer
and applies that decision; source directories are not repeated in Rust
registries. Standard tools live in the lower `lunco://` layer, while a Twin's
`tools/*.rhai` files are loaded into its own scoped overlay and are removed on
Twin close. A policy whose owner is supplied by an optional runtime feature can
declare `skip_when_hook_unavailable = true`; the selected build omits it when
that owner is not linked and exposes the capability in `policy_status()`.

## Canonical reusable entrypoints

| Need | Start with |
|---|---|
| local gravity, Sun, or Earth direction | `components/environment/probe.usda` |
| solar generation | `components/power/solar_panel.usda` |
| electrical storage | `components/power/battery.usda` |
| powered transmitter | `components/comms/transmitter_power.usda` |
| radio endpoint/link math | `components/comms/radio.usda` |
| steerable dish | `components/comms/antenna.usda` |
| chassis-mounted Wi-Fi radio | `components/comms/wifi_radio.usda` |
| powered science imager | `components/payload/science_camera.usda` |
| lunar navigation camera grade | `components/cameras/lunar_surface_camera.usda` |
| mount-system demo part | `components/mounting/demo_probe.usda` |
| configurable rover assembly | `vessels/rovers/skid_rover.usda` |
| environment + tracking regression | `scenes/tests/sun_tracker.usda` |

## Composition rules

- Use `lunco://` for every reference, payload, and sublayer to this library.
  Twin-owned files use `twin://`; do not escape an asset root with `..`.
- A component is independently valid. Optional connections are authored by the
  vessel/structure/scene that composes it.
- Environment values come from a distinct
  `LunCoEnvironmentProbeAPI` prim. Never declare an environmental output on a
  consumer and wire it back to itself.
- A probe's transform is its measurement frame. Put a panel probe beneath the
  panel when incidence must follow panel rotation; put a tracking probe on the
  controller mount when the controller needs vehicle-frame direction.
- One prim has one defining spec per layer/variant. Extend it inside that spec;
  do not create a sibling `def` and `over` with competing opinions.
- A lesson catalog is the unique JSON asset marked
  `kind = "lunco.tutorial-catalog.v1"`. Its Rhai source and optional USD scene
  are passed to the generic scenario launcher; the scene is ordinary USD
  composition and does not require a tutorial schema.

## Validation

Use the already-built production binary:

```sh
target/debug/luncosim --validate path/to/asset.usda path/to/model.mo
target/debug/luncosim test --scene assets/scenes/tests/<case>.usda
```

Also run the owning crate's focused tests after changing schemas or discovery.
Compilation alone does not prove that a composed scene loads or a Rhai program
executes.
