# High-quality 400 FPS performance handover

> Status: Open · Worktree: optimization · Scene: Summer Space School Apollo

## Objective

Measure and reach 400 FPS in the production Apollo scene with High visual
quality enabled. The result must preserve authored visuals, physics cadence,
BigSpace frame semantics, and the same behavior in windowed, offscreen, and
headless hosts.

## Current implementation

`lunco-celestial::globe_lod::update_globe_lod` is now change-driven. Its
existing `GlobeTiles` state records the settled camera identity and position;
the run condition wakes only for unfinished residency, material readiness,
LOD/handoff/grid changes, active-camera hierarchy changes, or camera motion.
The camera reconciler's mutable resource access is not used as a change signal.
The runtime exposure publisher is also change-driven: separate invalidation
domains skip unrelated surfaces, and authored control-root membership is cached
against the existing `UsdStageRevision`. No second propagation implementation,
quality reduction, fallback, duplicate cache, or local BigSpace fork is part of
the implementation.

The shared simulation application admits BigSpace's local-origin and transform
propagation sets from spatial input changes. A changed floating origin receives
one follow-up origin computation to settle BigSpace's per-computation unchanged
flag; that pending settle is tracked between admitted passes, so stable-frame
admission does not scan every grid. Stable frames then leave propagation closed.
BigSpace remains the sole owner of transform propagation.

The shared Modelica engine adapter follows the same boundary. Its document
generation cursor no longer polls the registry and engine queues every `Update`;
document revisions, completion notifications, and tracked edit-debounce
deadlines are the only wake sources. A completion latch is cleared only while
the engine mutex confirms both completion queues are empty, so the bounded
completion budget cannot strand a queued parse or library result.

The asset catalog follows the same UI boundary. The shared discovery owner now
enumerates USD, WGSL, Modelica, and Python extensions in one asynchronous task;
the spawn, shader, and program projections publish only after that task drains.
The startup and Twin lifecycle paths no longer synchronously walk the open Twin
roots from the UI schedule, and listing generations prevent a closed/reopened
Twin from publishing stale results.

This pass also reduces repeated reactive work without changing cadence or
validation: the HUD invalidation sources now share one combined `Or` query; the
Modelica projector checks prim and identity additions together once inside its
system instead of scanning them in both a run condition and the system; and
BigSpace's high-, local-, and low-precision admission predicates each combine
their existing spatial invalidation filters into one query per schedule
boundary. The USD telemetry projector performs one bootstrap pass, then uses
component insert/remove observers plus scalar stage-revision and asset-change
signals instead of repeated `Added`/`Changed` population queries in both its
run condition and invalidation system. BigSpace still owns all propagation
work and the origin-settle pass is unchanged. Physics-validation and telemetry
scratch buffers also retain capacity across calls. Domain member-class
discovery now consumes a typed `UsdSceneChangeBatch` and routes its
resynced/info paths through a canonical stage/root/member reverse index.
Stage-asset changes and missed generation batches requeue only roots on the
affected stage; the all-prim discovery is limited to initial admission.
Endpoint wiring invalidation keeps using the narrower queued-entity path.
Generated Modelica document sync also
uses lifecycle-queued wrappers and a dirty publisher latch rather than per-frame
`Changed` filters. Domain synthesis uses direct authored-property lookup for
member communication periods and enumerates root attributes once for both
actuator input and output ports. USD connection reconciliation also borrows
endpoint paths, generated aliases, and port surfaces in its temporary indexes
instead of cloning them; it builds those indexes in one endpoint sweep instead
of three separate population scans. Earth-direction demand now reconciles its
existing and desired marker sets, avoiding remove/re-add events on unrelated
rewires. Endpoint removals now dirty USD wiring only when the removed
capability belonged to an authored USD prim; unrelated `SimComponent` teardown
no longer triggers a whole wiring pass. These are source-level work reductions
only. Program binding extracts its declared input/output maps once and reuses
them for the admission verdict and published interface; the communication
period is read through a direct authored-property query. Orphan acausal
admission uses a non-allocating prefix probe to skip connectorless programs,
then derives declaration and connection presence from one attribute-name pass
when connectors exist. The authoring-review view model now reacts to
selection/control/camera/mount and identity-lifecycle changes, compares
diagnostic/fault contents to ignore no-op mutable publishes, and passes its
focused scheduling test. Its reduction remains source-level and unmeasured. A
clean settled Apollo run remains necessary before claiming an FPS improvement.
The shader-look binder also reuses loaded WGSL stage verdicts by asset ID and
stage, invalidating them on shader asset events so shared terrain looks do not
revalidate identical source; see the dated note in the [200 FPS
handover](open-200fps-performance-handover.md). No measured FPS delta is
claimed.

## CPU investigation lead

Two owned Apollo startup captures separated domain synthesis from publication.
In `summer-space-school-domain-zones-20260923-4191.tracy`, the ten
`domain_projection_commit` spans accounted for 4.298 s total (430 ms mean,
1.383 s max). The follow-up capture,
`summer-space-school-domain-zones-20260923-4191-after-index.tracy`, recorded
the same ten commits at 32.1 ms total (3.21 ms mean, 16.3 ms max). The measured
commit interval fell by about 99.3% in these startup captures.

The source-level cause was repeated composed-stage work inside signal-layout
publication: each telemetry ownership check enumerated every prim, and each
unit cloned the already-expanded exact-path/provenance maps from earlier units.
The projector now builds one telemetry ownership index per canonical stage
generation or prepared-plan identity and shares it across roots in a projection
batch. Unit expansion reads immutable source-map snapshots, so generated keys
do not recursively feed later units. The new Tracy zones measured two telemetry
index builds at 30.1 ms total, five signal-layout builds at 0.77 ms, and five
unit expansions at 0.60 ms. The 27 domain tests, including external telemetry
ownership and multi-unit mapping regressions, pass.

This is startup/reactive-path evidence, not a steady-frame or FPS result. The
post-change 45.28 s capture had 941 frames and 5.72 million zones, but the
owned API check still reported `ready=false` with USD physics admission pending.
It must not be used as a clean settled acceptance window. The live canonical
USD `Stage` remains `!Send`; if a future profile establishes that synthesis
itself affects a user-visible budget, any worker boundary must pass an owned,
send-safe snapshot and fence publication by root and stage generation.

## Verification

- `cargo clean` from the optimization checkout removed 16.2 GiB after the
  checkout reached 98% disk usage; all evidence below is post-clean.
- `cargo build -j 4 -p lunco-luncosim --bin luncosim --features ui`: passed.
- `scripts/run_rust_tests.sh -j 4 -p lunco-celestial --lib`: 128 passed.
- Production run used `target/debug/luncosim`, X11, High quality,
  `--no-vsync --no-throttle --log-diag`, API 4366, and the authored
  `traverse_apollo15.usda` scene. `/api/ready` returned
  `ready=true`, `world_hold=false`, `faulted=false`, `pending_count=0`.
- Current optimization build: focused `cargo check` and exposure/core tests
  pass. Apollo reached `/api/ready` in 4.1 s from process start; its settled
  diagnostic tail was 92–147 FPS with 7.3–10.9 ms frame samples. The sandbox
  reached readiness in 2.8 s. These runs remain non-acceptance evidence.
- On the post-change production vehicle run (binary rebuilt after syncing
  `d7ad4ad0f`), the shared listing was scheduled at `01:56:35.675886Z`, the
  scene was spawned at `01:56:36.732963Z`, and scene participants were ready at
  `01:56:41.677245Z`. This is approximately 8.88 s from process start and is
  evidence that catalog enumeration no longer blocks the scene schedule, not a
  claim that the 2 s loading target is met.
- The final rebuilt binary also completed a headless vehicle smoke: the shared
  listing was scheduled at `02:43:32.324864Z`, the scene spawned at
  `02:43:32.547703Z`, and participants were ready at `02:43:36.320508Z`.
  The run was terminated by its 10 s verification timeout after readiness; the
  port was free afterward. This confirms the new catalog path on the rebuilt
  binary, but the approximately 5.01 s process-to-readiness interval remains
  above the 2 s target.
- Settled diagnostics reported 4100 entities and approximately 33–79 FPS with
  approximately 12.6–30.3 ms samples in the captured window. The API `Exit`
  command was accepted and port 4366 was verified closed.
- `nice -n 19 cargo test -j 4 -p lunco-luncosim-edit-ui --lib
  authoring_review_view_gate_sleeps_until_an_owner_or_identity_changes` passed
  (1 test). This verifies the scheduling gate, not an FPS gain.
- `nice -n 19 cargo test -j 4 -p lunco-usd-sim-domain --lib` passed all 27
  tests after the telemetry-index and signal-map changes.
- `nice -n 19 cargo build -j 4 -p lunco-luncosim --bin luncosim --features
  tracy` passed. The post-change capture's typed API `Exit` was accepted and
  port 4191 was verified closed; the existing 4163, 45552, and 37431 sessions
  were left untouched.

## Remaining blocker

The 400 FPS acceptance target is not met. The maintained BigSpace dependency
is pinned to the reviewed `bevy-0.19` revision available here
(`5f255228e9b4…`). Application admission skips stable propagation frames, but
an active high-precision pass still uses BigSpace's worker/channel fan-out; the
available Tracy attribution predates the application gate, so its current cost
is not established. The remaining render/update budget also needs a clean,
settled measurement against the 2.5 ms frame budget. Further per-entity
fan-out reduction belongs in the maintained BigSpace owner, not an application
duplicate or degraded path.

## Acceptance

Close only after a real production High-quality Apollo run sustains at least
400 FPS with p95 frame time at or below 2.5 ms, while the required physics,
visual, headless/windowed, and API shutdown checks remain green. Until then,
keep this handover open and record the exact upstream/render evidence here.

See [`open-200fps-performance-handover.md`](open-200fps-performance-handover.md)
for the shared profiling commands and BigSpace architectural constraints.
