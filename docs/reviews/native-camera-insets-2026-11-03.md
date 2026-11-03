# Native camera insets and recording readiness

The route scene needed a body-wide Moon view and a projected mission flag.
The existing camera feed owned the correct GPU image but exposed it only in a
floating workbench window. The app now binds that same handle to an admitted
HUI node tagged `camera_feed`. A descendant tagged `camera_marker` projects one
exact live prim path into the image. No screenshot stream, second renderer,
planet pose override or guessed site coordinate is introduced. Projection is a
frustum check, not an occlusion assertion. The generic runtime UI crate owns
pending-image readiness; the app adapter owns feed/prim resolution.

A newly allocated render target is not evidence of a useful scene image.
`minimum_scene_items` explicitly distinguishes a required geometry observation
from an intentional empty/sky observation. A render-world receipt follows
submission and requires ready pipelines across opaque, alpha-mask and
transparent phases. The planetary LOD owner separately certifies a complete
material-ready resident draw cover, while finer refinement can continue.
The existing visual preparation gate consumes both facts. Missing visible
feeds publish structured diagnostics and block recording without inventing a
replacement image. `visual_pending_reasons` explains the blocker; the loading
overlay's acknowledgement is a separate status field.

The green native gate uses DEBUG binary
3228886e42b828bab65feeab68481a43febaa4e307f07572643634f4df375c28,
built on latest main 678c60d4e, in an owned headful 2560×1440 session on API
4493. All 70 images contain Moon pixels, including frame zero; the minimum
400×400 image-region average is 36.5951 Y (encoded black is approximately 16).
Removing the source holds the frame count; restoration resumes it. Both rover
models report Running with 703 values each; both panel controllers report
Running with three values each. Runtime diagnostics report zero errors and
warnings. This bounded native recheck does not replace the separate complete
route capture's 24 physics checks and image/tick receipts.

The full route source uses binary b51b24df2e7044dd66a2ca797d9339b9c76531bc68e84300623cdbc2c52a96ef.
All 1,598 images contain the Moon image. Failed experiments with clear-only
startup frames remain rejected and preserved. Unit checks cover geometry-required
image admission, invalidation of an already-ready surface by a pending descendant,
and editor-only camera-guide visibility. The guide test was observed red before
the policy change and green afterward. Normal views and recordings exclude
Cinematic path guides, even when their authoring checkbox defaults on. The HUI
mission flag is independent and retained.

Evidence is under the Griffin V13 R4 package's `renders/evidence/`:
`native-camera-map-gate-r4-latest-main.json`,
`map-native-ready-proof-r4-v9.mp4`, `latest-main-runtime-r4.json`,
`native-camera-map-gate-r4.json`, and `record-routes-headful-28.json`.
The user authorized local work without Trello. No push was made.
