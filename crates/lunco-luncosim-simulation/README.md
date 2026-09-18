# lunco-luncosim-simulation

Renderer-independent LunCoSim domain composition.

This package installs the persistent world shell, Avian physics, USD loading
and projection, terrain, celestial, Modelica/cosimulation, mobility, avatar,
controller, hardware, telemetry, scene commands, and headless execution
plugin. It is intentionally separate from `lunco-luncosim-core`: changes to
the host-neutral Bevy substrate do not require the full domain composition to
be part of the same crate.

Application services and Rhai policy integration belong to
`lunco-luncosim-runtime`; window/render composition belongs to
`lunco-luncosim-ui`.
