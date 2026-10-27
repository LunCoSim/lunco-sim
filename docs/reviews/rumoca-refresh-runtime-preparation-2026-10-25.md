# Rumoca runtime sweep storage qualification

The authorized native workspace change is published as
`eca3948416fa90d8695e2f91557ee9db18dbe75d` on LunCoSim/rumoca branch
`fix/torn-sweep-workspace`. It retains the current compact assignment-producer
index and immutable shared schedule. The regular application build with
Avian `ba8978bd` passes the bounded production qualification below.

## Owner and mechanism

Rumoca's per-runtime `TornSweepCache` retains raw residuals, dense native
outputs and development-build independent reference state/residuals. Every live
entry is initialized for its current call; no numerical answer is cached.
Cloning starts with empty independent storage. Existing checked capacity
helpers report reservation failures; reentry returns a typed runtime error.
Per-row reference replay, full f64 bit comparison, finite checks, causal order
and native decline decisions remain intact.

The public Model Exchange adapter remains the application boundary. No solver
implementation, scratch cache, setting or alternate backend is introduced in
application Rust. This numerical scratch-lifetime invariant has no changeable
policy and needs no Rhai hook. Immutable construction sharing has its own
[owner review](rumoca-shared-assignment-preparation-2026-10-25.md).

## Native qualification

- All 130 focused solver-runtime tests pass, including changed inputs, overflow
  decline/recovery, capacity/pointer reuse, clone isolation, explicit reentry
  failure and signed-zero preservation after a compiled backend decline.
- Solver all-target/all-feature Clippy passes with `--no-deps -D warnings`.
- The fixed twenty-model MSL canary has twelve high-agreement OpenModelica 1.27.1
  traces, no deviations/timeouts, six ToDae refusals and two EL005 solve refusals.
  All fourteen simulation outcomes and refusal messages match the previous
  current-pin log, as do its agreement-band counts. Previous raw traces are no
  longer available; cross-run bit equality and full-cohort parity are not claimed.
- Touched Rust is formatted once with the native pinned nightly formatter;
  complete diff review and `git diff --check` pass. The DCO-signed-off commit
  is pushed and its remote SHA verified.

Native logs and retained result JSON are ignored under
`../rumoca/.cache/codex-evidence/torn-workspace-20261027/`.
The [Avian replay repair](physics-first-step-joint-replay-2026-10-25.md)
fixes the independently established ECS joint-pass ordering cause. Production
acceptance of this workspace change uses that repair with the unchanged exact
reference and tolerances.

## Production qualification

The regular build (`cargo build -p lunco-luncosim -j 4`) completes in 5m23s
with the normal target and sccache. Manifest and lockfile changes are exactly
the Rumoca source revision replacement; the dependency graph is unchanged.
All ten unchanged production replay profiles pass 360 exact checks on owned
API 5520. The coupled initialization/clock/pause gate passes 44 checks, and
the authored Modelica drive law passes eight. The negative initialization gate
reaches its expected terminal `modelica-initialization-failed` diagnostic.

That negative gate exposed a missing dependency declaration in the shared Rhai
fixture: imported modules do not inherit the caller's `assembly_edit` alias.
The helper now imports its authoritative tool library explicitly. Both direct
and imported initialization cases pass, without a core resolver change.
The initial failed fixture run remains recorded; it is not counted as solver
qualification. Production logs/receipts are ignored under
`.cache/codex-evidence/performance-20261027/torn-workspace/`.

## Griffin observation

Regular first-fill PID 920224/API 5521 and warm PID 921385/API 5522 both
preserve all 271 source hashes at model `2b3c0a9`, matching the preceding Avian
qualification. Both reach sleeping, finite four-pad handoff, remain unpaused,
and have zero registered runtime warnings/errors. Screenshots are inspected;
all owned processes and ports close. Logs retain existing telemetry-capacity
and sun-gate diagnostic warnings; clear registered diagnostics do not mean
warning-free logs.

| Measured owner | First fill | Warm repeat |
| --- | ---: | ---: |
| First physics readiness | 24.349 s | 11.534 s |
| Initializer checked IR preparation | 5.569 s (miss) | 0.908 s (disk hit) |
| Initializer runtime construction | 3.931 s | 3.403 s |
| Native kernel attempts / hits / compilations | 16482 / 16482 / 0 | 16482 / 16482 / 0 |
| Native physics median / p95 | 0.438 / 0.663 ms | 0.432 / 0.654 ms |
| Fixed-schedule service median / p95 | 4.750 / 5.739 ms | 4.672 / 5.750 ms |
| MainPropulsion worker mean | 8.459 ms | 8.779 ms |
| Frames median / p95 | 16.918 / 22.786 ms | 15.585 / 19.677 ms |

The prior Avian-only run's 21.302 s readiness also had a prepared-IR miss,
although all its native kernels hit. A new compiler revision owns a distinct
prepared cache identity. Cold/warm prepared-cache observations must not be
attributed to sweep reuse. A foreign offscreen rendering session runs during
both new windows; desktop and other workloads differ from the earlier baseline.
No isolated whole-product timing gain or complete three-Twin acceptance is
claimed. Native tests directly prove stable scratch storage across warm calls.
Loading, graphics and complete-step targets remain open.
The full coupled-step target includes Modelica worker/response work outside
`FixedFirst` through `FixedLast`; that schedule service interval is not the
complete coupled-step duration, and overlapping worker durations cannot be added.

## Latest-main integration

The application rebases onto `936375974`, preserving its Twin input/HUD
admission changes and dependency additions. The locked regular rebuild takes
1m43s. On this combined artifact, owned API 5523 repeats all 360 exact replay
checks, 52 coupled/drive assertions and the expected initialization-failure
verdict. API 5524 passes all 21 Twin presentation/lifecycle checks, including
paused replacement, preference retention, three terminal invalid-profile cases
and recovery. Both test sessions close. The unrelated handoff files match
their saved hashes. `integrated-validation.json` records the executable hash
and each production verdict.

The integrated Griffin run (PID 938560/API 5525) lands with sleeping finite
state, unchanged source hashes and clear registered diagnostics. Its initializer
records 4,748 hits, 11,734 compilations and 11,734 evictions, constructing for
7.187 s; readiness is 22.539 s. This is not a warm-native-cache comparison.
The screenshot has a black scene behind the HUD, so that window does not qualify
visual or graphics acceptance. No camera pose was recorded in that run, and
its cause is not established.

An offscreen default/focused view check (PID 952967/API 5526) renders Griffin
and terrain correctly. A subsequent controlled windowed run (PID 955482/API
5527) records the active camera pose and renders the default scene, terrain,
HUD and a focused lander view correctly on the same integrated executable.
All 271 source hashes match, four-pad handoff is sleeping and unpaused, and
registered diagnostics are clear. Both sessions close; the black-frame
observation remains retained rather than being counted as a rendering pass.

The controlled windowed run observes 7.778 s readiness and 2.132 s initializer
construction with all 16,482 kernels hitting and zero compilations/evictions.
Native physics median/p95 is 0.358/0.554 ms; fixed-schedule service is
3.731/4.364 ms; MainPropulsion worker mean is 6.139 ms. Frames before the
post-measurement focus command have median/p95 9.630/12.853 ms. This later run
has lighter recorded host load and a warm cache; it does not isolate the sweep
optimization's effect or meet all original targets. Camera poses, screenshots
and receipts remain in `griffin-view-qualified/` and `griffin-windowed-camera/`.
