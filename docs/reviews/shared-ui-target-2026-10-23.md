# Shared UI target: loading-card retention

Status: production-runtime image verification passed.

The reference path tracer adds `STORAGE_BINDING` to its camera's
`CameraMainTextureUsages`. Bevy keys intermediate main textures by target,
usages, format and MSAA. The workbench synchronized MSAA and HDR but omitted
usages, so its load-preserving UI camera obtained a private texture. Earlier UI
cards accumulated there even after the status bus and capture UI state were
clear. These were stale pixels, not multiple active loading operations.

`lunco-workbench::viewport::sync_egui_host_framebuffer` now copies the complete
camera texture configuration. It observes usage changes and component removal,
and runs after camera binding and viewport reconciliation in PostUpdate.
`lunco-render::RenderCameraBindingSet` is the shared binding boundary. Rust owns
this framebuffer invariant; no Twin/film policy or status suppression is involved.

The regular tutorials debug build was exercised on the existing HDRI sphere
scene copied through native SaveAsDocument into a temporary Twin. The active
Twin selected reference path tracing at 64 samples per output image. A real
DomeLight/scene load produced the progress states; no synthetic status clearing
was used.

Evidence retained locally:

- Before: `.cache/roadster-stale-progress-before.png` shows several retained
  loading cards while `GetOfflineRecordingStatus.ui_presentation_ready=true`.
- After: `.cache/roadster-stale-progress-after.png` shows the same ray-traced
  test view without retained cards after the real load completes.
- `/home/rod/.cache/roadster-offline-pathtracer-v17-session.log` contains
  `PATH_TRACING_CAPTURE_PASS`: three native images, one completed 60 Hz physics
  cycle per image, at least 64 submitted render samples per image.
- `/home/rod/.cache/roadster-pathtracer-v17/shot_01/take.mp4` is 2560×1568,
  three images at 60 fps. Its raw frames were reviewed for the card artifact.

## Reload ownership and regression coverage

Scene-derived status already has one lifecycle owner: `StatusBus` is a registered
`SceneResource`. Scene teardown clears active progress, history, reverse IDs,
mirror handles and terminal outcomes. Active `TwinClosed` uses the same retirement
operation. Busy IDs remain monotonic, so late drops from an outgoing scene cannot
remove a replacement scene's progress. The existing
`status_retires_at_twin_close_and_scene_teardown` test covers those boundaries.
The workbench renders one current progress notice from that bus; it does not
retain a collection of loading cards.

Framebuffer synchronization must also wake for an explicit viewport binding
change, even if neither camera component changed. The regression test
`egui_framebuffer_follows_binding_replacement_and_backend_changes` covers an
existing-camera switch, storage-binding changes, HDR removal and the no-camera
interval before a reload's replacement camera is admitted. These checks protect
the shared composition invariant rather than clearing or suppressing indicators.

Both focused regression tests passed on 2026-10-23. A native `RestartScene` of
the ray-traced test view also completed with no retained cards; its image is
`.cache/roadster-reload-1.png`. The longer renderer round trip was interrupted
by the test process ending and is not counted as a passed lifecycle run.

The ray-tracing backend itself remains under separate qualification, including
custom surfaces, scene fidelity and film lighting. This UI acceptance does not
qualify Roadster footage or a finished film.
