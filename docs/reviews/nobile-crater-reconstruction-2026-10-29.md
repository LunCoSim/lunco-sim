# Nobile crater reconstruction

The Nobile crater walls cross missing stereo-DEM coverage. Height-only boundary
constraints flatten curved walls inside those gaps. The orthophoto has unmapped
zero-valued pixels at several of the same craters; treating those zeroes as
radiance during resampling adds an artificial dark outline.

`lunco-assets-processing::process` owns both repairs. `fill_dem_voids` minimizes
squared discrete curvature at missing nodes, with all measured heights fixed.
The bounded, diagonally preconditioned conjugate-gradient solve rejects invalid
or unconverged results. The existing 64-ring admission bound remains in force.
The albedo processor converts reserved zero radiance to missing coverage before
the shared valid-sample resampler. Uncovered colour stays neutral, and mapped
texture remains available. Processing pipeline version 10 invalidates bake stamps.

This is offline numerical processing rather than replaceable scene policy, so it
does not introduce a Rhai hook, rendering path, shader setting, or dependency.
The real consumers are the native DEM/albedo processors and the existing terrain
oracle and material readers. Processing retains its cancellation and atomic
commit boundary. The terrain's USD binding, 4 m postings, 2048 m extent, vertical
datum and navigation coordinates are unchanged.

The curved-crater regression failed at the boundary slope before the repair.
The uniform-radiance regression also failed: a single unmapped zero produced a
dark ring after resampling. All six focused `void` tests now pass, including
measured-node preservation, planar and flat surfaces, curved walls, neutral image
coverage, and rejection of an oversized gap. Biharmonic image completion is also
documented by [scikit-image](https://scikit-image.org/docs/stable/api/skimage.restoration.html#skimage.restoration.inpaint_biharmonic);
this implementation uses the existing native processor and f64 arithmetic.

The native asset CLI regenerated the 513 by 513 Nobile raster. All 261,442
measured nodes are bitwise unchanged. Only 1,727 originally missing nodes change,
by at most 2.9411011 m relative to the installed input. Discrete curvature energy
around those gaps falls from 412.04944 to 259.14870. The generated DEM SHA-256 is
`1a5412b72324fc5a5bfabebef5366b92ca61e3b629a28604a6864d2f2e3533b0`.
The regenerated albedo SHA-256 is
`ce6fd32acdf8b3412ff27e0d5f7292630e238d67bc0b7df8ecf82b144cfee7c9`.
Both maps are installed in the Twin's authored and cached terrain locations.

The production oracle read all 1,727 void postings from the old raster exactly,
then read all repaired values within one micrometre after an API scene reload.
Matched southern, overhead and northern views use the production binary and
an owned 1280 by 720 window. The review composition was authored through USD
document operations and references the Twin's actual terrain and lighting.
Evidence is under `.lunco/crater-review-2026-10-29/`, including the raster checks,
live readbacks, camera/LOD records and before/after captures.
The final cold production run uses both regenerated maps, reads all 1,727
repaired samples correctly, and has zero runtime errors or warnings and no GPU
validation errors. The owned process exits through the API and releases port
4794. `final-validation.json` records that boundary.

The integrated production check also loads the actual
`griffin_1_surface_ops.usda` mission after retaining the newer main-branch
renderer and native physics changes. It reaches readiness with no world hold,
fault, pending work, runtime diagnostics or GPU validation errors. The lander
is held at its 60 m startup pose for `griffin-startup-final.png`. The owned
session exits through the API and releases port 4796;
`integrated-validation.json` records the binary hash, source commit, readiness,
lander pose and shutdown state.

Focused validation commands:

```sh
cargo test -p lunco-assets-processing --lib void -j 4
cargo test -p lunco-luncosim -p lunco-modelica-ui --lib installed_file_finishes_opening_without_rendering_a_canvas -j 4
cargo build -p lunco-luncosim --bin luncosim -j 4
target/debug/lunco-assets process --twin "$TWIN/.cache/nobile-terrain-preparation" -a dem
target/debug/lunco-assets process --twin "$TWIN/.cache/nobile-albedo-preparation" -a albedo
```

Filled heights remain estimates. The roughly 1 m imagery and 4 m elevation grid
do not supply new measurements inside source voids. The terrain review scopes
its numerical checks to the native processors and its visual checks to the
Twin's composed terrain and material, independently of vehicle dynamics.
