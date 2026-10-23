# Offline path tracing integration

Status: implementation under runtime qualification. Do not accept Roadster
production takes as path traced until compatibility and image gates pass.

## Requested result

The normal renderer remains raster PBR. Settings → Graphics selects a renderer
and a positive samples-per-pixel target. The active Twin can override them with
`render.backend` and `render.samples_per_pixel`. Physical source assets stay
shared across renderers; the renderer must not change equations or dynamics.

An offline output image consumes one complete simulation duration of `1/fps`,
then freezes that state while gathering path samples. GPU rendering throughput
only changes wall-clock production time. At 60 fps and 60 Hz physics, advancing
images still have one completed physics cycle each; explicit analysis holds
have zero. No sparse-frame interpolation or real-time denoiser is required.

## Owners and implementation

- `lunco-render::backend` owns persisted renderer intent, Twin override facts,
  observable admission status and revisioned image-sampling tickets.
- `lunco-luncosim-ui::render_settings` projects the active Twin manifest on
  workspace changes. Closing/replacing a Twin removes its override through the
  same projection. Invalid values remain errors rather than selecting a default.
- `lunco-render-bevy::backend` installs upstream `RaytracingScenePlugin` and
  `PathtracingPlugin`, checks native GPU features and binds compatible geometry
  and the active camera. It does not install `SolariLightingPlugin` or DLSS.
- `lunco-capture` issues a fresh image ticket only after the ordinary fixed-step,
  participant and visual barriers. The clock stays frozen until a matching render
  acknowledgement reaches the configured sample count. A stale acknowledgement
  cannot finish a later image. Capture locks renderer configuration for the take.
- The workbench displays the same settings and admission errors. Capture status
  exposes the selected backend, sample target/progress and sampling errors.

Ray tracing uses private derived GPU mesh assets with the upstream vertex/index
layout; it never edits a shared raster mesh. The mesh cache participates in scene
teardown. Camera MSAA/HDR/texture usage are overridden only for the ray-traced
view, with raster state retained for restoration. The reference path tracer
resets accumulation for each output image, including stationary-camera shots.

Hook review: selection comes from persisted preferences and authored Twin facts.
The Rust code applies those typed facts, validates backend invariants and counts
render submissions. It contains no Roadster or episode selection policy.

## Dependency identity

The configured registry does not provide `bevy_solari` 0.20. The integration
selects it from the exact maintained Bevy revision already used by `bevy_shader`.
The user approved vendoring only this crate and a public ordering hook on
23 October. Its package manifest uses the declared released Bevy 0.20 identities;
Cargo resolves one ECS/render graph. The sole runtime source patch exposes
`PathtracingSystems::Trace`. See
[`VENDOR.md`](../../third_party/bevy_solari/VENDOR.md) for the source identity,
licences and exact patch.

## Compatibility boundary

The reference path tracer accepts opaque `StandardMaterial` ray meshes.
The application wrapper invokes Bevy's public transparent pass after tracing
and before early post-processing, so effects reach bloom and tonemapping. It
preserves primary-view opaque depth. Transparent effects remain forward rendered
and never enter path accumulation or secondary reflections. Raster and preview
views do not run this additional pass.

Opaque custom shaders, alpha testing, order-independent transparency and active
point/spot/rectangle lights remain explicit capture errors. Hidden lights and
lights outside the selected view's layers do not contribute. The Roadster's
material and lighting composition still require direct qualification; building
the wrapper alone does not qualify a film take.

## Qualification

- Focused render/capture `cargo check`: passed.
- Sampling-ticket and settings-resolution tests: 2 passed.
- Regular `cargo build --bin luncosim -j 4`: passed.
- Native full-course source: 1,980 images, one completed 60 Hz physics cycle per
  image. This is raster footage from the canonical Twin, not a ray-tracing test.
- Native GPU reference path tracer: indirect emissive lighting and reflections
  visible; three 60 fps images with one completed physics cycle and at least
  64 submitted samples per image passed (`roadster-offline-pathtracer-v17-session.log`).
- Native backend/reload round trip: path tracer → raster → path tracer passed,
  including clean UI images after loading (`roadster-reload-check.log`).
- Windowed hybrid fixture: emissive lighting/reflection and native transparent
  plume behind a solid occluder are visible. Moving the plume to a new position
  leaves no old primary-view plume in the next captured image (`v38`).
- Unsupported local light and opaque custom shader requests now end with
  `failure`, `active:false`, `pending:false`, and zero saved images (`v39`).
  A valid three-image take succeeds afterward. Replacing the probe Twin with
  the regular Roadster clears the override to raster /128 samples.
- Actual offscreen output: three images at 1280×720 and 60 fps show path-traced
  emissive reflection and the forward plume (`v42`). The native Rhai gate then
  passes three images, 64 ready submitted samples/image and one completed
  60 Hz cycle per advancing image in the offscreen host (`v43`).
- Full Roadster content/lighting qualification remains pending: its opaque
  procedural tire shader and authored local lights are outside this renderer's
  supported contract. No production film is claimed path traced yet.

The path-traced UI host originally retained old loading-card pixels because its
main-texture usages differed from the scene camera. The shared framebuffer
invariant was repaired and integrated separately; see
[UI target verification](shared-ui-target-2026-10-23.md).

Native fixture authoring also exposed an independent document issue in the
previous binary: `AddPrim` on a newly-created stage rejected with `no file format
recognizes the content of anon:...`, while attribute edits succeeded. No USD text
was patched. Retest through the new normal build before diagnosing that owner.

## Recording boundary repairs found by negative/host tests

Unsupported renderer intent is terminal rather than a visual-loading wait.
Material assets that are still preparing use `RendererStatus.pending_reason`;
unsupported formats/features use `error`. The shared recorder observes renderer,
sampling and runtime faults, cancels an armed or active take through its ordinary
teardown, and publishes `failure`. A new request clears that per-take result.
CLI zero-image failure must exit with error after teardown, while an idle API
host with no take remains live.

The windowless authored camera remains active with `output_mode:Skip` for pose
and LOD. Its mirror writes the image. Backend binding and sample acknowledgements
now select real color writers, excluding editor preview cameras. Both active
source and mirror still run Bevy prepasses and share a target, so the backend
keeps their HDR, texture usages and MSAA consistent. Only the writer owns
`Pathtracer`. Raster state is restored on backend removal.

The offscreen mirror uses `set_if_neq` for pose/cell synchronization: assigning
an identical transform marked `GlobalTransform` changed on every submission,
which correctly made upstream reset accumulation to one sample. This repair
preserves change detection; it does not bypass upstream resets.

Rejected evidence: v39 offscreen returned a sample count from the non-writing
source but captured raster pixels; v40 hit a shared-target MSAA validation
error; v41 correctly waited at one sample because every pose copy looked like
motion. None qualifies a path-traced recording. v42 pixels and v43 native
receipts replace these failed attempts.

Local evidence: `/home/rod/.cache/roadster-hybrid-v39-proof/`,
`roadster-hybrid-native-v43.log`, `roadster-hybrid-offscreen-v42/`, and the
associated source/binary proof in the campaign delivery records. These are
bounded renderer fixtures, not Roadster stunt qualification.

## Open zero-image process shutdown issue

The v44 CLI negative light take publishes terminal failure, saves no frames,
and reaches `recording drained after terminal failure (0 frames)`. The process
then aborts in cleanup with `free(): invalid pointer` rather than exiting
normally with code 1. Do not count that as graceful CLI rejection. Windowed
API terminal rejection and recovery are verified separately. Recheck this
boundary after integrating current main; its runtime ownership changes are
not present in this test binary. This remains an experimental backend checkpoint.
