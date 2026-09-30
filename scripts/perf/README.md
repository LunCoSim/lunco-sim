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
