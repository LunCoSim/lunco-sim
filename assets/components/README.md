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
