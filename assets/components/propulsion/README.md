# Rocket engine presentation

Reference `rocket_engine_visual.usda` at `/Engine` from an engine component.
It supplies render-only exhaust geometry, shader materials and a light under
`Exhaust`; position that frame at the physical nozzle exit. It also supplies a
dark metal material for an engine's `Bell` mesh. There is no engine-name detector.

Instantiate `LunCo.Propulsion.PlumePhotometry` and connect actual combustion
activity, thrust, propellant flow and effective exhaust velocity, plus nominal
thrust capability and nozzle radius/area. `engine_exhaust::connection_ops`
provides the standard 23 USD scalar connections to geometry, shader and light.
For an aggregate engine network, set `engine_count` once; outputs are per nozzle.

Modelica owns the metric envelope and colour. Omitted inputs use library defaults:
1000 N capability, 0.10 m nozzle radius, 2600 m/s design exhaust velocity,
1000 Pa visible pressure threshold, radial expansion 1.6, core radius fraction
0.65 and neutral white unknown-fuel colour. Zero optional area, capacity, core
radius or luminance selects the documented derived value. Explicit values remain
available for calibrated designs. Fuel codes are 0 unknown, 1 hydrocarbon,
2 hydrogen and 3 hypergolic. Richness is -1 fuel rich, +1 oxidizer rich, or 0
for an estimate from actual O/F and an explicitly authored chemistry reference.
These colours, pressure cutoff and shape factors are visualization estimates,
not a spectral model, calibrated photometry or CFD.

Delivered momentum is the lower of thrust and flow times exhaust velocity.
Zero momentum yields zero render activity, visible length and luminous power.
The shader multiplies opacity and emission by that activity. The chamber uses
mixture efficiency for combustion activity, thrust and heat, so oxidizer flowing
without fuel cannot sustain combustion. No script switches plume visibility.

Production regressions: `rocket_engine_plume_defaults.usda` covers a generated Modelica network with omitted library inputs, burn, zero
flow with stale thrust, scaling and palettes; `lander_plume_activity.usda`
covers real feed/valve spool response. Griffin's external Twin additionally
exercises actual tank depletion in its production assembly.

`engine_exhaust::connection_ops(edit_target, engine_path, photometry_path,
output_names)` accepts an explicit source-port map. Use an empty map for
`PlumePhotometry`; for `RCSJet`, map `intensity` to `light_intensity`, `radius`
to `light_radius`, and `render_throttle` to `activity`. Geometry remains owned
by this shared component. The helper also authors the four local overrides
needed to connect inherited exhaust prims.

`RCSJet` composes the same photometry model. Connect its
`available_fuel_mass_kg` and `available_oxidizer_mass_kg` inputs to the owning
reservoir outputs. The feed availability multiplies valve demand before the
thruster calculates force and flow; absent either reactant, activity and plume
outputs fall to zero through the model equations. Standalone default feed
inputs are available for component studies, so defaults are not evidence of
vehicle tank wiring. `rcs_feed_starvation.usda` verifies both starvation paths
while valve demand remains open, and restoration between them.
