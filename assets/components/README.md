# Components

Components are reusable parts, not scenes or complete vehicles. Group them by
what they are so the spawn palette and text search expose a stable category:

- `avionics/`, `comms/`, `gnc/`, `payload/`, `power/`, `thermal/`
- `mobility/` for wheels, motors, gearboxes, and suspension
- `cameras/` and `lights/` for reusable presentation hardware
- `environment/` for non-spawnable environment source/adaptor prims
- `mounting/` for plug/socket examples and mounting hardware
- `terrain/` for reusable terrain fixtures

A part owns its intrinsic model, ports, geometry, and mount frames. It does not
name a battery, host vehicle, or mission. The parent USD assembly owns those
connections.

Before adding a component, search by domain API, connector, and Modelica class:

```sh
rg -n "LunCo[A-Za-z]+API|connectors:|info:sourceAsset" assets/components
```

The reusable headlight is intentionally split into two facets: `lights/headlight.usda`
owns the standard USD lamp, casing, and lens, while
`lights/headlight_controller.usda` owns the Modelica enable/photometry/load
interface. A vehicle mounts the pair and wires the controller's
`outputs:light_intensity` to the lamp's `inputs:light_intensity` LunCo light port;
the runtime maps that port to standard USD `inputs:intensity`. Rhai writes the
vehicle command input and Modelica owns the electrical math.

Communications uses the same separation. `comms/antenna.usda` owns the passive
steerable dish and its directional link phase centre; `comms/radio.usda` adds an
optional Modelica link budget. `comms/wifi_radio.usda` is a separate
chassis-mounted device with its own generic geometry endpoint and `WifiLinks`
projection. Do not put `LunCoWifiAPI` on the dish feed: a Wi-Fi radio is not the
high-gain antenna.

`cameras/visual_review_camera.usda` is the shared observer for deterministic
tutorial captures. Visual fixtures reference it and author only their
subject-specific pose; numeric scenes keep their own lean camera contract.
