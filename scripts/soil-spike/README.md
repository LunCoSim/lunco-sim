# Single-wheel soil numerical spike

Status: numerical feasibility only. No production plugin, scene, or backend change.
Owner: standalone experiment; related tracking issue [#52](https://github.com/LunCoSim/lunco-sim/issues/52).
This harness does not instantiate Avian and does not replace the production mobility model.

## Reproduce (PowerShell, repository root)

```powershell
./scripts/soil-spike/run.ps1
```

Uses the repository Rust toolchain and no third-party dependencies. Binaries and
generated CSV stay in ignored `target/soil-spike/`. Physical inputs are in
`cases.csv`; SI values, friction angle in radians. Coefficient dimensions are
`kc: Pa*m^(1-n)`, `kphi: Pa/m^n`. Parameters are deliberately synthetic,
not measured lunar-regolith properties. Width 0.2 m, radius 0.3 m, loads 25/50/100 N,
slip 0 through 0.3, n=1, kc=0, kphi=100000, cohesion=100 Pa,
phi=0.5 rad, shear length=0.02 m. Loads are prescribed; gravity is not simulated.

## What is calculated

The constitutive expressions are Bekker pressure-sinkage and Janosi-Hanamoto
shear, as summarized in [the paper, section II](https://arxiv.org/html/2410.04371v1).
The circular-foundation/contact construction below is this experiment's simplified
assumption, not a claim to reproduce Chrono SCM or a validated rolling-wheel model.

For wheel radius r, width b, sinkage z, half-contact length a=sqrt(2*r*z-z*z):

- Local vertical indentation h(x)=z-r+sqrt(r*r-x*x), x in [-a,a].
- Vertical pressure p=(kc/b+kphi)*h^n acts on horizontal projected area b*dx.
- Prescribed displacement proxy j=s*(a-x), signed slip s.
- Shear tau=(c+p*tan(phi))*(1-exp(-abs(j)/K))*sign(j).
- Midpoint integration gives vertical load and gross horizontal shear.
- Bisection solves sinkage for the prescribed load.

The n=1 independent load integral is
N=b*(kc/b+kphi)*(r*r*asin(a/r)-(r-z)*a).
A uniform-pressure patch has an independent analytic shear integral used by tests.
Those references test numerical implementation, not physical accuracy.

The intentionally restricted domain is z/r <= 0.2, abs(slip) <= 0.3, positive
width/radius/load/shear length/exponent, nonnegative pressure coefficients/cohesion,
and 0 <= phi < pi/2. Invalid inputs and overloaded cases fail explicitly.
There is no temporal solver, so refinement concerns spatial quadrature, not dt.

The reported cohesionless Coulomb bound N*tan(phi) is an analytic comparison
envelope. It is NOT a run of LunCoSim/Avian or a measured hard-ground baseline.

## Observed results (Windows, Rust nightly-2026-02-27)

Five mathematical tests failed against the unimplemented stub, then passed after
implementation. Optimized sweep: 20 cases, two runs with identical CSV values.
Observed whole-sweep times: 8.261 ms and 5.515 ms; these include parsing/formatting,
exclude compilation, and do not predict production frame rate or real-time factor.

At 256 cells and slip 0.2:

| Load N | Sinkage m | Gross shear N |
|---|---|---|
| 25 | 0.011400458148 | 8.886926839783 |
| 50 | 0.018138460850 | 18.823479454176 |
| 100 | 0.028899219663 | 40.125327758248 |

For 100 N and slip 0.2, relative error against the independent n=1 load integral
falls from 5.09e-4 at 32 cells to 7.96e-6 at 256 and 4.97e-7 at 1024.
Gross shear changes from 40.1165 N to 40.1255 N over that refinement.
The 256-to-1024 shear difference is approximately 0.00033%.

## Decision and remaining evidence

**Proceed with a bounded Rust research path; do not adopt Chrono or this model
as production physics yet.** A small dependency-free kernel is numerically feasible,
but this result does not settle soil-model fidelity or integration effort.

Existing production ownership was inspected in `crates/lunco-mobility/src/lib.rs`:
`tire_patch_force` and `longitudinal_tire_step` own tangential force, and jointed
wheel contacts disable duplicate Avian friction. Production integration must replace
the selected force path, never add a second force on top. Continuous reference
models belong in Modelica, spatial/hot mechanisms in Rust, test policy in Rhai.

Not tested here: Avian scene behavior, Modelica execution, measured soil curves,
cross-platform repeatability, WASM, Chrono parity, persistent terrain deformation,
unloading/reloading, multiple passes, curved-patch wheel kinematics, compaction
resistance, axle torque balance, or terrain/render projection. Gross shear is not
net drawbar pull. Sinkage is independent of slip in this simplified model.

Next gate: obtain a licensed measured plate/single-wheel dataset and parameter
fit; compare the current production single-wheel scene and this candidate under
the same load/slip definition. Then review a Modelica reference and authored
USD/Rhai scene test before any runtime integration. A good analytic error does
not satisfy that physical acceptance gate.
