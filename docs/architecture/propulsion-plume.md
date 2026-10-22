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

The existing shader-port adapter explicitly validates and lowers Modelica
f64 values to GPU f32 uniforms. The shader interpolates stations, converts ray
length to metres, and integrates isotropic `j*efficacy/(4*pi)` in cd/m2 with the
camera's exposure. Both volume layers use additive RGB and zero alpha. Their
energy fractions sum to one; compact radial weights have unit cross-sectional
mean. Noise warps and attenuates the field, with per-instance variation; it
cannot add radiative power. A tail fade loses light at the observation boundary.
Modelica integrates transported distance in f64 and publishes its sine/cosine
phase over a 1024-exit-radius period. This continuous bounded pair avoids GPU
f32 precision loss on long burns; periodic noise joins the phase boundary.
Pause and time scaling therefore apply to advection through the shared clock. The local light uses the same estimated luminous power as a
raster illumination proxy; it is not an additional propulsive energy source or
a volumetric lighting solver. Fuel colour remains a documented heuristic.

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
occlusion. Each ray starts at its pixel's near plane. The nearest opaque depth
sample is reconstructed through the current view matrix and converted to the
volume's local ray coordinate. Each station interval ends at
`min(station_exit, opaque_surface_t)`; an empty interval emits nothing. Reverse-Z
clear depth does not invent an opaque surface. Perspective and orthographic
rays use their native projections; MSAA reads the corresponding depth sample.
Before the required depth pass is available, the shader emits nothing.

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
