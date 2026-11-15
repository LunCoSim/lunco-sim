# Propulsion assemblies and presentation

`pressure_fed_engine.usda` references three reusable Modelica program parts:
`FuelValve`, `OxidizerValve`, and `Chamber`. Its USD connections own the internal
fluid topology. Install it under an engine identity and include all three part
paths in the parent `CollectionAPI:components`; the assembly creates no separate
solver. Connect tank outlets to the valve inlets, command and health to both
valves, reservoir mass to each valve, and nozzle-design velocity to the chamber.
Install the reference and wait for its composed ports before applying external
connections. Set valve/chamber parameters from the owning design requirements. Use chamber
outputs for thrust, plume photometry and wall heating.

The `pressure_fed_engine::component_plan` Rhai tool creates this component with
typed USD document operations. Its numerical models retain passive pressure
loss, valve response, mass conservation and chamber performance equations.

## Rocket engine presentation

Reference `rocket_engine_visual.usda` at `/Engine`; position its render-only
`Exhaust` frame at the physical nozzle exit. It provides expanding volume
bounds, materials, and a local light. The engine component supplies its bell.

Connect delivered thrust, flow, effective Isp velocity, nozzle exit radius/area,
Mach, gas gamma and exit wall angle to `LunCo.Propulsion.PlumePhotometry`.
`engine_exhaust::connection_ops` wires its field, bounds, tint, clock and light
to the reusable component. `engine_count` divides aggregate thrust/flow into
identical per-nozzle values; independent engines use their own model, count 1.

Modelica supplies five conservative gas stations and an energy-bounded optical
estimate. WESL integrates emitted light in metres inside a Cylinder and adds
advected detail. Zero delivered flow/thrust is dark. The observation horizon
(default 20 exit radii) is a rendering limit, not a physical end to vacuum
exhaust. Optical defaults and fuel RGB palettes are uncalibrated assumptions.
See the [equations, validity limits and acceptance contract](../../../docs/architecture/propulsion-plume.md).

`connection_ops(edit_target, engine_path, photometry_path, output_names)`
accepts an explicit source-port map. For `RCSJet`, map `intensity` to
`light_intensity` and `radius` to `light_radius`. Keep `render_throttle` as the
plume's delivered-energy output. RCS physical geometry supplies throat, exit
radius, bell length and contour; the same shared equations compute its field.
One additive volume evaluates both normalized optical components. `core_fraction`
partitions their radiative budget; `core_width` sets the downstream core profile.
Stable `Shader.inputs:seed` controls detail, and `steps` caps the entire ray. Axial
displacement preserves cross-section area and stays inside the observation bound;
noise and the tail fade only reduce the displayed estimate.

Connect RCS fuel and oxidizer availability to the owning reservoir outputs.
Feed availability multiplies valve demand before the thruster computes force
and flow. Either missing reactant extinguishes the jet through Modelica.
Standalone default feed inputs support component studies, not vehicle wiring.

Production gates: `rocket_engine_plume_defaults`, `lander_plume_activity`,
`rcs_feed_starvation`, plus the owning Twin's engine-out and depletion cases.
Use windowed views and clean GPU logs to accept appearance.
