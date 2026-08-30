# Stable 200 FPS performance handover

> Status: Open · Audience: maintainers working on the luncosim frame loop,
> BigSpace, terrain, rendering, or physics

## Objective

The Summer Space School Apollo scene must run at a stable **200 FPS or better**
on the reference development machine, with High visual quality enabled. That is
a frame budget of **5.0 ms**, including the CPU schedules, physics, UI, render
submission, and presentation work. The target is not an idle-shell number and
must not be achieved by disabling the scene, reducing physics fidelity, or
silently selecting a lower visual preset.

The acceptance result is still open. The strongest clean, non-Tracy High run
available for this handover settled around **80–90 FPS** with approximately
**13–16 ms** frame times. It therefore misses the target by roughly 2.5–3x.
The measured physics solver itself was generally sub-millisecond to a few
milliseconds; the dominant problem is the combined frame loop, especially the
BigSpace high-precision propagation path and CPU scheduling around rendering.

This document is a work handover, not a claim that 200 FPS has already been
reached. Delete this open review once the acceptance criteria below have been
met and the durable design rules have been moved to the relevant architecture
pages.

## Reference workload and reproducible commands

Use the production binary and the real Apollo scene. Do not use an old sandbox
binary, `cargo run`, a synthetic empty scene, or a parse-only validation as
performance evidence.

```sh
cd /home/rod/Documents/luncosim-workspace/main

# Use the repository target and the normal shared Cargo/sccache setup.
cargo build -p lunco-luncosim --features tracy -j 4

setsid env WINIT_UNIX_BACKEND=x11 \
  target/debug/luncosim \
  --api 4133 \
  --no-vsync \
  --no-throttle \
  --log-diag \
  --scene /home/rod/Documents/models/summer_space_school/space-school-twin/sim/scenes/traverse_apollo15.usda \
  --render-quality high \
  >target/luncosim-apollo-high.log 2>&1 &
```

The API port must be free and owned by this run. Check `/api/ready`, wait for
the entity count and terrain work to settle, collect a fixed measurement window,
then send the typed API `Exit` command. Verify that the process and port are
gone before starting another run. Never use `pkill`, overlap runs, or reuse an
owned port.

For a Tracy capture, start the capture before the app and keep the capture in
the repository's ignored capture directory:

```sh
../tracy/capture/build/tracy-capture \
  -o scripts/perf/captures/apollo-high-200fps-YYYYMMDD.tracy \
  -a 127.0.0.1 -p 8086 -f -s 20 \
  >target/tracy-apollo-high.log 2>&1 &
```

The exact API shutdown and process/port verification are part of the run. A
Tracy capture adds overhead, so its FPS is not a clean product-performance
number. It is still valuable here because the user requirement is that the
longest paths be fixed even when observed under Tracy. Report both clean-run
and profiled-run results, clearly labelled.

Do not create custom temporary directories or temporary files. Use `target/`
for logs and the existing `scripts/perf/captures/` directory for profiler
artifacts. Do not export or repeatedly parse the 100+ MB Tracy capture unless a
specific question requires it; inspect the longest paths first with the Tracy
GUI or the existing profiling tools.

## Evidence collected

The evidence below came from the main checkout at `f87dfc020` (which is an
ancestor of the `usd` branch). Startup and settled-state costs are separated;
terrain baking and USD prim observers during startup must not be mistaken for a
steady-state frame regression.

### Clean runtime

- Scene: `traverse_apollo15.usda` from Summer Space School.
- Mode: windowed, X11, High quality, `--no-vsync --no-throttle --log-diag`.
- Settled entity count: approximately **4008**.
- Observed settled diagnostics: roughly **50–113 FPS**, with the stable tail
  commonly around **80–90 FPS** and frame times around **13–16 ms**.
- Avian total step in the clean run was often around **0.7 ms**, so the clean
  frame deficit cannot be attributed to Avian alone.

### Tracy CPU capture

Capture: `scripts/perf/captures/summer-space-school-500fps-high-20260828.tracy`
from the main checkout. The capture was successful, but capture-time FPS and
frame-time diagnostics are not acceptance data.

The most important settled and mixed-window CPU attribution was:

| Path | Approximate attribution | Observed cost / meaning |
|---|---:|---|
| `hp_propagation_worker` | 19.57% | 4.064 s total, 164,979 calls, approximately 24.6 µs mean; BigSpace worker fan-out |
| `hp_propagation_producer` | 12.19% | 2.531 s total; producer/channel overhead feeding the same propagation |
| `propagate_high_precision_channeled` | 3.04% | approximately 1.461 ms mean; the top-level BigSpace propagation envelope |
| Render schedule | 16.81% | approximately 8.103 ms mean; broad schedule envelope, not one leaf function |
| PostUpdate schedule | 13.71% | approximately 6.605 ms mean; inspect contained systems before changing schedule structure |
| Update schedule | 13.09% | approximately 6.307 ms mean; same rule as above |
| `physics_telemetry::retain_physics_telemetry` | 2.62% | approximately 0.871 ms mean; only a secondary target after propagation |
| `prepare_preprocess_bind_groups` | 2.09% | approximately 1.005 ms mean; render preparation |
| `globe_lod::update_globe_lod` | 2.04% | approximately 0.982 ms mean; should be change/revision-driven when stable |
| `prepare_clusters` | 2.02% | approximately 0.975 ms mean; one approximately 39 ms outlier was observed |
| `drive_engine_sync` | 1.96% | approximately 0.941 ms mean |
| `publish_exposure` | 1.94% | approximately 0.930 ms mean |
| `domain_projection::project_domain_islands` | 2.90% | approximately 1.397 ms mean in the mixed capture |

Startup-only paths included `on_usd_prim_added` at approximately 8.84% and
`terrain_tile_bake` at approximately 4.89%. They explain load/settling stalls,
not the stable 200 FPS budget; optimize them separately only if the startup
experience is also a requirement.

### Tracy GPU capture

The GPU was not the first bottleneck in the observed High run:

- `main_opaque_pass_3d`: approximately **1.894 ms mean**.
- `main_transparent_pass_3d`: approximately **0.513 ms mean**.
- `bin_unpacking`: approximately **0.366 ms mean**.
- `msaa_writeback`: approximately **0.330 ms mean**.
- Bloom: approximately **0.289 ms mean**.
- Tonemapping, clustering, and upscaling were each below approximately
  **0.12 ms mean**.

An older valid settled capture put the main GPU work near **3.5–5.1 ms** and
the CPU render system near **5.3 ms**. These numbers make GPU inspection useful,
but the first fix must address the CPU propagation/scheduling fan-out and its
interaction with render and async terrain work. Do not lower quality merely to
hide the CPU problem.

## Root-cause hypothesis, ranked by evidence

### 1. BigSpace propagation creates a full worker/channel scope every frame

The locked BigSpace dependency is the Bevy 0.19 branch at revision
`5f255228e9b4…`, checked out locally under Cargo's git checkout. Its
`Grid::propagate_high_precision_channeled` implementation obtains the Bevy
`ComputeTaskPool`, creates a channel and task scope every frame, starts one
high-precision worker per compute thread, and dispatches grid work through that
scope.

On the reference host, the default Bevy policy exposes 24 compute workers on a
32-logical-thread machine. The result is a settled world that repeatedly pays
worker creation, channel coordination, producer/consumer scheduling, and
cache/CPU contention even when the hierarchy has not materially changed. Tracy
directly attributes the largest self-time to `hp_propagation_worker` and
`hp_propagation_producer`, making this the first optimization target.

The earlier local experiment in the main checkout capped the default compute
policy at 8 workers while preserving the explicit deterministic thread count
used by scene tests. It built successfully and produced a valid Tracy capture,
but it was **not accepted as an improvement**: no clean post-change comparison
was completed. The experiment is therefore not part of this `usd` handover
branch.

The durable fix belongs at the BigSpace propagation owner. The application must
not clone the dependency's plugin into a second local implementation or add a
main-only special case that changes physics/render semantics. Preferred design
work, in order:

1. Add or adopt an upstream propagation path that checks the authoritative
   floating-origin and `GridDirtyTick` inputs, plus the actual high-precision
   children, before spawning a worker scope. A clean hierarchy must return
   without creating consumers, channels, or producer tasks.
2. Replace per-frame fan-out with a persistent, bounded worker mechanism whose
   scheduling contract is explicit and measurable.
3. Preserve the existing dirty-tick, nested-grid, floating-origin, and
   stationary-hierarchy semantics while making the no-change path cheap.
4. Keep the same propagation result in GUI, offscreen, and headless execution.

#### Current investigation result

The Apollo scene authors `lunco:cameraMode = "freeflight"`. Its camera owner
was rebuilding and assigning the same rotation every interaction tick; the
spring-arm and surface camera writers had the same equal-write behavior, and
the spring-arm also assigned unchanged cell/local translation. Those writes
were correctly observed by BigSpace as dirty non-stationary inputs, so the
dependency's otherwise-unconditional worker scope was not an idle no-op in the
real scene. The camera owner now compares values before assignment in the USD
checkout. This removes false dirty propagation while preserving every actual
camera movement.

The dependency-side experiment is not part of the application contract. It was
never selected by Cargo and did not address the Apollo symptom: the real cause
was mixed ownership in the application, where celestial placement also queried
and migrated the avatar camera. The canonical fix is to remove that path
entirely. Site placement now mounts only the authored site root and physical
descendants; the camera subsystem alone owns explicit camera frame changes
through the shared atomic migration operation. No local BigSpace fork or
application shim is retained. Future generic propagation optimization belongs
upstream in BigSpace and must be adopted only after an upstream revision is
available and measured against the same production scene.

#### 2026-08-28 verification after source-owner fixes

- `rustup run nightly-2026-02-27 cargo test -p lunco-avatar --lib -j 4` passed
  **27/27**.
- `rustup run nightly-2026-02-27 cargo build -p lunco-luncosim --bin luncosim -j 4`
  passed and produced the production `target/debug/luncosim` binary. The binary
  reported the real NVIDIA RTX 5060 Laptop GPU and the explicit High preset.
- A fresh windowed Apollo High run on API port `4133` reached
  `/api/ready` with `ready=true`, `faulted=false`, and `pending_count=0`;
  `/api/diagnostics` reported zero broken connections and zero runtime faults.
  `ListEntities` returned 319 API entities; the runtime diagnostics reported
  4,016 ECS entities.
- `CaptureScreenshot` and `CaptureFromCamera` each produced a real 2561x1553
  RGBA PNG containing the terrain, rover, route ribbon, labels, and HUD. The
  generated evidence files were moved to the desktop trash after inspection.
- The Apollo runtime emitted live diagnostics for more than two minutes. A
  separate user-owned `main` luncosim was also running an empty shell on port
  `37431`, so this is an observed, contended measurement rather than a clean
  acceptance run. Sampled settled values ranged from approximately **52–137
  FPS** and **7.3–19.2 ms** frame time, with physics generally around
  **0.8–1.3 ms**; it did not meet the 200 FPS / 5 ms target. Repeat the clean
  window after the other session is closed.
- Typed API `Exit` returned `accepted=true`; the close flow reported no dirty
  documents, and both the owned process and port `4133` were verified gone.
- The discarded local BigSpace propagation experiment was removed. The app
  remains pinned to the reviewed upstream Bevy 0.19 revision
  `5f255228e9b4…`; no unintegrated dependency code is used or retained.

#### 2026-08-29 app-side camera contract verification

The app-side camera repair is now complete at the actual pose owners. Camera
mode systems, interaction interpolation, mounted USD cameras, cinematic paths,
and the persistent `OriginAnchor` all use Bevy value-gated writes for the
BigSpace-facing `(CellCoord, Transform)` pair. The obsolete blend marker was
removed because it had no producer/advancer and was a second, dead transition
owner. The unreachable surface branch was removed from `freeflight_system`;
`SurfaceCamera` is now the sole surface-rotation owner.

The implementation reuses the existing coordinate contracts rather than adding
camera-specific state: `Grid::translation_to_grid` performs the final cell/local
split, `Grid::grid_position_double` reads the authoritative f64 pose,
`lunco_core::coords::grid_relative_pose` resolves hierarchy-relative poses, and
`lunco_core::attach::migrate_to_grid` performs atomic parent/cell/transform
migrations. BigSpace remains responsible for propagation and derived
`GlobalTransform`; the application owns camera mode and pose policy.

Focused verification used the repository's plain Cargo commands:

- `cargo test -p lunco-avatar --lib -j 4`: **27/27**.
- `cargo test -p lunco-time --lib -j 4`: **46/46**.
- `cargo test -p lunco-usd-bevy --lib -j 4`: **236/236**, one ignored.
- `cargo fmt -p lunco-avatar -p lunco-time -p lunco-usd-bevy
  -p lunco-scene-commands -- --check`: passed.
- `cargo build -p lunco-luncosim --bin luncosim -j 4`: passed in **2m18s**.
- `git diff --check` and a repository-wide search for the retired blend marker
  excluding `target/`: passed; no legacy marker references remain.
- `cargo test -p lunco-scene-commands --lib -j 1` compiled the crate but could
  not link its test harness because the host clang/mold linker reproducibly
  segfaulted. The same crate compiled successfully in the production build;
  this is an environment linker failure, not a Rust test failure.

The complete authored gate was also run against that rebuilt production binary
with `./scripts/run_scene_tests.sh --no-build`: discovery found 58 headless and
2 graphics scenes. The camera regression scene
`free_flight_speed_boost` passed, and 55/58 headless scenes passed overall. The
remaining headless failures were the existing `descent_lander_runtime`
420-second liveness timeout, `drivetrain_parity` tolerance failure, and
`landing_legs` settling failure. The two graphics scenes remained red for
environment/asset assertions: HDRI cubemap projection could not complete
because the cached HDR was absent, and the shader-fallback pixel assertion did
not observe the expected red cube. These failures are recorded rather than
being treated as camera evidence; they keep the repository-wide scene gate
red.

Fresh production runtime verification used the newly built binary and the real
Apollo Traverse scene on API port `4133`:

- `/api/ready`: `ready=true`, `world_hold=false`, `faulted=false`,
  `pending_count=0`.
- `/api/diagnostics`: zero broken connections, pending work, algebraic loops,
  and runtime faults; `ListEntities` returned 319 API entities and runtime
  diagnostics reported 4,016 ECS entities.
- A real 2561x1553 screenshot showed the surface rover view. Typed
  `FocusTarget` on Earth returned `accepted=true` and the inspected screenshot
  showed the Earth orbit view; typed `ReturnFromOrbit` returned `accepted=true`
  and the inspected screenshot restored the surface rover view.
- Typed API `Exit` returned `accepted=true`; the close flow completed and port
  `4133` was verified released.

This runtime was a behavior check, not 200-FPS acceptance: observed on-screen
values varied with active machine load and remained below the 200 FPS / 5 ms
target. The remaining performance work must be measured at the actual render
and application owners; there is no local dependency fork to carry forward.

If the upstream API cannot express this cleanly, stop and document the required
dependency change rather than maintaining a forked/shimmed path in the app.

#### 2026-08-29 stable projection and LOD verification

The USD branch now gates the remaining stable projection owners at their
authoritative inputs. The Modelica document registry revision wakes engine sync
while per-document generations decide actual parse/upsert work. Modelica and
physics telemetry pace batch construction by authoritative solver/fixed time;
the shared signal registry still owns channel history. Behavior target paths are
cached per vessel, while target active-frame ancestry is checked separately.
Celestial curvature tracks its input components, and globe LOD caches the pure
desired leaf set separately from material readiness and bounded tile streaming.
No duplicate propagation path or compatibility API was added.

The focused checks passed:

- `cargo test -p lunco-autopilot --lib -j 4`: **30/30**.
- `cargo test -p lunco-modelica --lib -j 4`: **282 passed, 1 ignored**.
- `cargo test -p lunco-usd-sim --lib -j 4`: **124/124**.
- `cargo test -p lunco-celestial --lib -j 4`: **119/119**.
- `cargo check -p lunco-usd-sim -j 4`: passed.
- `cargo build -p lunco-luncosim --features tracy -j 4`: passed.

The fresh Apollo High Tracy capture (`target/apollo-curvature-lod-cache-short-
20260829.tracy`) ran for 8 seconds after readiness and was shut down through
typed `Exit`; API port `4192` was verified closed. Post-warmup CPU samples were:

| Owner | p50 | p90 | p99 | max |
|---|---:|---:|---:|---:|
| `globe_lod::update_globe_lod` | 0.004 ms | 0.039 ms | 0.071 ms | 0.121 ms |
| `placement::sync_terrain_body_curvature` | 0.003 ms | 0.018 ms | 0.035 ms | 0.069 ms |
| `drive_engine_sync` | 0.001 ms | 0.007 ms | 0.013 ms | 0.085 ms |
| `resolve_behavior_targets` | 0.003 ms | 0.045 ms | 0.078 ms | 0.117 ms |

The same trace shows the remaining budget in BigSpace validation,
`prepare_preprocess_bind_groups`, workbench rendering, exposure publication,
and the GPU/render path. This is profiler evidence, not clean FPS acceptance;
the 200-FPS criterion remains open.

#### 2026-08-29 merge and release verification

The synchronized application branches now point at `4a0c736a8`, including the
terrain stream-readiness accounting fix. `main` and `usd` are both clean and
contain the same three commits beyond `origin/main` (`042f02467`,
`375f97125`, and `4a0c736a8`). The latest focused regression gate passed:

- `cargo test -p lunco-terrain-surface --lib -j 4`: **108/108**.
- `cargo build --release -p lunco-luncosim --bin luncosim -j 4`: passed and
  produced the production `target/release/luncosim` binary.
- A real Apollo High release run on API port `4195` reached readiness with
  zero faults, broken connections, and pending work. Typed `Exit` was
  accepted, and both the process and port were verified gone.

The release run is healthy but does not meet the target: its captured settled
tail varied from approximately **14.6–39.3 ms** per frame (roughly **25–70
FPS** in the sampled tail). It is therefore runtime evidence that the
production path works, not 200-FPS acceptance.

The isolated BigSpace propagation experiment was not adopted and has been
deleted. Cargo still resolves the app to the reviewed upstream Bevy 0.19
revision `5f255228e9b4…`; the application contains no fork, alias, or alternate
propagation path. The measured Apollo issue is fixed at its actual owner in the
application: site placement no longer has an avatar/camera path at all, so it
cannot reclaim a camera-owned BigSpace grid. The authored avatar camera remains
under the site root and inherits its mounted frame; explicit orbital/surface
transitions remain in the camera subsystem.

Subsequent local A/B runs were not acceptance data because a separate
user-owned luncosim session on API port `4139` was active; that process and its
build were preserved. Do not use those runs to claim a performance gain.

#### 2026-08-30 bridge read change-gating verification

The next measured owner was `lunco-usd-avian::pose_to_position`. Its previous
steady-state path scanned every synced body twice per physics read even when no
pose or hierarchy input changed. The bridge now uses Bevy's change detection as
the wake signal and keeps the exact `BridgeShadow::is_representation_only`
check as the semantic authority. First reads, active-frame handoffs, and real
plain-ancestor motion still process the complete body set so descendants and
frame transport retain their existing semantics. Bodies without the bridge's
`PhysicsPoseSeeded` marker remain eligible until their initial pose is actually
written; this preserves late/pre-existing materialization without a recovery
scan. The unused `BridgeShadow::matches` helper was removed.

Focused verification passed:

- `cargo test -p lunco-usd-avian --lib -j 4 -- --nocapture`: **67/67**.
- `cargo test -p lunco-usd-avian --test bridge_physics -j 4 -- --nocapture`:
  **15/15**, including late-body seeding, frame handoff, teleport, ancestor
  re-split, and rotating-parent invariants.
- `./scripts/run_scene_tests.sh --no-build --exact allocation_spec`: passed.
- `./scripts/run_scene_tests.sh --no-build --exact ackermann_parity`: passed.
- `cargo build -p lunco-luncosim --bin luncosim -j 4`: passed and produced
  `usd/target/debug/luncosim`.

A clean windowed Apollo High run on API port `4218` reached
`ready=true` in approximately 3 seconds, with no runtime faults, broken
connections, pending work, or algebraic loops. After settling, sampled clean
diagnostics ranged from approximately **167–275 FPS** and **3.7–6.2 ms** frame
time, with Avian around **0.5 ms**. This is a valid improvement measurement,
but not 400-FPS acceptance. Typed `Exit` was accepted and the process and port
were verified gone.

The existing authored `drivetrain_parity` Rhai gate still fails its heading
comparison at approximately **17%** versus the **15%** tolerance. A deterministic
baseline run against the pre-optimization bridge failed the same assertion, so
the bridge change is not the cause and the gate remains an independent follow-up
task; its tolerance was not weakened.

#### 2026-08-30 physics telemetry lifecycle cleanup

The next measured app-owned leaf was `physics_telemetry::retain_physics_telemetry`
(approximately **0.17 ms** in the settled Tracy attribution). Its sampling
cadence and shared `SignalRegistry` ownership were already correct, but its
cleanup path rebuilt a live-entity `HashSet` and retained four state maps on
every fixed step. The producer now uses Bevy `RemovedComponents` plus a live
source query to retire only entities whose last rigid-body or wheel source has
gone. Archived signal history remains in `SignalRegistry`, and a source
transition does not discard state while another source is still live.

Verification passed:

- `cargo test -p lunco-usd-sim --lib -j 4 -- --nocapture`: **125/125**.
- `cargo build -p lunco-luncosim --bin luncosim -j 4`: passed in `usd/target`.
- A clean Apollo High run on API port `4218` reached `ready=true` with no
  faults, and typed `Exit` released the process and port.
- The 20-second Tracy capture completed with **622 frames**, **2,154,791
  zones**, and **108.09 MB**. Its profiler-contended readiness/FPS is not
  acceptance data; clean FPS remains the required product measurement.

The cleanup is a lifecycle-cost reduction, not a new telemetry store or a
change to sampling policy. The next performance decision should use a fresh
function-level attribution if the remaining frame budget still requires it.

#### 2026-08-30 runtime UI extraction acknowledgement cleanup

The render-world acknowledgement previously rebuilt the visible exposure
namespace set on every extraction, scanned required roots separately from
their readiness state, and used a linear root-membership check while walking
extracted UI ancestry. It now reuses the existing `EngineExposures.revision`
boundary for the derived visibility set, combines the root/readiness query,
and uses Bevy's `EntityHashSet` for root membership. Live roots and extracted
nodes remain queried each frame, so topology and presentation readiness are
not cached or duplicated.

Focused verification passed:

- `cargo test -p lunco-luncosim --lib ui::runtime_exposure -j 4 --
  --nocapture`: **18/18**.
- `cargo build -p lunco-luncosim --bin luncosim -j 4`: passed and produced the
  production `usd/target/debug/luncosim` binary.
- `rustfmt --check`, `git diff --check`: passed.

Two clean candidate launches on API ports `4218` and `4220` had no runtime
faults and shut down through typed `Exit`, but neither reached a valid settled
`/api/ready` gate: the same ten authored bodies remained under
`ShouldBeDynamic`, reproducing the independent USD physics-admission issue.
Those launches are therefore not FPS acceptance data. No performance gain is
claimed until a settled Apollo run can be repeated; the UI change only removes
known per-extraction work at its owning render acknowledgement boundary.

#### 2026-08-30 exposure publisher cadence gating

Tracy attributed approximately **0.93 ms** of mean work to
`engine_exposure::publish_exposure`. The publisher already emitted snapshots at
20 Hz, but Bevy entered the system every render frame to tick a private timer
and construct its large query parameter set before returning. The cadence is
now an existing Bevy scheduler condition shared by the change detector and
publisher. The first publication remains immediate, and `ExposureRefresh` still
captures changes between cadence ticks; stable frames skip the publisher
entirely.

The condition has a focused regression test for immediate startup and the
20-Hz interval. The production binary was rebuilt in `usd/target` and Apollo
was exercised on API port 4221. The run produced no fault and reached the
`scene participants ready` lifecycle log after the existing admission warning;
its final `/api/ready` response remained false because the same ten authored
bodies stayed under `ShouldBeDynamic`. A non-acceptance tail after admission
reported 26 samples at 94.2--278.9 FPS (mean 198.8 FPS) and 3.909--10.617 ms
per frame (mean 5.551 ms). It was exited through the typed API command, and
the process and port were confirmed gone. These samples are diagnostic only;
the Apollo readiness gate remains blocked by the independent ten-body
USD-physics-admission issue recorded above.

#### 2026-08-30 generated-domain projection trigger gating

`domain_projection::project_domain_islands` was approximately **1.397 ms** in
the mixed Tracy attribution. Its own code already knew the exact lifecycle
triggers and returned immediately on stable frames, but the production
schedule still entered the system and constructed its large query set every
frame. The existing trigger contract is now also the Bevy `run_if` condition:
new prim/identity, USD wiring dirty, or member-class resolution. The internal
guard remains the same semantic protection for direct system invocation; the
production schedule now skips the idle system entirely.

The focused `lunco-usd-sim` domain-projection tests and production build are
the verification gate for this change. A settled Apollo rerun is still needed
for an FPS comparison after the independent readiness issue is fixed; no FPS
gain is claimed from the blocked run.

#### 2026-08-30 generated-domain projection work-set gating

The first trigger gate exposed a second cost: while runtime instance identities
were being minted, one `Added<GlobalEntityId>` caused the projector to walk all
USD prims and re-run ownership resolution for each one. The projector now
keeps the existing full pass only for `WiringDirty` and member-source
resolution, and processes only prims with an added USD path or identity for
identity-driven work. The shared `is_domain_network_root` predicate remains
the first composed-stage check before synthesizer selection.

Before the work-set change, the clean 20-second Tracy capture recorded 179
frames and `project_domain_islands` at 8.081 s total across 175 calls (46.18 ms
mean). After it, the same production Apollo High run recorded 362 frames in
20.29 s, reached `/api/ready`, and shut down through typed `Exit`; the dense
1.45-million-zone trace was saved as
`scripts/perf/captures/apollo-high-domain-targeted-20260830.tracy`. Its CSV
decoder was not used for a post-change per-zone percentile because it exceeded
the practical decode window, so no unobserved post-change zone timing is
claimed. The clean runtime gate and focused `lunco-usd-sim` suite passed.

#### 2026-08-30 bounded parallel authored scene runner

`scripts/run_scene_tests.sh` now owns one bounded worker scheduler for both
headless gate and diagnostic stress runs. `-j/--jobs N` defaults to four and
limits independent production `luncosim` processes; each gate process still
uses `--threads 1 --jitter 0`, and the graphics pass remains serial because it
is a shared offscreen/GPU acceptance path. Results are written atomically and
reported in discovery order, while worker failures before result publication
are reported as launcher errors rather than hanging the gate.

Verification:

- The first `./scripts/run_scene_tests.sh --no-build -j 4 rocker_bogie` run:
  **7/7** headless authored scenes passed.
- A repeat of the same command exposed a real load-sensitive failure:
  `rocker_bogie_high_speed` reported `speed_ripple=101.82%` under four
  concurrent production processes, while
  `./scripts/run_scene_tests.sh --no-build -j 1 --exact rocker_bogie_high_speed`
  passed. This is tracked by rover-jitter card 71; no retry, serialization
  exception, or tolerance relaxation was added.
- `./scripts/run_scene_tests.sh --no-build`: default **4-process** gate
  completed all 58 headless scenes with **55/58** passed. The three existing
  authored failures were `drivetrain_parity`, `landing_legs`, and
  `prismatic_spring`; both graphics fixtures remained red for the previously
  recorded missing HDRI and fallback-pixel conditions.
- No missing-result, scheduler, or process-cleanup error occurred.

#### 2026-08-30 merged-main verification

After merging `main` into the USD optimization branch, the USD checkout was
rebuilt in its own `target/` and the production binary reran the focused and
authored gates. The results below are the post-merge baseline; unchanged tests
were not repeated after this run.

- `cargo test -p lunco-usd-sim -j 4`: all 127 unit tests, 6 reader tests, 6
  drivetrain-parity tests, 15 hook-synthesizer tests, and 20 connection-
  derivation tests passed.
- `cargo test -p lunco-celestial --test terrain_curvature_determinism -j 1`:
  **5/5** passed. The public placement contract is exercised directly by the
  integration test.
- `cargo build -p lunco-luncosim --bin luncosim -j 4`: passed in the USD
  checkout's `target/`.
- `./scripts/run_scene_tests.sh --no-build -j 4 rocker_bogie`: **7/7** passed.
- The full authored gate completed all 58 headless scenes with **53/58**
  passed. The five real failures were `ackermann_parity` and `parts_attached`
  (physics bodies escaped), `drivetrain_parity` (16.96% yaw-raycast mismatch
  against a 15% limit), `landing_legs` (pad tilt, residual hull motion, and
  excessive strut travel), and `prismatic_spring` (missing active scene root).
  No bounds clamp, retry, tolerance relaxation, or recovery scan was added.
- Graphics remained **0/2** because the HDRI asset is absent and the existing
  shader-fallback fixture does not produce its expected red pixel. The
  parallel scheduler itself published every result and cleaned up every
  production process.

#### 2026-08-30 BigSpace stationary streamed visual tiles

The BigSpace dependency already provides `Stationary`, which skips cell
recentring and spatial-hash updates for an immutable high-precision entity
while retaining initial and floating-origin propagation. Streamed globe and
terrain visual tiles now use that existing component at their spawn boundary:
their LOD lifecycle replaces entities, and no resident-tile path mutates its
`CellCoord`, `Transform`, or `ChildOf`. Physics collider tiles were deliberately
left on the normal path because live re-bakes update their pose components.

This is an owner-level use of BigSpace's maintained optimization, not an app
propagation fork or a second cache. Focused crate tests, the production build,
and a clean Apollo runtime are required before claiming a frame-time change.

#### 2026-08-30 connectivity projection scheduler gating

The generic link kernel already owns a sim-time cadence, but its regular Bevy
system was still entered on every render frame. Its cadence cursor is now a
scene-scoped resource shared by a scheduler condition and the direct-system
guard, with `Option<f64>` rather than an epoch sentinel. Scene teardown clears
the cursor and AOS/LOS edge history. The separate Wi-Fi projection now wakes
only when a `WifiNode` or source `LinkGeometryState` changes. Link geometry,
debounce, policy hooks, and published state are unchanged.

The next verification gate is the focused celestial suite plus a production
Apollo run; no FPS gain is claimed until a settled comparison is recorded.

#### 2026-08-30 Bevy clustered-light topology gating

The settled Tracy profile still attributed measurable render work to Bevy's
cluster preparation. Bevy's maintained `LightPlugin` requires `Clusters` for
every `Camera3d` and uses the default 4,096-cell `FixedZ` configuration when a
camera has no explicit `ClusterConfig`. The assignment owner only clusters
point lights, spot lights, light probes, and clustered decals; Apollo's authored
sun is directional. The render camera binder now uses Bevy's existing
`ClusterConfig::Single` when the ECS topology contains no clusterable object,
tracks those four component lifecycles with observers, and the camera
reconciler waits for positive `Clusters` dimensions before activation. A live
USD light projection therefore restores the normal Bevy configuration
automatically without exposing an invalid zero-sized GPU view.
Explicit camera `ClusterConfig` components remain untouched.

The same production load also exercises the empty scene-root path contract:
the USD boundary resolves the stage's composed `defaultPrim` before celestial
projection reads it. This keeps a same-frame deferred visual projection from
marking the root before its site anchor can be discovered.

The final focused `lunco-render-bevy` suite passed **52/52**, and the plain
production build passed in `usd/target/debug`. A clean Apollo smoke run on API
port `4228` reached `ready=true`, `world_hold=false`, `faulted=false`, and
`pending_count=0`; `/api/diagnostics` reported zero broken connections,
pending work, algebraic loops, and runtime faults. Typed `Exit` was accepted
and the process and port were verified gone. A post-change Tracy A/B could not
be collected because another user-owned `main` session held the sole Tracy
connection on port 8086. No FPS gain is claimed until a clean before/after
settled run is available.

### 2. Render CPU has several approximately 1 ms leaves and schedule stalls

The render, Update, and PostUpdate schedule totals are not individual fixes.
Use Tracy's longest settled child zones to identify which system is doing work
and whether it is continuous by design. The current evidence points to render
preparation (`prepare_preprocess_bind_groups`, `prepare_clusters`), LOD
maintenance, exposure publication, and domain projection as the next group.

The correct shape is change-driven work:

- LOD decisions run when the camera/viewport/terrain revision changes, with a
  bounded update budget and cached prepared results.
- Stable material/shader readiness is checked through handles, events, or
  revisions, not by rescanning all terrain tiles every frame.
- Cluster and bind-group preparation must not repeatedly allocate or upload
  unchanged data.
- Exposure, telemetry, domain projection, and workbench presentation must
  publish on producer revision changes; a stable frame only performs cheap
  change detection.
- Async terrain bakes and GPU uploads must not block the UI/render schedule.

The approximately 39 ms `prepare_clusters` outlier requires a separate Tracy
inspection. It may be an upload/allocation stall rather than average work, and
the stable-frame target requires p95/p99 evidence, not only means.

### 3. Physics remains a separate, lower-priority budget

Physics must stay independent from visual quality and visual transforms:

```text
USD-authored terrain / physics facts
    -> terrain oracle + authored collider settings
    -> Avian collider and solver

USD-authored appearance / camera intent
    -> render quality + visual LOD
    -> terrain meshes, materials, and GPU work
```

The terrain substrate already documents the intended boundary: visual and
physics products sample one height oracle but choose independent sampling
contracts. `colliderDepth` and `colliderResolution` are authored physics
inputs; renderer quality is not a physics input. Do not reintroduce coupling by
using High LOD for colliders or by reading `GlobalTransform` as physics truth.

After the propagation fix, measure Avian and its bridge again. If the normal
physics cycle is still above the desired **1–1.5 ms** envelope, profile the
authoritative solver configuration and the physics telemetry/bridge separately.
Do not remove diagnostics to make the number look smaller: the existing
measurements show telemetry is secondary, and diagnostics are not the solver
root cause.

## Regression context

The quality regression investigation identified this sequence:

- `272de659e` introduced configurable terrain/rendering quality.
- `90e52dcd6` coupled terrain collider quality to graphics settings.
- `31ec957e5` added process-level render-quality profiles and the High CLI
  override.
- `396800f08` restored the architecture by authoring explicit
  `colliderDepth`/`colliderResolution` settings and removing the visual-quality
  input from the physics contract.
- `f87dfc020` reduced repeated terrain material-readiness work and propagated
  terrain extents to shader uniforms; it also contains the current quality and
  presentation changes used by the baseline.

The quality commits can increase visual terrain resolution, shadow budgets,
terrain tile budgets, and asynchronous bake pressure. That explains why High
can expose render/streaming cost, but it does not explain the complete settled
CPU profile by itself: the largest attribution is BigSpace propagation fan-out,
which runs for the transform hierarchy and is independent of terrain visual
quality. Keep the two investigations separate and verify each A/B change against
the same scene and settled window.

## Required architectural constraints

Any implementation proposed from this handover must satisfy all of these:

- Physics results and cadence are identical in headless, offscreen, and
  windowed execution for the same authored scene and inputs.
- Physics never reads renderer quality, visual LOD, GPU resources, or rendered
  `GlobalTransform` as its source of truth.
- Render quality never changes collider topology, collider resolution, solver
  settings, or the physics step count.
- BigSpace remains the sole owner of high-precision hierarchy propagation; the
  application does not add a duplicate plugin, alternate propagation path,
  compatibility shim, or silent fallback.
- Per-frame work is limited to genuinely continuous rendering, physics,
  animation, and input. Stable derived state uses observers, change detection,
  generation cursors, or content-addressed caches.
- Tunable terrain/collider values live in the owning settings/resource or
  authored USD schema. Do not add magic constants in a hot-path system.
- No transient simulation pose is written back to authored USD every frame.
- Do not optimize by disabling visual features, lowering High to Balanced/Low,
  reducing physics fidelity, or hiding stalls behind a profiler-disabled build.
- Follow the targeted-check rule: format only touched Rust files and run the
  narrowest affected package/test. Never use `cargo fmt --all` for a focused
  change and do not create custom temporary directories.

## Execution plan for the next owner

### Phase A — establish one valid comparison

1. Start from the current branch and record the exact commit, GPU adapter,
   window dimensions, quality preset, API port, and scene path.
2. Run one clean production session without Tracy. Ignore startup until the
   entity count and terrain status are stable; record a fixed 20-second window
   of FPS, frame-time p50/p95/p99/max, and Avian cycle timings.
3. Run one Tracy capture of the same settled window. Inspect the longest child
   zones first. Do not run broad test suites or repeat equivalent captures.
4. If the compute-thread cap experiment is still present, make one clean A/B
   run with the same scene and settings. Accept it only if it reduces settled
   frame-time tails without changing simulation behavior; otherwise revert the
   experiment and continue at the owning dependency.

### Phase B — fix BigSpace fan-out at its owner

1. Reproduce the no-change propagation cost with a minimal hierarchy and the
   Apollo scene, keeping the same BigSpace plugin ordering.
2. Confirm whether clean-subtree frames still create the worker scope. Use
   Tracy zone counts and dependency source inspection as evidence.
3. Implement the bounded/persistent or no-dirty-work path in the maintained
   BigSpace dependency, preserving stationary and nested-grid semantics.
4. Add the smallest generic regression coverage the dependency can observe;
   behavior must be checked through production luncosim for both rendered and
   headless paths. Do not add a Rust test that merely duplicates an observable
   scene assertion.
5. Rebuild the production binary, repeat the one clean run and one Tracy run,
   and compare p95/p99 plus the physics result—not just average FPS.

### Phase C — remove remaining CPU frame-tail work

1. Inspect the child zones under Render, Update, and PostUpdate in the same
   settled capture.
2. For each top leaf, identify its owner and gate it on the producer's revision
   or event. Extend the existing cache/revision mechanism instead of adding a
   second cache or a second traversal.
3. Investigate `prepare_clusters` outliers and any terrain upload/allocation
   that crosses the render schedule.
4. Re-run only after inputs/code changed. Keep the captured before/after line
   in the handoff or PR; do not commit multi-megabyte captures.

### Phase D — prove physics and visual independence

1. Run the same authored scene headless and windowed with identical inputs.
2. Compare physics cycle timings and authoritative poses/telemetry, not visual
   transforms.
3. Change only visual quality and verify collider settings, physics cadence,
   and physics results remain unchanged.
4. Inspect the Apollo terrain at close range to ensure the brightness/moon
   stability fixes remain intact while optimizing the frame loop.

## Acceptance criteria

The issue is closed only when all of the following are recorded from a real
production session:

- Windowed Apollo scene at High quality, after startup settles, sustains at
  least **200 FPS for 20 seconds** with no manual camera pause or hidden
  throttling.
- Clean-run frame time is **p95 <= 5.0 ms**, with p99 and maximum reported
  separately. A single large startup bake is not part of the settled window;
  any settled outlier must be explained and fixed rather than discarded.
- Tracy confirms that the BigSpace propagation path no longer pays avoidable
  per-frame worker/channel fan-out and that the longest remaining paths are
  understood. If profiler overhead prevents the same absolute number, report
  the measured profiler overhead and prove the code path remains bounded; do
  not call a capture-time number a clean acceptance result.
- Normal physics cycle is within the agreed **1–1.5 ms** target for this scene,
  with no visual-quality-dependent change in physics cost or results.
- Headless and windowed runs use the same physics owner and produce equivalent
  authoritative state for the same input sequence.
- No terrain brightness oscillation, close-moon frame-to-frame mutation, or UI
  responsiveness regression is present.
- `git diff --check`, targeted formatting, the narrowest relevant checks, and a
  real production runtime/API smoke all pass. The handoff states exact commands
  and whether the process and port were cleanly shut down.

## Do not start from these dead ends

- Do not run a full workspace format or a full suite before profiling the
  longest path.
- Do not optimize from the average FPS shown during Tracy capture.
- Do not lower `--render-quality` to claim the High-quality target.
- Do not make collider resolution follow renderer LOD.
- Do not patch the Cargo git checkout in place as an unreviewed local fork.
- Do not add a cloned BigSpace plugin, compatibility alias, fallback path, or
  app-specific propagation implementation.
- Do not rerun equivalent captures after an unchanged build/input; reuse valid
  evidence and spend the next run on the highest-ranked unresolved path.

## Related canonical docs

- [`scripts/perf/README.md`](../../scripts/perf/README.md) — profiling tools and
  measure-first rules.
- [`../architecture/42-ui-frame-discipline.md`](../architecture/42-ui-frame-discipline.md)
  — change detection and frame-budget discipline.
- [`../architecture/45-big-space-correct-usage.md`](../architecture/45-big-space-correct-usage.md)
  — BigSpace ownership and pose boundaries.
- [`../architecture/46-bigspace-deep-analysis.md`](../architecture/46-bigspace-deep-analysis.md)
  — BigSpace maintenance checklist.
- [`../architecture/terrain-substrate.md`](../architecture/terrain-substrate.md)
  — one terrain oracle with independent visual and physics products.
- [`../architecture/terrain-layered-rendering.md`](../architecture/terrain-layered-rendering.md)
  — terrain rendering pipeline and bounded derived work.
- [`../architecture/render-decoupling.md`](../architecture/render-decoupling.md)
  — renderer ownership boundary.
