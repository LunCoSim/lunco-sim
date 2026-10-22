# Rocket engine presentation

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
The shader's two additive layers partition the radiative budget with energy
fractions summing to one. Material detail only attenuates that budget.

Connect RCS fuel and oxidizer availability to the owning reservoir outputs.
Feed availability multiplies valve demand before the thruster computes force
and flow. Either missing reactant extinguishes the jet through Modelica.
Standalone default feed inputs support component studies, not vehicle wiring.

Production gates: `rocket_engine_plume_defaults`, `lander_plume_activity`,
`rcs_feed_starvation`, plus the owning Twin's engine-out and depletion cases.
Use windowed views and clean GPU logs to accept appearance.
