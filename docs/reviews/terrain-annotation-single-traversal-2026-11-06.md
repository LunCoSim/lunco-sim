# Terrain annotation traversal

The shared terrain shader resolves material and overlay strokes in one sparse
index traversal. Separate winning tints retain segment order on equal coverage;
material marks are applied before lighting and overlays afterward. Diagnostic
mode selects across both kinds in the same segment order. The owner and texture
format remain those described in [USD-driven visuals](../architecture/50-usd-driven-visuals.md).

On the Griffin landing fixed view at 2560×1440, alternating original and
single-traversal shader prototypes measured an opaque-pass mean of approximately
9.4 and 6.6 ms respectively in Tracy. The actual shared implementation measured
6.685 ms paused and 6.896 ms playing in a separate capture. These GPU pass
times are diagnostic, not end-to-end FPS measurements. The earlier comparison
uses shader reloads, which retire the shadow raster optimization in that session.
A clean, uninstrumented single-traversal launch before the segment-box change
measured 54.4 FPS with clear runtime diagnostics. A separate original/new pair
measured 20.9/31.2 FPS while another
checkout was compiling and the measured physics service times also changed;
that pair is not evidence of an FPS gain. The later clean comparison is below.

The production image comparison in `target/perf/fps-20261106/annotation-parity/`
alternates the original and new shaders at a frozen landing pose. The captured
terrain and scene region is pixel-identical across both repetitions; suppressing
the annotations changes the image, confirming that marks are visible. This
view alone does not cover every possible overlapping stroke or wheel trail.
`annotation-final-style-parity/` transiently forces the live stroke records
to be lit, then assigns mixed lighting modes, and compares original and final
shader captures. Both terrain/scene comparisons and the diagnostic view are
pixel-identical; both forced styles visibly differ from the unmodified view.
An earlier diagnostic-view capture had small scattered differences, mostly one
8-bit level, so the single-frame comparison does not establish parity for all
views or temporal conditions.
`annotation-single-overlay/` passes production, slope, LOD and restored
terrain GPU phases with depth, normal and motion prepasses. The runs preserve
all 548 Twin source files and close their owned API sessions.

Before evaluating a segment's distance and coverage, the shared traversal now
rejects fragments outside a conservative box expanded beyond the segment's
radius and derivative footprint. This preserves coverage while avoiding the
distance calculation for distant strokes. In the alternating `annotation-aabb-tracy/`
capture, baseline opaque-pass means were 6.591, 6.352 and 6.474 ms; bounded
means were 5.524 and 5.246 ms. The captured view was pixel-identical across
all phases. A fresh production capture with the normal shadow optimization
measured 5.331 ms paused and 5.626 ms playing for the opaque pass, and 4.248 /
5.064 ms for early directional shadows. These are diagnostic GPU results, not
an FPS acceptance result.

An explicit early return for an empty root cell produced identical images but
no repeatable improvement in `annotation-early-tracy/`, so it is not retained.
The diagnostic `annotation-radius-tracy/` uses a loose 10 m pre-bound to avoid
some per-segment style reads; baseline opaque means were 5.421, 5.431 and
5.389 ms, while candidate means were 5.951 and 5.986 ms. Its image and a
visible-record radius sentinel matched the baseline, but the candidate was
slower and the bound is not part of the implementation.
The procedural-noise hash rewrite and disabling the horizon visibility path also
showed no repeatable improvement in this view. Those observations do not rule
out different scenes or devices.

The diagnostic in `annotation-density/` colors fragments by the selected
root node's segment count. Most visible terrain in this view selects a leaf
with more than 16 segments. The index currently subdivides above 64, so large
screen areas can inspect many distant strokes. Any more aggressive subdivision
must preserve the existing hard density limit, bounded node allocation,
antialiasing and transactional updates; simply lowering the hard limit would
reject valid overlapping strokes.

The `annotation-leaf-bounds/` probe shows that the union of those leaf strokes
occupies only a small portion of the populated screen area. In
`annotation-leaf-axis/`, a conservative interval on the narrower terrain axis
rejects approximately 70% of the captured scene pixels. The production index
now publishes that interval together with each leaf list. It is derived from
the segment union, expanded and rounded outward at the GPU boundary, and rolled
back with the same transaction; extreme coordinate ranges use a full-range
interval. Runtime and performance acceptance of this additional change must
use a rebuilt binary with the matching shader. The focused owner test passes
axis selection, width changes, rollback and the extreme-range interval; the
existing 10,000-segment transactional index test also passes.
`annotation-leaf-style-parity/` uses the rebuilt normal binary and matches the
original shader pixel-for-pixel in lit, mixed and diagnostic captures, with
clear runtime diagnostics. `annotation-leaf-camera-parity-fixed/` also matches
the original and restored captures pixel-for-pixel at closer and higher camera
positions; disabling annotations visibly changes both views. The GPU timing
and normal FPS checks remain separate from these image results. The rebuilt
binary also passes all four phases of `annotation-leaf-overlay/` with depth,
normal and motion prepasses. In the same-process `annotation-leaf-ab-tracy/`
capture, opaque-pass baseline means are 5.162, 5.179 and 5.652 ms, while the
interval-enabled means are 2.670 and 2.896 ms; all five captured images match.
A fresh production capture without shader reload measures 2.764 ms paused and
3.058 ms playing for the opaque pass, with 4.385 / 4.868 ms for early shadows.
These GPU timings include Tracy and do not replace the normal FPS measurements.
With leaf culling active, `annotation-leaf-segment-ab/` measures 2.708, 2.696
and 2.909 ms with the per-segment box, versus 3.046 and 3.124 ms without it;
all five captures match, so both conservative checks are retained.

The existing Griffin landing-annotation gate does not pass on the current Twin:
its readiness helper attempts to serialize a scenario state containing a SysML
handle, and the authored test declares an inactive `EgressRampPort` as an
entity dependency. A temporary readiness and compact-egress adaptation started
but timed out without a verdict while held. Neither attempt is counted as a
passing gate; no Twin source was edited.

The uninstrumented `annotation-paired-normal-final/` uses the rebuilt binary
and alternates the original and optimized shaders in four separate qualified
landing-scene sessions. Mean FPS is 49.46 / 49.21 / 46.54 / 51.62 for
original / optimized / original / optimized; p95 frame times are 25.56 /
24.47 / 28.04 / 25.55 ms. All sessions preserve Twin source hashes, report
zero runtime diagnostics and release their owned API ports. The variance is too
large to attribute a stable end-to-end gain, and the 100/150 FPS goals remain
unmet. GPU monitoring shows markedly less GPU utilization and power in the
optimized windows, motivating a fresh CPU and frame-pacing investigation.

In the uninstrumented same-session `annotation-pause-verified/` experiment,
alternating paused / playing / paused / playing / paused windows measure
84.02 / 58.92 / 84.83 / 57.39 / 84.75 FPS. The playing windows advance 617 and
616 fixed ticks in approximately 10.2 seconds each, with the normal eight
substeps; paused windows hold their tick. No compilation appeared in the
recorded phase-boundary workloads, diagnostics were clear and Twin source
hashes matched. GPU monitoring measured roughly 82% utilization at 2.3 GHz
while paused and 57% at 1.6 GHz while playing. The scene evolves between
windows, so this is evidence of a simulation-related frame limit in this
workload, not a controlled estimate of one CPU system's cost.

In two uninstrumented ten-second warm windows of the same landing workload,
MainPropulsion averaged 1.477 and 1.528 ms per worker job over 607 and 605
jobs. The sum across all 13 participants was 3.150 and 3.259 ms per tick;
these jobs execute concurrently, so that sum is not coupled-step wall time.
The fixed-tick service p50/p95 was 3.497/4.394 and 3.553/4.421 ms. Another
simulator process was active during these windows, so they are not an isolated
machine benchmark. The scene advanced at the normal 60 Hz and eight substeps,
reported no runtime diagnostics, preserved all 548 Twin source files, and the
owned session and port were released. Evidence is in `modelica-warm-cohort/`.

A further `terrain-site-cull-tracy/` experiment skipped continuation-map samples
where the site blend was zero. Original / candidate / original / candidate /
original opaque-pass means were 2.918 / 2.913 / 2.848 / 2.733 / 2.656 ms;
all captured scene regions matched exactly. The changing baseline does not
establish a gain, so that candidate is not retained. The source hashes matched,
runtime diagnostics were clear and the owned API and Tracy ports were released.

The `terrain-cost-current/` paused diagnostic removes one shader stage at a
time after the annotation changes. Opaque-pass means for original / bump-disabled
/ original / site-textures-disabled / original / native-lighting-disabled /
original are 2.651 / 2.185 / 2.652 / 2.605 / 2.654 / 1.712 / 2.657 ms. The
four original scene regions are pixel-identical; disabling bump visibly changes
the image, and disabling native lighting changes it substantially. Disabling site
textures produces no image difference in this view, so that phase does not prove
a general optimization. These ablations deliberately alter or suppress shading,
and none is retained. GPU times are diagnostic, not product FPS. All 548 Twin
source hashes match, runtime diagnostics are clear, and the owned app and Tracy
ports are closed.

A separate `terrain-normal-reuse/` hot-reload trial explicitly reused the
normalized world normal and DEM-local position already computed in the fragment.
Original / candidate / original / candidate / original opaque-pass p50 times
were 2.651 / 2.652 / 2.647 / 2.647 / 2.656 ms. The five scene regions matched
pixel-for-pixel. The compiler already appears to reuse this work in this view;
the candidate is not retained. Source hashes and diagnostics passed and the
owned API and Tracy ports were released.

The `terrain-dark-verified/` fixed-view probe colors the live layered terrain
and separately colors only fragments with a valid Sun direction and a
non-positive Sun-facing shading normal. The all-terrain control marks 3,458,735
pixels in the 2560×1360 scene crop; all of those have a valid Sun direction and
none meets the non-positive test. The dark-only and restored images exactly
match the baseline. Thus a back-facing Sun shortcut cannot improve this terrain
view; this observation does not characterize other scenes or materials. Source
hashes and diagnostics pass, and the owned API session exits.
