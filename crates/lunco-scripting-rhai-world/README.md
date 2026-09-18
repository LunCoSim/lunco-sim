# lunco-scripting-rhai-world

Reusable Rhai world/runtime substrate for LunCoSim.

This crate owns the reflected world bridge, `RhaiScenarioRuntime`, authored
application/Twin policy activation, and optional Twin-scoped native hook
providers. It is separate from the application composition package so
catalog/diagnostic consumers and world-bridge changes do not rebuild command,
tool-library, or timeline-registration code.

`lunco-scripting-rhai-runtime` installs and composes this package with the
Rhai command, tool, and timeline surfaces.
