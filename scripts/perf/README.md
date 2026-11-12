# Performance drivers and diagnostic readers

Keep generated profiling data in ignored `scripts/perf/captures/`, `.cache/`,
or `target/perf/`. Commit reusable tool sources and documentation only.

Launch the production `LUNCOSIM_BIN` from this checkout on a verified free API
port. For diagnostics, build with `--features tracy`, choose a free `TRACY_PORT`,
and start `../tracy/capture/build/tracy-capture` before launching the app. A
non-on-demand Tracy client needs a fresh process for each capture.

Wait for the exact scene and projected target, not just `/api/ready`; the API
can be ready before scene preparation finishes.

`twin_startup.py` owns a windowed production session and measures
`OpenTwin` to physics readiness separately from the settled mission window.
First readiness uses one read-only Rhai evaluation of readiness, Twin identity,
projected root and physics state in the same World snapshot. The driver records
the response time and polls at 50 ms; HTTP and frame scheduling remain a positive
measurement delay. Receipts identify this method as `atomic-world-v1`; earlier
sequential-query timings include additional frames and are not directly comparable.
The latter requires settled terrain, an optional authored handoff signal, five
seconds of readiness, then ten seconds and at least 240 fresh simulation ticks.
It rejects source changes, retired body identity, runtime faults, coupled-object
pauses and render/script failures. Record camera differences and other workloads
before interpreting its frame summaries as comparable graphics measurements.
Receipts retain the exact readiness, body, terrain and handoff facts at a
qualification loss, including source and process/port state after failure.
A capture is requested at the first qualified physics-ready result and its
publication time is recorded separately. Inspect `first-ready.png` as well as
the final mission image; a physics-ready timestamp alone does not prove that
all visual assets are already visible.
Receipts retain the starting frame count and the first-ready engine history so
loading-frame count and recent stalls can be compared alongside wall-clock
readiness. The engine history retains only 240 frames; it does not certify the
maximum stall over a longer load.
For 2K acceptance on Linux, use `--window-size 2560x1440 --min-fps 150`.
Use `--pointer-passthrough` on an interactive desktop to clear only the owned
window's X11 input shape while preserving its visible shape and render target.
The receipt records this setting; pointer events invalidate the isolated run.
The optional sizing path requires `python3-xlib` and X11/XWayland. It resizes
only the window whose native PID matches the owned process and verifies the
active scene camera's physical target throughout the measured window. The FPS
gate requires both mean FPS and p95 frame time to meet the requested budget;
receipts also report the percentage of frames within budget. The settled samples also retain the native
`engine-health` exposure, including aggregate Modelica worker work per
completed physics tick, its exact tick denominator and participant count. Keep
that attribution metric separate from the largest participant mean and from
coupled-step wall time. Missing frame
history rejects the run. `--min-fps` cannot accompany `--tracy-port`: record
profiling and product acceptance in separate processes. A binary hash identifies
the measured artifact; the checkout HEAD alone does not prove how a retained
binary was built.
By default the driver opens the Twin's current default scene with `OpenTwin`.
Use `--scene scenes/example.usda` to open an exact Twin-relative scene through
`OpenFile`, which mounts its owning Twin without changing the manifest. Receipts
record the command and path; keep them consistent when comparing runs.
Use `--max-load-seconds 5` to require physics readiness strictly below five
seconds. The driver still completes mission and rendering checks and writes its
receipt before reporting a failed load budget. Report cache conditions separately;
this interval excludes application bootstrap and the later landing milestone.
Bootstrap qualification requires the requested startup document, committed scene
epoch, clear readiness and a spawned scene marker before the timed open. The
receipt records those facts and bootstrap duration separately; older receipts
without `bootstrap_qualification` may overlap startup-scene preparation.

```sh
LUNCOSIM_BIN="$PWD/target/debug/luncosim" python3 scripts/perf/twin_startup.py \
  --port 5410 --twin /absolute/model/twins/mission --twin-name mission \
  --source-root /absolute/model --body /Mission/Lander \
  --handoff-property landing_handoff --output .cache/perf/mission-startup
```

The output must be ignored by Git. For a separate Tracy run, start the matching
collector first, set `TRACY_PORT` and `TRACY_SAMPLING_HZ=1000`, then add
`--tracy-port <port> --symbols`. The driver verifies the profiler socket against
its owned PID and saves native maps/ELF/symbols for subsequent sample resolution.
With `--symbols`, it records `/proc/<pid>/maps` and the Cranelift perf map at
qualification and before exit. The receipt's `symbol_snapshots` records each
file path, size and SHA-256. `PERF_BUILDID_DIR` selects the perf-map directory;
when unset, the driver sets it to `/tmp` for the owned process. Validate each
saved map against its measurement interval before resolving samples.
For optimized CPU attribution, retain symbols in the exact diagnostic ELF:
`cargo rustc --release -p lunco-luncosim --bin luncosim --features tracy -j 4
-- -C strip=none`. This preserves release optimization and thin LTO while
retaining native function symbols; normal product builds retain their standard
profile. Verify nonempty text symbols before capture (`nm --defined-only`),
retain that executable independently, and match its hash and native load map
when resolving addresses. A stripped release capture can still expose zones
and JIT mappings, but cannot provide precise Rust function attribution by itself.
Profiler timings remain diagnostic; retain a separate regular build/run. Workload
snapshots are retained before launch, at first readiness and after mission
qualification; distinguish restarts during these separate windows.

Keep unprofiled product measurements separate. `ReadExposures` on `engine-health`
provides raw frame time and `engine_revision`; de-duplicate revisions when
sampling. Polling does not observe every rendered frame, so report sampled
coverage and concurrent workloads rather than claiming complete frame capture.

## Native shadow input comparisons

`SceneCameraAudit { shadow_inputs: true }` provides compact native cascade
matrices and candidate membership/pose/mesh fingerprints. Save one owned
process's responses as JSONL records with a `phase` string and the query data
in `audit`, then compare consecutive host frames:

```sh
python3 scripts/perf/shadow_input_changes.py .cache/perf/shadows.jsonl \
  --output .cache/perf/shadow-changes.json
```

The reader accepts gzip input, excludes 2D/non-active scene-camera entries,
and reports duplicates/gaps instead of treating sparse polling as consecutive
coverage. Matching fingerprints and matrices establish diagnostic candidacy
only: shader/material revisions, deformation, atlas allocation and GPU state
remain outside this snapshot. Use raw-pose pages (offset plus limit up to 64)
for individual inspection. Query load affects the observed workload; these
records are separate from normal FPS acceptance.

## Interaction ownership

Pointer movement reaches Rhai only while a document owns a registered
interaction. Click context carries that same owner as `active_pointer_move`.
Idle primary clicks perform no route queries. Active route placement resolves
its explicit route and preview target; semantic add-point gestures may discover
routes. This bound is independent of the model hierarchy size.

## USD source isolation

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

## Saved CPU samples

The native CSV exporter reports instrumented zones. `tracy_samples.cpp` exports
sampled CPU stacks through Tracy's own reader and symbol tables. Build its
server library from the same Tracy release as the capture; the versioned CSV
executable alone does not establish the version of its adjacent static library.
The Bevy 0.20 client uses Tracy 0.14.1. Use an unmodified matching tool checkout:

```sh
cmake -S ../tracy-0.14.1/csvexport -B ../tracy-0.14.1/csvexport/build \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER_LAUNCHER=sccache \
  -DCMAKE_CXX_COMPILER_LAUNCHER=sccache
cmake --build ../tracy-0.14.1/csvexport/build --target TracyServer -j4
bash scripts/perf/build_tracy_samples.sh ../tracy-0.14.1
target/perf/tracy-samples "$CAPTURE" "$OWNED_PID" 0 "$BEGIN_NS" "$END_NS" \
  > .cache/constructor-samples.csv
```

Arguments require the recorded process PID, an OS thread ID (`0` selects all
threads), and a half-open capture-relative nanosecond interval. The exporter
rejects mismatched ownership, unreadable/version-mismatched captures and empty
sample selections. Check its terminal exit before using the CSV. Its stderr
receipt includes the actual sampling period and selected sample count.

Append an exact instrumented zone name to select only CPU samples inside that
zone on the same thread, within the requested time interval:

```sh
target/perf/tracy-samples "$CAPTURE" "$OWNED_PID" "$TID" "$BEGIN_NS" "$END_NS" \
  cosim_exchange_write > .cache/write-samples.csv
```

The filter reads complete native zone events, merges overlapping instances and
counts each sample once. It excludes unfinished zones and rejects a missing
zone or a zone selection without CPU samples. Check `matched_zones` in stderr
alongside the native zone CSV; the exact-name filter is case sensitive. An
unfiltered frame window also contains unrelated work and cannot by itself
attribute that work to one child zone.

`csv_thread` is Tracy's compressed thread ID used by the native zone CSV;
`tid` is the OS thread ID. Select the constructor's thread as well as its time
interval. Each stack contributes `samples` once, repeated on its frame rows;
group by `(tid, callstack)` before counting samples. For inclusive function
counts, count each function once per stack. Unknown symbols and absent frame
metadata remain explicit; source locations may still identify their owner.
The `image` column retains Tracy's native module identity. For unresolved static
symbols, resolve addresses only against the exact captured ELF and its recorded
load bias; another build's symbol offsets are not interchangeable. Anonymous
JIT frames can lack both an image and a recoverable caller. Such incomplete
stacks cannot establish inclusive caller coverage.
The maintained Cranelift JIT records function ranges in
`$PERF_BUILDID_DIR/perf-<PID>.map`. For a separate owned diagnostic, preserve
that exact process's map after preparation, together with its current memory
mappings. Do not assign stale, overlapping or unmapped
ranges to samples by guessing; admission and code-arena retirement can change
which ranges are live. The flag records JIT names and does not restore stripped
static ELF symbols. Its preparation-time file writes exclude it from a clean
startup-performance comparison.
Validate every JIT-map row before attribution. In the current Cranelift writer,
concurrent constructors can interleave the formatted fields of separate rows.
A successfully completed capture does not make a malformed symbol map usable.
These CPU samples attribute diagnostic work, not wall time or product FPS.
