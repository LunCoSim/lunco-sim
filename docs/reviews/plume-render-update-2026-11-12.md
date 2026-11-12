# Plume render update - 2026-11-12

The close-engine path now evaluates one coherent, optically thin volume per
nozzle. The previous broad and core cylinders had the same live envelope, so
they duplicated rasterization, interval traversal and procedural field work.
Their two normalized optical contributions now share an integral and a total
128-sample ceiling. This changes optical presentation, not engine force, feed,
gas conservation or the fixed simulation timestep.

The former fragment `sample_index` also multiplied the entire integral by
MSAA sample count. The new fragment shares a centre ray, retains covered opaque
depth exits, and averages the visible fraction of each integration segment.
It preserves raster coverage and native perspective/orthographic depth. It is
centre-ray material shading, rather than supersampling the gas field. Native
coverage-mask behavior and its portability limit are described in the
[WGSL contract](https://www.w3.org/TR/WGSL/#sample-mask-builtin-value).

Two advected noise bands use wrapped cubic lattices instead of crossfading two
complete 3D fields each. Frequency quantization to 1/1024 cell per exit radius
makes the 1024-radius transport period seamless. The lateral displacement is
constant across each axial section, area-preserving, and restricted to the
remaining observation bound. Noise and tail fade remain bounded attenuation.
Per-emitter constants belong to the bound Shader, and all Griffin seeds have
been checked against the actual published material ports.

The opaque nozzle material uses a generated piecewise approximation of the
original Planck/CIE quadrature. Temperature is interpolated before conversion;
emissivity remains independent of throttle. The checked float32 XYZ error is
1.615559e-5 relative (about 0.00162%) over 256..1e9 K and band boundaries.
Below 256 K, omitted XYZ terms are each below 4e-24 cd/m2. Modelica wall
integration and cooling are unchanged.

## Ownership and deployment

Modelica owns delivered gas state, station emissivity and transported distance.
USD owns bounds, connections, optical constants and material binding. WESL owns
the passive radiance integral. Rhai owns authoring and test stimuli. The stock
lander's passive photometry is now `/DescentLander/PlumePhotometry`, independent
of a render primitive. The redundant `core_envelope_radius_m` output and second
render volume were removed from all tracked callers in these two repositories.
No Rust, dependencies, physics tolerances or cadence were changed.

Deploy the shared Modelica files, shared visual/helper, WESL modules and Griffin
Twin together. Historical tracked film overrides were migrated without changing
camera/timeline data. Ignored study candidates discovered during authoring were
restored byte-for-byte to their pre-task snapshots. All USD changes used the
native document API; no authored USD was edited as text.

## Validation and evidence

Evidence is in `.cache/perf/plume-update/` in the simulator checkout. The retained
native binary identifies itself as `99249918-dirty`; its SHA256 is
`292ed92c75a4f34be681ca605a71625568e05b965d7c338afb6c0a437ea14c98`.
This was an existing production binary with the explicit updated asset root,
not a claimed fresh Rust build. The GPU was an NVIDIA RTX 5060 Laptop GPU using
Vulkan on Linux. Other native app sessions were left running, so timings are
contended diagnostics, not isolated GPU kernel times or a portable FPS guarantee.

- `generate_blackbody_radiance.py`: spectral reference, dense float32 fit,
  boundary checks and perturbed-coefficient negative control.
- `gpu-final/validation.json`: native Planck samples, radial normalization,
  support, noise lattice identity/periodicity, ray allocation and partial
  segment visibility; green/red/green controls and owned API shutdown.
- `depth-msaa-shared/validation.json`: eight perspective/orthographic cases,
  including inside-volume and fully blocked views. Viewport comparisons against
  the preceding per-sample implementation differed by at most one 8-bit level.
- `sampling-wrapped-final/sampling.json`: frozen Modelica-derived scalar field,
  128 vs 512 total samples in side/axial/inside views, exact scalar readback,
  restored-budget control and native framebuffer size. These are display-space
  convergence checks, not a claim of exact numerical radiometric integration.
- 232 authored scenario checks passed: conservation 52, nozzle wall thermal 12,
  stock lander activity/ports 6, RCS starvation 16, Griffin propulsion requirements
  104, Griffin engine-out 20, fuel exhaustion 11 and oxidizer exhaustion 11.
- `griffin-final/review.json`: nineteen exact authored/runtime seed matches,
  powered close/underside/cluster captures and a playing close view. Readiness
  is clear and the GPU log has no validation failures. The playing run retains
  the 1/60 s simulation timestep and advances the fixed tick counter.

At the measured native framebuffer of **2560 x 1568**, the final Griffin fixture
reported 27.35 FPS paused close, 20.46 FPS underside, 40.82 FPS cluster and
**26.45 FPS playing close**. The frozen single-plume stress fixture reported
33.11 FPS side, 21.68 FPS axial and 39.91 FPS inside at 128 samples. Selected
versus 512-sample images differ by at most one 8-bit level in the central
comparison crop, with worst per-channel RMS below 0.285; restored views also
pass. A subsequent wider-viewport audit (excluding UI) found a peak difference
of two levels in the axial view, and one level in the side and inside views. These frame-counter measurements include
the whole app and concurrent workloads. Full-screen volumes remain expensive;
this does not establish a sustained high-FPS production target or an isolated
speedup over the original two-volume implementation.

The complete physical model, assumptions, calibration gaps and test commands
remain in [propulsion-plume.md](../architecture/propulsion-plume.md).

The supplied Griffin fixed-pose visual composition is a render acceptance
fixture, not an operational landing or plume-surface-interaction validation.
Its pre-existing dual-Sun/terrain engine-input warnings are outside this change.
The existing NOBILE03 source inputs were processed through `ProcessDataset` to
refresh a stale derived product and clear readiness; no download was requested.
The prior derived product is retained under `terrain-before/` in the evidence
folder. No running user session was controlled or stopped.

To inspect the paired source revision with the retained test binary from the
simulator checkout, use an owned process and the explicit shared asset root:

```sh
LUNCO_ASSET_ROOT="$PWD/assets" BEVY_ASSET_ROOT="$PWD" \
LUNCOSIM_CONFIG="$PWD/.cache/perf/plume-update/interactive-config" \
LUNCOSIM_EPHEMERAL_SETTINGS=1 .cache/perf/plume-update/bin/luncosim \
  --scene /home/rod/Documents/models/lunar-base-model/twins/astrobotic-griffin-1/scenes/griffin_shader_review.usda \
  --render-quality high
```

The command is a local evidence-workspace example, not a portable installation
path. A normal deployment must package the corresponding shared assets with
its simulator build. The source revision is committed locally in simulator `main` and the Twin;
no push or integration into other worktrees was performed.

## Certainty recheck

A fresh native GPU contract run passed its green/red/green controls; the
blackbody fit check also passed again. Evidence is under
`.cache/perf/plume-certainty/gpu/`. The original 232 authored-check PASS records
were rechecked, not rerun in this audit. No shader or physical implementation
was changed during this recheck.

The playing timing receipt advanced from fixed tick 312 to 557 during a nominal
five-second measurement window: 245 ticks represent 4.0833 seconds of simulated
time. The 1/60 s timestep is preserved, but this loaded run does not establish
wall-clock real-time operation. The short fixed-pose review also does not replace
an operational landing, sustained-load acceptance, other GPU/backend coverage,
or a deployed-build check with the paired asset library. Those remain open.

## Completion pass before landing repair

The wrapped lattice now reuses its six integer axis identities for eight
corners; the shared PCG hash arithmetic remains bit-equivalent. Thermal fitting
uses degree seven instead of twelve and retains the original 0.02% relative
XYZ acceptance bound. The dense float32 maximum is 1.615559e-5. The native GPU
contract, including independent value-noise interpolation comparisons and
thermal reference samples, passes (`plume-completion/gpu-optimized/`).

With the GPU otherwise idle, a 60-second High/2560x1440 powered close-view
baseline measured 49.76 FPS, p95 22.40 ms, and a simulation/wall ratio of
0.99984 (3622 ticks in 60.3765 seconds). This removes the previous uncertainty
about sustained simulation advancement for that fixture; it does not establish
150 FPS. This baseline preceded the final equivalent hash factoring and
lower-degree thermal approximation. A later diagnostic was contention-affected
by a newly started user session, and is not accepted as a paired speedup.

The body-fixed leg-camera revision is verified separately by its four views
and camera/body/leg motion receipt. The full landing-stability fixture ran
300 seconds of simulated time, reproduced the near-ground climb-away and
returned INCONCLUSIVE. Landing repair is the next task, not a passing gate in
this camera/plume revision.

The subsequent [contact-cutoff repair](lander-contact-cutoff-2026-11-12.md)
resolves that reproduced landing failure and records its separate production
landing and stability acceptance.
