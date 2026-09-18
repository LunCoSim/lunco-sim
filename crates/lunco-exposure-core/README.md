# lunco-exposure-core

Renderer-independent typed exposure storage for LunCoSim.

This crate owns the reusable `EngineExposures` registry, typed
`ExposureValue`s, subject identity, revision tracking, and coalesced refresh
state. It does not derive domain values or render them. The production ECS
projection is owned by `lunco-luncosim-exposures`; API, scripting, recovery,
and UI consumers read the same registry.
