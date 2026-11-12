# Propulsion plume model and rendering

`LunCo.Propulsion.computeFreeJetProfile` owns a reduced, forward, supersonic
ideal-gas streamtube. `computePlumePhotometry` handles per-nozzle normalization
and optical estimates. `PlumePhotometry` and `RCSJet` expose the same field to
USD. Modelica owns all continuous math and transported distance; Rhai authors
connections and supplies test stimuli. Rust's existing Modelica, USD, scalar
port and material mechanisms are sufficient. No plume-specific Rust is needed.

The Twin and shared asset library form one interface revision. Deploy the
updated BellNozzle, plume models, package order, shader and authored connections
together. An older library cannot resolve `gamma_out`/`exit_lip_angle_rad`;
an intermediate Twin using `flow_phase_m` also needs the current phase pair.
Do not add substitute gas values or compatibility outputs to conceal a mixed
deployment.

The upstream chamber uses a limiting-reactant mass balance: reacting fuel is
`min(fuel_flow, oxidizer_flow/O_F)`, and useful mixture fraction is reacting
propellant divided by total delivered flow. Chemical heat uses reacting fuel.
O/F remains a diagnostic, rather than an unbounded feedback variable in the
pressure solve when fuel runs out. The nominal stoichiometric point is
unchanged; off-design performance is a reduced study model, not equilibrium
chemistry. Both reservoir-depletion gates must exercise the coupled feed.

## Gas model

Inputs are delivered thrust F, mass flow mdot, effective Isp velocity, exit radius
r_e, exit Mach M_e, gamma, gas constant R and ambient pressure p_a. BellNozzle
and the plume share `supersonicExitMach`, the bracketed supersonic inverse of
the [isentropic area-Mach relation](https://www.grc.nasa.gov/WWW/BGH/isentrop.html).
Nozzle contour supplies the exit wall tangent. Effective Isp velocity includes
pressure thrust; it must not be substituted for gas speed:

```
A_e = pi*r_e^2
u_e = (F + p_a*A_e) / (mdot*(1 + 1/(gamma*M_e^2)))
T_e = u_e^2/(gamma*R*M_e^2)
p_e = mdot*u_e/(A_e*gamma*M_e^2)
T0  = T_e*(1 + (gamma-1)*M_e^2/2)
h0  = u_e^2/2 + gamma*R*T_e/(gamma-1)
```

The opening closure is explicit: a finite exit disk plus transverse thermal
spread (two transverse degrees of freedom, each with assumed variance R*T_e)
and the geometric lip direction:

```
r(z) = sqrt(r_e^2 + 2*(z/(sqrt(gamma)*M_e))^2) + z*tan(lip_angle)
```

This is an engineering spreading closure, not a Prandtl-Meyer or CFD solution.
Five stations at z/L = [0, 1/16, 4/16, 9/16, 1] resolve the near-nozzle gradient.
At each station the supersonic area relation gives M; constant stagnation
enthalpy gives T and u; `rho = mdot/(pi*r^2*u)` closes mass. Outputs include
station radius, density, temperature, speed and engineering residuals. The
thrust residual also compares F against mdot times effective Isp velocity.

L is an explicit rendering observation horizon, default 20 exit radii. Vacuum
exhaust does not physically stop there. The model rejects invalid gas/nozzle
parameters, positive thrust without flow, and overexpanded conditions requiring
shock compression. Zero thrust produces no modeled hot gas, including residual
unburnt feed during shutdown; the propulsion owner still drains that feed.
It does not predict rarefied backflow, chemistry, shocks,
entrainment, interaction between adjacent jets, or plume-surface interaction.
NASA's [plume-surface interaction overview](https://www.nasa.gov/plume-surface-interaction/)
illustrates why those are separate physical problems.

## Optical approximation and GPU boundary

The radiative estimate is `P_rad = eta_rad*mdot*h0`. The optically thin emission
profile is weighted by rho squared and normalized to P_rad by eight-point Gauss-Legendre quadrature over the
piecewise-linear radius and log-linear emissivity field. Independent composite
Simpson integration verifies that budget. Positive log interpolation resolves
the steep density-squared falloff without bright axial station bands. Radiation is passive postprocessing
of the gas solution: its small energy loss is neglected in the adiabatic gas
state. The owner requires eta_rad <= 0.01 to bound this neglected term to 1%. Defaults eta_rad = 0.0002 (main), 0.00005 (RCS), efficacy = 200 lm/W,
R = 300 J/(kg K), and the fuel-family RGB palette are **uncalibrated study
assumptions**. They are not a combustion spectrum, blackbody law, or measured
Griffin radiometry. Calibration needs species/temperature/emission data.

USD connects the physical scalar outputs to a unit Cylinder's scale/center and
to `plume.wesl`. The nozzle is at local y=-0.5 after its 180 degree rotation;
exhaust expands downstream. The cylinder is an invertible render bound, never
a collider or force-actuator frame. Keep a shared RCS actuator on its original
mount and put its visual volume in a child, so volume scaling cannot change
force position or direction.

The existing shader-port adapter validates and lowers Modelica f64 values to GPU
f32 uniforms. One Cylinder and one material integrate both optical components.
The shader interpolates the five stations, converts ray length to metres, and
integrates isotropic `j*efficacy/(4*pi)` in cd/m2 with camera exposure. Output is
additive RGB with zero alpha. `core_fraction` partitions the field between the
outer profile and `core_width`; their normalized radial weights have unit
cross-sectional mean. Each component retains its hot/edge colour parameters.

The coherent lateral displacement is a function of axial position and the
Modelica phase only. It translates each cross-section without changing its area.
Its magnitude is bounded by the remaining observation-envelope radius, so the
raster bound encloses the complete emitted support. Three-dimensional noise is
bounded attenuation shared by both components. Attenuation and the explicit
tail fade reduce the displayed radiant estimate; they do not conserve its exact
integral. The underlying unattenuated field remains normalized to Modelica's
radiative estimate. Numerical ray integration has a separate approximation error.

`seed` is an authored, per-emitter scalar with default zero. Vehicle instances
choose stable seeds on the bound `Shader.inputs:seed`; GPU instance allocation
never determines appearance. An unconnected `Gprim.inputs:seed` does not feed
a material uniform. Shared stock RCS tints use small local Material references
with a seed override, so each emitter keeps its identity without copying the
shader or its physical model.
Modelica publishes a bounded sine/cosine phase over a 1024-exit-radius period.
Both axial displacement and 3D attenuation wrap their integer lattices.
Attenuation frequency is rounded to 1/1024 cell per exit radius so the transport
period contains whole cells. Two optical noise bands need two 3D evaluations
per accepted ray sample; neither band crossfades two full noise fields. The
wrapped cubic interpolant joins values and slopes, so pause and transport rate use the simulation clock without a render-clock mirror.

`steps` is a total ray ceiling, clamped to 16--512. The shared visual requests
128. Clip all four station intervals to opaque depth first, estimate sampling
demand from metric path length and core radius, then distribute the available
samples proportionally with a cumulative partition. Every nonempty interval gets
at least one sample; the sum cannot exceed the ceiling. Each interval uses its
actual `ds`. This is a bounded numerical approximation, not an exact power
conservation claim. Compare the selected budget with a higher-budget reference
for close, end-on and side views before changing authored quality.

The reusable stock lander's passive model is `/DescentLander/PlumePhotometry`,
separate from rendered geometry and from the main propulsion model partition.
The `engine_exhaust` authoring helper wires one volume and one local light.

The reusable rocket visual anchors this point-light proxy at the nozzle exit
(the unscaled Exhaust frame), independently of the observation volume. Its world position therefore stays at the nozzle as the
envelope length changes or intersects terrain during landing. Luminous power
still comes from photometry, including extinction at zero delivered energy.
The ground illumination is an approximation, not resolved plume-surface radiation.

## Exit filling and hot nozzle walls

Both optical components start across the physical exit disk. The near-exit radial
distribution is `(45/32)*(1-r^8)^2`, with unit disk mean. Over four exit radii
it blends into the existing normalized downstream core distribution and its
authored width. Advected intermittency develops over that same distance rather
than cutting holes into the exit plane. This changes optical distribution, not
the gas density, mass flow, or available radiant power. Increasing apparent
density by inventing extra propellant is not an optical fix.

`NozzleWallThermal` estimates throat, middle and exit-wall temperatures using
three existing `ThermalMass` components and connected heat-exchange boundaries.
The bell design supplies geometry and gamma. Frustum areas follow its contour;
gas recovery temperatures use local isentropic Mach numbers. Film transfer uses
the Bartz area dependence `(Ae/A)^0.9`, with an authored exit reference
coefficient. This is a reduced axial closure, not the full Bartz correlation.
Axial conduction transfers heat between zones. The exposed outer surface
radiates to the environment; the inner wall is heated by the gas rather than
being counted as a second cold-space radiator. Total heating remains capped
by the same authored fraction of jet power. The observer is passive: it does not yet feed wall heat
loss back into the chamber solution. Its heat capacity, areas, recovery factor,
film coefficient and mounting conductance require hardware calibration; the
defaults are explicitly study assumptions, not measured Griffin properties.

The opaque `thermal_metal.wesl` material interpolates the three wall temperatures
at their actual local axial positions using the same nozzle length. It converts
the resulting temperature to visible
emission through the shared `blackbody_radiance.wesl` approximation of Planck's
law and CIE 1931 quadrature (400--700 nm, 20 nm spacing,
[CIE 2019 data](https://doi.org/10.25039/CIE.DS.xvudnb9b)). The shader interpolates
temperature before the nonlinear conversion and multiplies by emissivity.
`scripts/render/generate_blackbody_radiance.py` owns the spectral data and
reproducibly generates piecewise Chebyshev coefficients for log(XYZ/T) in
reciprocal temperature. The final interval extends to the Rayleigh--Jeans limit.
Its float32 comparison requires relative XYZ error below 0.02% over 256--1e9 K
and across interval boundaries. The explicit cutoff below 256 K omits less than
4e-24 cd/m2 per XYZ component. There is no per-pixel wavelength loop or GPU
lookup-resource lifetime. Temperature and emissivity retain the explicit GPU
narrowing boundary; Modelica thermal state and diagnostics stay double precision.
The reported `wall_temperature_k` is a heat-capacity weighted mean, not the
hot-zone temperature. Cold metal stays dark. Hot metal can remain luminous
after delivered thrust and plume light have reached zero. This is a surface
emission model, not ray-traced indirect lighting or a combustion spectrum.

Run `scenes/tests/nozzle_wall_thermal.usda` for native warm-up, shutdown,
cooling, power bounds, independent stored-energy integration and the real GPU
temperature/emissivity wires. Inspect a powered and cooling vehicle view as
well: this mathematical fixture alone does not establish a convincing nozzle.

## Opaque occlusion

The cylinder is a rasterization bound, not a gas surface. Testing its front face
against the depth buffer does not stop a ray from integrating behind a leg
inside the bound. Conversely, testing an inside-camera exit face can reject
foreground gas because that face lies behind a solid object.

`plume.wesl` declares `//!@capability lunco.volume-depth.v1`. The render adapter
enables Bevy's native `DepthPrepass` on 3D views while a bound mesh consumes that
capability, including preview and alternate cameras. It retires only prepasses
it supplied; existing explicit prepasses remain owned by their caller. Source,
material-presence and camera lifecycle events invalidate that decision. Live
uniform writes do not rescan the scene.

The volume pipeline leaves depth writes off and lets the fragment shader own
occlusion. It uses one centre ray per fragment. Each covered MSAA sample's
opaque depth is reconstructed through the current view matrix and converted
to that ray's local coordinate. Station intervals stop at the farthest covered
exit. Within each integration segment, the shader averages the exact visible
length fractions over covered samples; the rasterizer still applies the
proxy's coverage. The material field is evaluated once, with visibility in
[0,1], rather than once for every sample. This is centre-ray antialiasing with
sample-specific depth limits, not supersampling the gas field.

`sample_index` would force per-sample invocation when its result affects
output ([WGSL built-in contract](https://www.w3.org/TR/WGSL/#sample-index-builtin-value)).
`sample_mask` preserves coverage and also works on backends that supply a
single-bit invocation mask; the performance gain is backend-dependent. The
fixed array covers Bevy's supported 1/2/4/8 sample modes. Reverse-Z clear depth
does not invent an opaque surface. Perspective and orthographic rays use their
native projections. Before the required depth pass is available, the shader
emits nothing.

This clips against opaque and alpha-masked geometry in the native prepass. It does not implement
transparent-surface extinction or ordered refractive volumes. Lens bloom can
still spread light across a silhouette after compositing; it is distinct from
integrating hidden gas.

## Acceptance

Run `rocket_engine_plume_defaults.usda` through the production editor scene
runner. Its Rhai observer independently checks mass, stagnation energy,
pressure-thrust separation, emissivity integration, similarity, flow scaling,
transport and zero-flow darkness. Run `lander_plume_activity.usda` for the real
valve/feed and shader-port boundary, `rcs_feed_starvation.usda` for both
reactants, and the owning Twin's engine-out/depletion gates. Inspect powered,
partial and idle main/RCS views in a windowed runtime and check GPU diagnostics.
A numeric gate alone does not establish visual quality or flight accuracy.

Run `scripts/api/test_plume_depth.py --output <directory> --quality high` and
repeat with `--quality low`. The visible graphics fixture verifies front,
embedded and rear occluders and an inside-volume camera in both projections.
It disables bloom only to isolate geometric clipping in its pixel verdicts.

Run `python3 scripts/render/generate_blackbody_radiance.py` to check generated
source and the independent spectral reference, including a perturbed-coefficient
negative control. Run `scripts/api/test_plume_field_contract.py --output <dir>`
for native GPU spectral samples, radial normalization, support, phase wrapping
and total-budget partitioning; it requires green/red/green pixel controls.
