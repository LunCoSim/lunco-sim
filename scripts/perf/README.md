# Native input performance drivers

Launch the production `LUNCOSIM_BIN` from this checkout on a verified free API
port. For diagnostics, build with `--features tracy`, choose a free `TRACY_PORT`,
and start `../tracy/capture/build/tracy-capture` before launching the app. A
non-on-demand Tracy client needs a fresh process for each capture.

Wait for the exact scene and projected target, not just `/api/ready`; the API
can be ready before scene preparation finishes.

```sh
python3 scripts/perf/summer_space_school_motion.py --port 4748 \
  --move-seconds 10 --rotate-seconds 10 --rotate-degrees 360
python3 scripts/perf/left_click_burst.py --port 4748 \
  --first-path /Traverse/Rover --second-x 640 --second-y 600
```

The movement driver reports actual avatar displacement, drives semantic
movement input, and rotates with raw mouse motion while the configured look
button is held. It measures native yaw and releases the button on errors.
Each injected turn waits for observed yaw within the rotation deadline;
command admission alone does not mean the controller has consumed the input.
The click driver resolves its first target using
`viewport_position`; additional coordinates are logical window coordinates and
must match the current viewport. Both drivers use production commands and
window input. They never invoke pointer hooks directly or edit USD assets.

Keep unprofiled product measurements separate. `ReadExposures` on `engine-health`
provides raw frame time and `engine_revision`; de-duplicate revisions when
sampling. Polling does not observe every rendered frame, so report sampled
coverage and concurrent workloads rather than claiming complete frame capture.

## Interaction ownership

Pointer movement reaches Rhai only while a document owns a registered
interaction. Click context carries that same owner as `active_pointer_move`.
Idle primary clicks perform no route queries. Active route placement resolves
its explicit route and preview target; semantic add-point gestures may discover
routes. This bound is independent of the model hierarchy size.

## USD Editor tabs

`usd_editor_tabs.py` owns a fresh production session and opens each exact source
as an Editor preview. It matches `ListOpenDocuments` file origins to preview
identities, waits for projection readiness, then samples each settled Visual
tab and the final authored/composed text views. It records physics counts before
and after, API state, frame samples, and a screenshot. Every measurement
checkpoint rejects retained errors from `RuntimeDiagnostics`, including event
delivery holds that leave physics and readiness operational. It does not edit sources.
It dispatches `OpenUsdSourceDocument`, not the workspace-opening `OpenFile`.

```sh
python3 scripts/perf/usd_editor_tabs.py --port 4749 --scene /absolute/scene.usda \
  --file /absolute/first.usda --file /absolute/second.usda \
  --output target/perf/editor-tabs
```

This Linux driver verifies the launched process through `/proc`. Record other
running workloads and use a separate unprofiled run for product acceptance.
Pass `--compare-first` to keep the first Visual view and camera visible while
additional files remain open. Compare its settled windows and physics topology
checkpoints, then inspect authored/composed text with all previews retained.
Add `--paired-first` to close only the extra previews opened by this driver,
measure the warmed first view alone, then reopen the same sources and measure
that first view again. Frame samples and per-window physics snapshots are saved
separately as `.samples.json` and `.windows.json`.
The first settled source is explicitly framed. Equal-camera windows record the
focused view and reject changes to its identity, target, distance, or projection.
Closed preview leases are reopened through `OpenUsdPreview`; their source
documents remain open and unchanged.
Paired runs capture the warmed one/many Visual views, then return from text to
the same Visual view without editing its lights. This exercises parked-view
reactivation alongside the camera-guarded editor-count comparison.
Screenshot requests wait for a fresh published file before proceeding or
closing the owned session; an existing stale artifact is rejected.
Each window checks full readiness, unchanged physics topology and advancing
steps at one-second checkpoints and at its end. A late hold, fault or stalled
checkpoint rejects the window. The exact source scene must be mounted and its
time selection committed before a five-second clear-readiness soak; the active
Twin must remain unchanged through each window. Camera comparisons include orbit yaw/pitch,
projection, target, distance, orthographic scale, measured image rectangle and
window scale factor; the visible pose must also remain fixed within a window.
These additional API queries contribute diagnostic sampling overhead.
Run the negative measurement checks with
`python3 -m unittest discover -s scripts/perf -p test_usd_editor_tabs.py`.
The standard per-file mode intentionally changes the visible asset and is not
an equal-camera editor-count comparison.

Run the source-isolation Rhai verdict separately through the existing
production asset-launch command:

```sh
python3 scripts/api/test_usd_source_isolation.py --port 4750 \
  --scene assets/scenes/luncosim/sandbox_scene.usda \
  --source assets/vessels/rovers/skid_rover.usda \
  --selection-path /SkidRover/Motor_RR \
  --log target/perf/source-isolation.log \
  --screenshot target/perf/source-isolation.png
```

The driver waits for the exact mounted scene and a five-second clear-readiness
soak, supplies all three required scenario parameters, and requires the
eight-check `USD_SOURCE_ISOLATION` verdict before API shutdown. It can capture
the selected preview with its exact viewport/selection state;
the shared `ProductionSession.capture_screenshot` waits for fresh publication.
It does not establish the startup latency milestone, which additionally requires
all startup producers' initial admission passes to complete.
