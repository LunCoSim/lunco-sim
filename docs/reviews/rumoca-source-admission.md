# Modelica source-root admission

## Current owner contract

The workspace pins Rumoca `488d6d5f239fcae2bf4ce5a1b729a23d2cc6c396`. That
revision exposes Rumoca's parsed `StoredDefinition`, `Name`, and class queries,
but it does not expose `compile::source_root_dependencies`.
`lunco-modelica-ast::source_dependencies::required_source_roots` is the single
LunCoSim adapter that derives external namespace roots from those parsed facts.
It returns a sorted set for each source definition and has no registry or scene
state input. Rumoca retains final compile-time name resolution and unresolved
member diagnostics.

Compile dispatch, the Modelica API, source-root diagnostics, and diagram
projection use that same adapter. `lunco-modelica-source-roots` maps each result
to one registered owner and queues a cold load before compilation; a ready root
is reused without another load. Twin-owned roots are retired with their Twin,
while application-owned library roots retain their application lifetime.

## Focused regression coverage

- `top_level_model_keeps_external_component_type_root` proves a top-level model
  referencing `LunCo.Mobility.RoverAckermannDrivetrain` requests the `LunCo`
  source root.
- `compile_root_admission_loads_cold_root_and_reuses_ready_root` proves a cold
  registered root is queued before compilation and a ready root is not loaded
  again.
- `recovered_package_model_keeps_external_base_namespace` covers an external
  base-class reference from a source authored under `within`.
