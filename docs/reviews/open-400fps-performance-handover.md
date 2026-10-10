# High-quality realtime performance handover

## Dataset catalog notification discipline — 2026-10-10

Dataset failure-outbox drains in `lunco-assets-datasets` and `lunco-assets`
preserve catalog resource change ticks. They still emit the same typed failure
events in the same order; actual registry mutations retain their notifications.
This removes false per-frame invalidation of the Rhai-owned consent projection.
The UI bridge also tracks interactive-window availability explicitly, so adding
or removing the last window refreshes policy facts without incidental registry
writes. Policy selection, authored rows/actions, async admission, solver and
physics mechanisms, precision and external dependencies remain unchanged.

Two focused native resource/lifecycle tests pass: quiet publication and registry
refresh, window insertion/removal, quiet diagnostic draining and exactly-once
failure delivery. The isolated UI test fails before the window-input fix and
passes afterwards. Native normal/Tracy builds pass; both report the existing
unused `PreparedSolveCache::clear` warning. The skill catalogue passes (43).

Tracy PID 2703340/API 4202 and collector 2703339 pass 30 authored checks:
sandbox smoke (7), telemetry history (17), and consent surface (6). The consent
gate verifies rows/actions, unrelated-scope dismissal, active dismissal and
retired-property clearing. Scene replacement clears readiness/diagnostics;
both processes exit zero and the port closes. Settled publisher p50 falls from
0.272814/0.266708/0.280068 ms to 0.004909/0.005871/0.004048 ms in View/Build/View,
using +1.5 to +7.5 s after each activation. This is owner attribution, not a
causal overall-FPS claim across different hardware clocks/workloads.

Normal PID 2710646/API 4202 passes another 30 authored checks and replacement,
exits zero and closes its port. No concurrent app/Cargo process is recorded at
launch. High quality, equal camera poses, 50 bodies/33 colliders/20 joints and
advancing physics are retained. Sampled frame p50 is 10.841/10.794/10.921 ms;
physics-ring p50 is 1.117/1.063/1.137 ms, and readiness first clears at 2.758 s.
These after-only observations do not establish loading or physics-FPS gains.
Screenshots show the optional consent prompt before dismissal and an unobstructed
populated Builder after replacement. No download is requested. No new exact-state
replay claim is made for this notification/presentation checkpoint.

Evidence and hash-verified archives are under `target/entity-tree-perf/`, prefix
`dataset-`, including `dataset-tracy-comparison-20261010.json`. Normal SHA-256:
`3fdfbf0c94adec0c3f600edf0b233e14400f2af00cdfbce9cc426057d3a5acf0`;
Tracy: `b83b25682bf08a60b1438342f601f915335edba169950f84778e722ea45ccb8c`.

## Native hierarchy insertion delivery — 2026-10-10

The derived entity-tree gate receives immutable `ChildOf` insertions through
Bevy lifecycle messages. It coalesces entity identities and compares current
edges with the cached ancestry instead of scanning every hierarchy component's
change tick each frame. Mutable labels, marker membership, scene boundaries,
removals and identical-edge suppression retain their existing readers. Active
Twin closure clears queued insertions. This is the UI projection's generic
invalidation mechanism; visibility policy, worker revision fences and
authoritative simulation paths remain unchanged. No external library changes.

Eight focused generic lifecycle/resource tests pass, covering active-scene and
preview membership, unchanged edge replacement, reparenting, unnamed candidate
ancestors, removal/despawn, stable duplicate labels and Twin-close retirement.
The normal and Tracy production builds pass without warnings. Changed Tracy
PID 2561815/API 4202 and collector 2561814 pass 24 authored smoke/history checks;
both exit zero and the API port closes. Listener and connection ownership are
verified. Normal PID 2586102/API 4202 passes another 24 authored checks and scene
replacement, exits zero and closes its port. Screenshots show the populated
entity tree before and after replacement. The optional visual-resource prompt
remains visible in the compared screenshots.

The trace slices span +1.5 to +7.5 s after each perspective activation, excluding
startup, authored checks and replacement. Gate p50 is 0.132162/0.131946/0.121607
ms before and 0.098800/0.102672/0.101644 ms after in View/Build/View. No insertion
callback runs within those settled slices. Comparison and observer exports are
in `target/entity-tree-perf/hierarchy-tracy-comparison-20261010.json` and
`after-insert-cpu-20261010.csv`. These are observed owner timings; different
hardware clocks and overlapping main-worktree activity prevent a causal
product-FPS claim. The other session is left untouched.

The normal run preserves High quality, stable camera pose and advancing
50-body/33-collider/20-joint topology. Sampled frame p50 is
11.116/10.888/10.883 ms and physics-ring p50 is 1.069/1.173/1.029 ms;
readiness first clears at 3.003 s. These after-only observations do not establish
loading or FPS improvement. No new exact-state replay claim is made for this
UI-only checkpoint. Normal SHA-256 is
`981ececb06c961cf96c2b4bbd104f4fa821d4aa578ca3a9b477d09862e48ab9d`;
changed Tracy is
`9bc06af1f7142e7409383d2e12aa25c1d0cd7a2b49d5205262ec80aa77e38c28`.

Baseline Tracy PID 2448227 completes all timing windows, 24 authored checks
and replacement, then the helper force-reaps it after a 15-second API Exit
timeout. Collector 2448226 saves the capture and exits zero; this supplies
timing evidence, not successful shutdown acceptance. An earlier helper uses
an unavailable perspective command and supplies no measurement window.
The corrected driver uses `ActivatePerspective` and disconnects its collector
before app shutdown. Hash-verified gzip archives retain both Tracy artifacts
and the tested normal executable under `target/entity-tree-perf/`.

Evidence availability on 2026-10-10: this checkout's `target/` directory was
removed outside this task's commands during the follow-up. Earlier
`target/image-loader-perf/` and `target/perf/` artifacts referenced below are
therefore unavailable locally. Their descriptions record historical results;
new entity-tree evidence is retained under `target/entity-tree-perf/`.

## Participating-closure prepared-solve reuse — 2026-10-10

`lunco-modelica-worker` keys prepared solve IR by the strict compiler's
participating-source content identity, resolved solver and exact override bits.
Source-root admission still invalidates compiled artifacts and preserves all
ordered completion fences. Bounded prepared graphs remain reusable after a
fresh strict compilation yields the same closure. Native storage owns only the
current `prepared-solve-v8` namespace. Compiler admission revision remains
diagnostic context, outside prepared-model identity. This is generic immutable
cache ownership, with no policy hook or external-library change.

The inline-source regression fails before the change and passes afterwards:
admitting an unrelated library changes compiler revision but preserves exact
initial values, parameters and solve identity; editing the participating
library changes both identity and parameter values. All 64 worker tests pass,
including malformed-record rejection, identity validation, bounded retention
and ordered scheduling. The production native build and browser worker compile
check pass. The browser check reports the existing unused
`BackendCompileResult.unit` field; browser runtime acceptance is not claimed.

Two owned High-quality sandbox sessions use isolated cold caches and the same
authored scene, then replace it once. Baseline PID 2266553/API 4200 performs two
redundant replacement lowerings; changed PID 2280283/API 4200 performs none and
all nine replacement preparation observations are memory hits. Distinct disk
records fall from ten to eight. Byte-sliced log comparison verifies matching
model/source identities and the same initial solver and override vectors.
Initial readiness is 11.165/11.471 s and replacement readiness 1.034/0.962 s;
these single observations do not establish a causal loading-time or FPS gain.
Cold rover lowering and preparation queue waits remain open.

The exact changed production artifact
`c64c3acf84ff5b811a6c327260fd383ae2205eb269b29086b00a0339a7960efa`
passes ten production Rhai replay profiles with 360 authored checks, including
deliberate mismatch rejection, against the unchanged reference. Four-, eight-
and twenty-rover serial/default profiles plus two jitter levels and two seeds
use owned API 4201. Every successful session exits zero and closes its port.
Both native artifacts have hash-verified gzip archives. Evidence lives under
`target/image-loader-perf/` with prefix `prepared-solve-key-`; comparison and
full replay receipts are `prepared-solve-key-comparison-20261010.json` and
`prepared-solve-key-current-replay-20261010.json`.

An early readiness probe and a disk-full replay/check attempt have no acceptance
verdict. The successful retry supersedes them. Recovery removes only unused
outputs from this checkout's affected packages and the completed browser-check
profile; source, sibling worktrees, evidence and shared caches remain intact.
Visual-FPS and physics-FPS acceptance remain open.

## Local-main integration validation — 2026-10-09

The optimization tree includes main `43f130184` without dropping its incremental
terrain annotations, typed float-texture uploads or dependent-material refresh.
The integrated normal production build passes. Artifact
`7afd0dd35d4b49ed1659b43e6da6b90c9f00741693bb047a9d66cc250b606edd`
passes 68 rendered history, replacement and mesh-sharing assertions in owned
PID 1846124/API 4208, eight physics-frame rejection/recovery assertions in PID
1852940/API 4206, and 48 exact-state replay assertions in PIDs 1855813/1856086
on APIs 4207/4209. The existing reference remains unchanged, deliberate mismatch
rejection still passes, and all sessions exit zero with closed ports. The full
360-assertion replay matrix below remains the broader determinism evidence.

The focused native test
`surface_annotation_resize_refreshes_only_dependent_materials` passes. Its
unused test import is removed afterwards, followed by one formatter pass on
that file; these non-behavioral edits do not invalidate its result. The 43-skill
catalog validates. Evidence lives under `target/image-loader-perf/` with the
`main-integration-` prefix. The rendered screenshot retains scene geometry,
rover, headlights and shadows. Warm-cache readiness is 2.954 s; the short
View/Builder/View observations are integration evidence, not controlled FPS
or loading-gain acceptance.

The networking-feature production build also passes. Owned PID 1904839/API 4205
captures a settled 273-command schema and exits zero with its port closed;
the 262 contracts common to the normal host are identical. Regeneration from
both schemas preserves all 57 documented crates and all optional networking
commands. Neither multiplayer launch flag is supplied. Verified gzip archives
retain both builds, and the exact tested normal executable is restored.

Disk recovery cleans only this checkout's `bevy_render`, `faer` and
`lunco-luncosim` build outputs and losslessly compresses this task's three large profiler CSVs:
`frame-contract-cpu-20261009.csv.gz`, `native-vsop-cpu-20261009.csv.gz` and
`architecture-next-cpu-20261009.csv.gz`. Source, sibling worktrees and shared
Cargo/sccache caches are preserved. Use `gzip -dc` to read those evidence files.

## Physics-frame lifecycle invalidation — 2026-10-09

`lunco-usd-avian-core` replaces repeated component-change scans with native
lifecycle observers and a change-ticked resource. Admission and fixed-frame
consumers independently observe physical membership, Grid/CellCoord presence,
immutable ChildOf insertion/removal and active-frame changes/removal. Ordinary
pose values do not invalidate connectivity. The full validator, owner
diagnostics, solver holds, precision and schedule boundaries remain unchanged.
No external library is modified.

The settled 10–16 s Tracy slices exclude startup, deferred flushes and injected
scenario checks. The gate-system body has 534/742 events before/after: p50 falls
from 0.037319 to 0.001463 ms (96.08%), and p95 from 0.094256 to 0.007675 ms.
The first scenario command occurs after 22 s in both captures. This establishes
lower caller cost; different hardware clocks and event counts prevent a causal
product-FPS claim. Comparison:
`target/image-loader-perf/frame-contract-tracy-comparison-20261009.json`.

The generic lifecycle test passes. Eight production Rhai checks disconnect a
runtime body, require the `usd-avian`/`physics-frame` diagnostic and stopped
solver ticks (139 to 139), then replace the scene and require recovery (154 to
275). Topology remains 50 bodies, 33 colliders and 20 joints. The ten-profile
production replay matrix passes 360 exact-state assertions against the unchanged
reference, including deliberate mismatch rejection. Its initial 20-rover serial
attempt reaches the helper's 60 s timeout during Modelica preparation under
concurrent CPU load; it has no determinism verdict. The resumed run allows the
runner's existing admission bound to complete and passes. Evidence prefixes:
`frame-contract-runtime-20261009` and `frame-contract-current-replay-20261009`
under `target/image-loader-perf/`.

Normal artifact
`3b7648b8ac5fd7fc1951857ac6e35047267fc02cfffe1dcd04b66108de104594`
passes 26 rendered history/smoke/replacement checks in owned PID 1821445/API
4198. View/Builder/View retain High quality, 1280×720, an exact stable camera
pose, advancing stable physics topology and zero clipped demand. Frame
observation p50 is 10.950/10.760/10.977 ms; native physics-ring p50 is
1.227/1.235/1.275 ms, and whole fixed-tick p50 is 2.311/2.453/2.617 ms.
Readiness first clears at 13.655 s. GPU clock is 2610 MHz, versus about 367 MHz
in the earlier normal observations, so these values are not a controlled A/B.
The screenshot retains geometry and shadows. Evidence:
`target/image-loader-perf/frame-contract-normal-20261009.json`.

Tracy artifact
`5abae475d75611751aecffd5eb3edb52ab9dd3912e7405185bdcba8df4ad0df7`
runs in owned PID 1828061/API 4196 with collector 1828060/8086. Readiness,
diagnostics, stable camera pose, physics topology and 25 production checks pass.
All successful apps and collectors exit zero and their ports close. Verified
gzip archives retain both artifacts; the tested normal binary is restored.
The owner Rust file receives one formatter pass after validation, containing
only layout and trailing-comma changes.

The normal loading log records rover Modelica lowering misses of 4.8–7.3 s,
including a thermal child queued for 6.47 s. Headless probes hit the prepared
cache for a source identity that the rendered run misses. Source identity alone
does not establish the complete cache key: solver, parameter overrides and cache
location still need comparison. Loading, visual-FPS and
physics-FPS acceptance remain open.

## Exact-epoch native ephemeris operand reuse — 2026-10-09

`lunco-celestial-ephemeris` retains the native f64 heliocentric ICRF EMB/Earth
operands alongside its existing final-position cache. Earth, Moon and EMB
consumers share those samples at one exact Julian-Date bit pattern. An epoch
change clears both maps, including missing results. Subtraction, coordinate
rotation, parent composition, schedule order and simulation precision remain
unchanged. This is generic analytical reuse at the provider's existing mutex
boundary; it introduces no policy hook, worker or external-library change.

The 10–16 s Tracy slices exclude injected scenario tests and deferred command
flushes. Same-thread ephemeris spans are counted only when fully contained in
the link system body:

| Owner | Before p50 (ms) | After p50 (ms) | Events before/after |
| --- | ---: | ---: | ---: |
| Link sweep | 1.953829 | 1.281443 | 23/22 |
| Earth evaluation | 0.700889 | 0.665944 | 23/21 |
| Moon evaluation | 0.683276 | 0.201957 | 23/21 |
| EMB evaluation | 0.156833 | 0.000200 | 23/21 |

Link p50 is 34.41% lower; p95 is 2.523623/1.405003 ms. Earth still performs
the native evaluations when it first requests them; the later Moon and EMB
calculations reuse them. The Moon duration includes its ELP child span, which
is not added again. These are instrumented caller costs with different camera
conditions, not a controlled visual A/B or product-throughput claim. The first
scenario command occurs at 17.001 s before and 22.584 s after. Evidence:
`target/image-loader-perf/native-vsop-tracy-comparison-20261009.json`.
The before capture is `scenario-model-query-tracy-20261009.tracy`; the after
capture is `native-vsop-tracy-20261009.tracy`, executable
`9f8e076f6ac49c2988f7b6abd84405548176cc21198e4a39e476ade66c296cc8`,
owned PID 1667873/API 4193, collector 1667872. Listener and connection ownership,
readiness, diagnostics and advancing stable physics topology pass. The 17-check
history and seven-check smoke gates pass on the same replaced host; the empty
replacement negative gate passes with unchanged script status and inspection.
The screenshot shows geometry and shadows, but the camera rotation changes
during capture and reflected projection text is empty. Neither supplies visual
A/B acceptance. App and collector exit zero; ports close.

Normal executable
`89a68199ec55f062c3542fa4dec0cb8afd9c6802c32fbfe341661b0d028bc86e`
passes 64 production Rhai checks against 32 public `BodyPosition` baseline
samples for Moon, Earth, EMB and Sun at adjacent, revisited and separated epochs.
Queries alternate body order, repeat results exactly and reject deliberately
altered expected points. Both position-probe processes exit zero and API 4191
closes. The unchanged ten-profile production replay matrix passes 360 exact
state checks, including mismatch rejection; every process exits zero and API
4180 closes. `comms_demo.usda` passes all eight authored routing/off-cycle hook
checks, exits zero and closes API 4194. The generic native-cache seam test and
both production builds pass without warnings. Evidence uses the
`native-vsop-positions-*`, `native-vsop-current-replay-*` and
`native-vsop-comms-*` prefixes under `target/image-loader-perf/`.

Two normal visual measurement attempts reject their first window because the
camera moves. The second records hundreds of metres of active-frame movement
with a 2560×1568 render target, versus the earlier 1280×720 accepted run. Their
readiness observations are 2.496/2.313 s; neither is a causal loading or FPS gain.
Both apps exit zero and API 4192 closes. The rejected evidence is preserved in
`native-vsop-normal-20261009.json` and
`native-vsop-normal-camera-audit-20261009.json`.

Verified gzip archives retain both binaries; the tested normal executable is
restored byte-for-byte. The touched Rust file receives one focused formatter
pass after validation. Changes remain uncommitted and nothing is pushed.
The overall loading, visual-FPS and physics-FPS objective remains open; native
GPU shadow work and whole-simulation command/snapshot ownership still need
architectural work and product acceptance.

## Native scenario-model query reuse — 2026-10-09

`lunco-scripting` shares one World-owned native Bevy query state across
preparation, startup, fixed ticks and visualization. It caches archetype matches,
not actors or values. Every pass reads current model, authority and scope facts
and releases model borrows before hooks or structural commands. Existing stable
actor sorting, execution contexts, fixed schedules and precision remain intact.
The existing `RebuildOnChange` owner already retains its native query state;
it is not changed. No external library or authored USD is changed.

The same pre-test 10–16 s Tracy slice has these caller-system p50 costs:

| System | Before (ms) | After (ms) | Events before/after |
| --- | ---: | ---: | ---: |
| Scenario startup | 0.015049 | 0.004358 | 173/187 |
| Scenario preparation | 0.027611 | 0.018374 | 173/187 |
| Scenario fixed ticks | 0.015258 | 0.004418 | 357/359 |
| Scenario visualization | 0.014156 | 0.006422 | 173/187 |

These instrumented caller costs establish an owner-cost reduction, not a pure
query benchmark or product-throughput gain. The 10–18 s after slice includes
injected test assertions, including a 473 ms tick. It is retained separately;
native command events place the first replacement at 18.416 s before and
17.001 s after, outside the selected slice. Comparison:
`target/image-loader-perf/scenario-model-query-tracy-comparison-20261009.json`.
The before capture is `scenario-retirement-tracy-20261009.tracy`; the after
artifact is `4cc9ecf2290f46ce72e8115431bbf0af56c20c8903f90db020850d6e82d43944`,
PID 1643140/API 4190, collector 1643139. Profiler listener/connection ownership,
readiness, diagnostics, stable within-run camera pose and advancing stable
physics topology pass. The reflected projection text is empty, so it supplies
no projection-equality evidence. Both app and collector exit zero; their ports
close. The reviewed screenshot retains geometry and shadows.

The tested normal artifact is
`a890c284384d91d83e6ff5c62dfa7f6f1d624c9cbd1ee10da8da9750cd764414`.
All ten production replay profiles pass 360 exact-state checks against the
unchanged current-main reference, including deliberate mismatch rejection.
Authored replay source and tracked reference remain unchanged. Each process
exits zero and API 4180 closes between profiles. Evidence:
`target/image-loader-perf/scenario-model-query-current-replay-20261009.json`.
The focused neutral-driver test
`async_compiles_commit_as_one_stable_batch_after_reverse_completion` passes
with `--features rhai`; the initial default-feature command executes zero tests
and is not counted. Normal and Tracy production builds pass without warnings.

Owned normal PID 1636788/API 4189 passes the 17-check history and seven-check
smoke gates through a newly admitted host and its replacement. Two empty or
whitespace replacement checks reject with identical before/after `ScriptStatus`
and full `ScriptInspect` snapshots. View/Builder/View retain High quality,
1280×720, readiness, diagnostics, stable camera pose and advancing stable physics
topology. Frame-observation p50 is 32.282/33.089/33.104 ms; native physics-ring
p50 is 0.933/0.914/0.930 ms, and whole fixed-tick p50 is
1.852/1.693/1.860 ms. Clipped demand stays zero. Readiness first clears at
2.922 s; GPU utilization is 99%, clock 367 MHz and power 19.97 W. App exit is
zero and the port closes. Screenshot geometry and shadows pass visual inspection.
Evidence: `target/image-loader-perf/scenario-model-query-normal-20261009.json`.

The historical normal and current normal runs differ by one float ULP in one
camera rotation component, although each run preserves its pose exactly.
Together with the single run per artifact, this prevents a strict visual A/B
or causal loading/FPS claim. The observations and camera difference are recorded
in `scenario-model-query-normal-comparison-20261009.json`. All task-owned sessions
are closed. Verified gzip archives retain both exact binaries, and the tested
normal executable is restored byte-for-byte. Changes remain uncommitted and
nothing is pushed. The touched Rust file is formatted once after validation;
the 43-skill catalogue and `git diff --check` pass. The full performance goal
remains open: dominant GPU shadow
work and whole-simulation command/snapshot ownership still need architectural
work and product acceptance.

## Native scenario-retirement query reuse — 2026-10-09

`lunco-scripting-rhai-runtime` retains Bevy's native `QueryState` for pending
scenario hosts in its serial `PreUpdate` retirement system. The query reads
current membership and admits newly matched archetypes; host identities are
collected before teardown mutates the world. Retirement order, lifecycle policy,
fixed schedules, simulation precision and external libraries are unchanged.
Empty or whitespace-only `RunScenarioAsset` sources reject before URI conversion
or pending-host publication. The production negative gate exposed that the
input check had followed conversion to a nonempty library URI.

The existing before capture and owned after capture compare the 10–18 s slice.
Retirement system p50 falls from 0.081873 to 0.002074 ms (97.47%); p95 falls
from 0.181699 to 0.006061 ms, with 234/231 events. These instrumented system
durations establish an owner-cost reduction, not product FPS. Comparison:
`target/image-loader-perf/scenario-retirement-tracy-comparison-20261009.json`.
The after executable is `dea9f8e33c40b4c01f59eba7c3c5acecc90302b2be7124965987fb024ec99b7a`,
PID 1606368/API 4187, collector 1606367. Listener/connection identity, camera
pose/projection, readiness and advancing stable physics topology pass. Its
17-check history and seven-check smoke gates use the same host. A later
empty-source negative assertion fails before the command-input repair; the
preceding timing window and capture remain complete. API Exit is processed
and the process/port close; the driver does not retain the app return code
after that assertion. Collector exit is zero. The final retirement function
matches the captured source exactly; the command-input fix is outside it.

Final normal executable `6415f599b1d4198ec97cbbe056973b6f607602c354a318890e8a56fbd23958f0`
passes the 17-check history, seven-check smoke and two empty/whitespace rejection
checks on PID 1609295/API 4188. `ScriptStatus` and complete `ScriptInspect`
snapshots remain identical across rejections. View/Builder/View windows retain
equal camera poses, High quality, readiness, diagnostics and advancing stable
physics topology. Frame p50 is 33.007/33.791/34.007 ms; physics-service p50 is
1.030/0.971/1.065 ms and whole fixed-tick p50 is 2.045/1.901/1.989 ms. Clipped
demand remains zero. GPU utilization is 99%, clock 352 MHz and power 19.98 W.
There is no demonstrated loading or product-FPS improvement. The normal
and Tracy screenshots retain geometry and shadows on visual inspection.
Evidence: `target/image-loader-perf/scenario-retirement-final-normal-20261009.json`.
App exit is zero and port 4188 closes. An earlier normal measurement completes
its windows and two positive gates before a harness `ScriptStatus` query omits
its required target; it is retained separately, not counted as the final gate.

The focused scripting-runtime check and production builds pass without warnings.
The touched Rust file is formatted once after validation; skill validation and
`git diff --check` pass. All task-owned ports 4185–4188 and the collector are
closed. Changes remain uncommitted and nothing is pushed. The full performance
goal remains open; native GPU shadows still dominate visual frames.

## Relative shadow-pose evidence — 2026-10-09

Owned normal PID 1559815/API 4185 uses the prior exact normal artifact
`9c5d65cc450467b491fa12a8c0984afa57c4fc8078276de77905b3274d2f98c3`.
After full readiness and twelve seconds of settling, 24 production API snapshots
over 4.25 seconds contain all 206 meshes and twelve spotlights. The camera and
physics topology stay fixed while physics advances. All render affines change;
198 mesh positions, 133 mesh rotations, twelve light positions and eight light
rotations also change in the active simulation frame. Four lights have 25
exactly stable active-frame relative mesh poses each, but no complete scene or
light has stable relative render affines. These comparisons invert the sampled
affines in double precision; they are not GPU shader bit-equivalence tests.

Native spot shadow views additionally derive their basis from forward direction,
not full authored roll. Native preparation creates current depth attachments
and clears them at first use each frame. The measured inputs therefore do not
justify unchanged-map reuse, and no shadow cache is added. The snapshot and
analysis files are `target/image-loader-perf/shadow-relative-pose-samples-20261009.json`
and `shadow-relative-pose-analysis-20261009.json`. App exit is zero and port
4185 closes. A later screenshot request occurs after its bounded session
already closes and supplies no image; source and shadow draws are unchanged.

## Retained fixed-cycle demand — 2026-10-09

`lunco-time` admits the complete running virtual delta and drains at most 64
complete causal cycles per app update. All remaining duration stays in
`Time<Fixed>::overstep`; pause, solver and scene holds retain that balance.
The fixed timestep, serial World access, per-cycle admission check, ordered
physics/Modelica/Rhai work and completed-tick calendar are unchanged. The
telemetry query reports the full balance as
`fixed_loop.latest_pending_simulation_secs`. Manual scene-test and recording
consumers share this owner; the recorder waits for every whole admitted cycle
before readback. The standalone Lunica composition no longer overwrites its
workbench-installed clock. Current contracts are in
[unified time](../architecture/19-unified-time-and-clock.md) and
[offline recording](../offline-recording.md).

The new generic runner seam first fails on a 100 ms frame at 0.1x: only 3.3 ms
is admitted instead of 10 ms. It now passes at 0.1x/1x/64x, proving exact
completed-plus-pending conservation, the 64-cycle work bound, pause retention
and full later draining with no additional input duration. All 54 existing
and replacement time-owner tests pass using that same built test binary.
No repository/Twin asset is embedded in these generic Rust tests.

The normal production artifact is SHA-256
`9c5d65cc450467b491fa12a8c0984afa57c4fc8078276de77905b3274d2f98c3`.
All ten production replay profiles pass 360 exact-state checks against the
unchanged current-main reference, including deliberately mismatched-state
rejection. The authored source and tracked reference remain unchanged. Evidence:
`target/image-loader-perf/fixed-clock-budget-current-replay-20261009.json`.
The production `sensor` scene passes 58 checks through `luncosim test` on
API 4183, including the renamed pending-duration field and negative query
cases; its process exits zero and its port closes. Log:
`target/image-loader-perf/fixed-clock-budget-sensor-20261009.log`.

The owned normal High-quality sandbox (PID 1406381/API 4181) uses the same
fixed epoch, settings, camera pose, warmup and View/Builder/View windows as the
metadata-after baseline. Frame-observation p50 is 30.776/31.146/31.270 ms and
p95 is 33.006/34.658/34.306 ms. Native physics-ring p50 is
1.055/0.972/0.943 ms. Whole fixed-tick service p50 is 1.949/1.958/1.952 ms;
loop p50 is 3.600/3.730/3.688 ms. Latest pending duration is
0.009555/0.005378/0.017136 s. Cumulative clipped demand is zero in every
window, versus 0.665 s by the final baseline window. Both windows and later
17-check history / seven-check smoke gates pass; the camera, readiness,
diagnostics and advancing stable physics topology are checked per window.
The app exits zero and API 4181 closes. There is no demonstrated loading,
visual-FPS or tick-service speedup; the improvement is retained simulation
demand. Comparison:
`target/image-loader-perf/fixed-clock-budget-normal-comparison-20261009.json`.

The combined driver subsequently fails its first new recording attempt on a
signed/unsigned comparison in the test script; those comparisons are corrected.
A startup-only harness attempt calls a nonexistent lifecycle method, and a
subsequent attempt rejects still-pending scene admission. Neither supplies a
recording verdict; their diagnostic artifacts remain separate. The corrected
sandbox attempt passes five of six checks, but Modelica barriers prevent it
from witnessing a full 64-cycle update. On the existing lightweight
`diagnostic_visuals` fixture, PID 1414375/API 4182 passes all six recording
checks: three PNGs are saved, full budget and pending duration are witnessed,
and readback never starts with whole cycles pending. Exit is zero and the
port closes. Its separate fixture observer logs an unrelated unsigned-count
comparison error; this result certifies the six recorder assertions, not the
fixture's diagnostic-lease test. Current evidence:
`target/image-loader-perf/fixed-clock-budget-recording-current-20261009.json`.
The normal sandbox screenshot and first recording PNG are visually inspected.

A separate owned Tracy capture uses SHA-256
`c0f49cb610497a088fd471a32a7aa9dbf42550018abaa2cf4003d86e2c87ff50`,
PID 1430976/API 4184 and collector 1430975; listener/connection identity,
readiness, diagnostics, stable camera and advancing stable topology pass.
App and collector exit zero, and the API port closes. Trace:
`target/image-loader-perf/fixed-clock-budget-tracy-20261009.tracy`.
Its 10–18 s diagnostic slice contains 2,808 shared early shadow events
(234 groups of 12 views), totaling 4,620.311 ms, and 233 camera early events,
totaling 1,238.233 ms. Normalized aggregates are 19.745 ms per shared group
and 5.314 ms per camera event; these are not paired per-frame samples.
Opaque-pass p50 is 3.235 ms. The CPU fixed runner has 234 events with
9.034/18.685/24.241 ms p50/p95/max under instrumentation. The timing query
still reports zero clipped demand. Source snapshot, screenshot, CPU/GPU CSVs
and analysis are retained; the screenshot preserves geometry and shadows.
Analysis: `target/image-loader-perf/fixed-clock-budget-tracy-analysis-20261009.json`.
These diagnostic costs do not replace the normal product measurement.

Normal and Tracy production builds pass. The standalone Lunica binary passes
`cargo check -p lunco-modelica-ui --bin lunica -j 4` without warnings. The
five touched Rust files are formatted once after validation. No external
library, authored USD, simulation scalar precision or tick order is changed.
Full goal acceptance remains open: GPU shadow cost and a whole-simulation
command/snapshot ownership boundary still dominate the next architectural work.
Changes remain uncommitted and nothing is pushed. All owned ports 4180–4184
and collector sessions are closed.

Verified compression preserves the exact normal and Tracy executable bytes;
the tested normal artifact is restored to `target/debug/luncosim`. Current
archive mappings are
`target/image-loader-perf/fixed-clock-executable-compression-20261009.json`.
Source, sibling worktrees, registry and shared sccache are untouched.

## Physics metadata ownership — 2026-10-09

`lunco-usd-sim-telemetry` retains a publication flag per cached signal and moves
new metadata into `SignalRegistry`. Descriptions, units, provenance, groups and
presentation have one value owner. Per-source identity, global-owner tracking,
path invalidation, scalar values and sampling clocks retain their contracts.
In the observed 1,954-channel physics catalog, this removes 1,954 full metadata
copies and at least 7,816 owned string copies (four non-empty fields per channel,
with compound-presentation strings additional). This is a source-proven work and
storage reduction, not a measured product speedup.

The finer baseline trace is
`target/image-loader-perf/telemetry-retention-phases-20261009.tracy` (SHA-linked
executable `3b7894c5fb4bdb0ba2af8beafe37f02ed6d8783d54894c980a69be7dc2887b66`,
PID 1323696/API 4176, collector 1323695). It finds 4,007 channel creations,
maximum 0.921 ms, including a 0.420 ms creation at map size 1,792. Physics
metadata spans total 2.981 ms; source retention peaks at 1.368 ms. The prior
17.637 ms source spike does not recur, so its origin remains unproven.
The after trace uses executable
`faa3a4958a78682bcff3181fd5e3289c73ff78090c3578392d5e8d21f3c57deb`,
PID 1326661/API 4177 and collector 1326660. The same 1,954 metadata spans
now total 3.943 ms, and channel creation peaks at 2.227 ms. These profiler
windows do not establish a timing gain. Both owned captures pass readiness,
diagnostics, stable camera and advancing stable physics topology; their apps
and collectors exit zero and ports close. Source snapshots and per-event CSVs
are retained beside the traces. Comparison is
`target/image-loader-perf/telemetry-metadata-owner-comparison-20261009.json`.

Separate normal High-quality runs use a 12-second warmup and three six-second
View/Builder/View windows, with identical authored scene, camera pose and quality.
The root has the fixed authored epoch JD 2461395.5. Baseline PID 1329457/API 4178
uses SHA-256 `9112d26d8e24086ed63aa48284edb99f67653cb801ba28adc3b162c9972c4b6b`;
after PID 1339781/API 4179 uses
`6d125ffa0d1506f11a5359620401c3797c3fd732598de081ba7aa02156588dba`.
Frame-observation p50 is 30.478/30.723/31.323 ms before and
30.233/31.253/31.660 ms after. Physics-ring p50 is 0.936/0.944/0.933 ms before
and 0.945/1.013/1.000 ms after. First observed readiness is 2.389/2.854 s.
These runs establish no overall loading, rendering or physics speedup.
GPU utilization stays 99%, at 375/390 MHz and approximately 20 W; the owned app
is the recorded GPU process. Raw frame exposures are revision-deduplicated API
observations (95–98 per window), not every rendered frame. Native physics rings
retain 120 steps. Both app exits are zero and both ports close. The reviewed
screenshots retain scene geometry/shadows. Comparison is
`target/image-loader-perf/telemetry-metadata-normal-comparison-20261009.json`.
An initial baseline driver lost the process handle only while saving shutdown
facts after all windows completed. Its app/port closed; its exit code is not
claimed. The completed baseline above supplies the full lifecycle evidence.

The production `telemetry_recording_history.rhai` gate passes 17 checks,
including mass metadata, steady-value history, pacing/retention and absent-channel
rejection; sandbox smoke passes seven. The fresh normal binary also passes all
ten exact production replay profiles and 360 checks, including deliberate
mismatch rejection, with scenario and tracked reference unchanged. Evidence is
`target/image-loader-perf/telemetry-metadata-current-replay-20261009.json`.
The after timing profile's whole fixed-tick p50 is 1.935–1.982 ms, with
3.626–3.830 ms fixed-loop bursts. Its cumulative max-delta-limited simulation
demand reaches 0.665 s; the final latest sample is 0.001209 s. Mean Avian timing
alone therefore does not establish whole-loop or wall-clock cadence acceptance.

Focused normal and Tracy builds pass without warnings. Touched Rust files were
formatted once after validation; the diff check and 43-entry skill catalogue
pass. No external library, authored USD, authoritative scalar or ordered tick
boundary was changed. All owned ports 4176–4180 and collector sessions are
closed. Changes remain uncommitted; nothing was pushed. Full loading, visual-FPS,
physics-tail and UI-isolation acceptance stays open.

Task-owned executable compression preserves exact bytes with verified restored
SHA-256 hashes and frees 2.71 GB. Current archive mappings are in
`target/image-loader-perf/archived-executable-compression-20261009.json`.
Source, sibling worktrees, Cargo registry and shared sccache are untouched.

## Owned shader-raster loading — 2026-10-09

`lunco-materials` prepares decoded, owned images on the async-compute pool
before publishing typed `PreparedShaderImage` roots and their native image
children. `ShaderTexture` retains each colour/scalar/normal source independently
and preserves native reload dependencies. Extension-free registration leaves
native image and glTF decoding/settings intact. The render binder consumes
complete children and reacts to source events; its resident-pixel snapshot,
task/version registry and mip installation pass are removed. Prepared children
use native render-only extraction, moving bytes into GPU preparation while
retaining main-world descriptors. Generated terrain/annotation images retain
their native ownership. The canonical contract is in
[shader layers](../architecture/shader-layers-and-params.md).

The focused production-feature test command
`cargo test -p lunco-materials -p lunco-luncosim --lib image_ -j 4` passes ten
tests, including initial/reload complete chains, independent same-file roles,
native loader settings, sampler quality, render-only child usage, invalid PNG
and invalid pixels, plus shared pixel math. Inline codec fixtures exercise the
asset seam without repository/Twin paths. Evidence is in
`target/perf/image-raster-source-seams-20261009.log`.
`ShaderTexture::load_raster` accepts physical raster paths and rejects container
labels with a typed error before issuing a prepared-root request. USD shader
admission consumes that error through the existing texture-source diagnostic.
Importer-owned labeled images use `ShaderTexture::Image`; an inline native
container-loader test verifies their load, binding and reload, and verifies that
a rejected raster request starts no prepared-root load. Physical asset identity
and importer-owned labels remain separate contracts.

The source-admission build has SHA-256
`d8fa3aa3459fe09438760c7ddf96821bf08f6d541c034ea07a780ea7f9c6535f`.
The production Rhai replay passes all ten serial/default/jitter profiles and
360 checks against the unchanged current-main reference. It compares exact
physics and articulated state plus all 438 Modelica variable names/value strings
at maintained checkpoints, and rejects deliberate mismatches. Source and tracked
reference files remain unchanged. Evidence is
`target/image-loader-perf/image-raster-current-replay-20261009.json`.
A separate current rendered sandbox run (PID 1297561/API 4173) passes seven
smoke checks, readiness and diagnostics. Its reviewed screenshot retains
geometry and shadows; API Exit returns zero and the port closes. This is
rendering correctness evidence, not a new throughput window. Files are
`target/image-loader-perf/image-raster-rendered-current-20261009.*`.

The retained raster-worker Tracy executable SHA-256 is
`c75aeb67f36172322549a7a68db0a1c0cc0e635c0f9f520aecfc7b7316d53c2b`.
Owned PID 1257840/API 4168 and collector 1257839 verify the owned 8086 listener
and connection. Two `lunco_shader_raster_prepare` jobs take 196.14/178.67 ms
on worker threads 13/14. The removed app-thread preparation zone has no events.
Native `GpuImage` extraction has a 0.875 ms maximum over 312 calls and 4.34 ms
total in the first five traced seconds; these aggregate native spans are not
isolated texture costs or a loading-critical-path measurement. Readiness,
diagnostics, active-camera pose/audit, physics progress/topology and screenshot
geometry/shadows pass. The reflected projection string is empty, so it does
not independently validate projection fields. App and collector exit zero;
API 4168 closes. Raw trace is
`target/image-loader-perf/image-loader-after-20261009.tracy`, 210,454,898 bytes,
SHA-256 `8c41fd20789aff4a125c9782f03b5e2a0e731c02ff29d7d00dee89d1eabd756d`.
Source snapshots, individual worker/extraction CSVs and process facts are
beside that capture.
Both exact production executables are retained as verified gzip archives in
`target/image-loader-perf/image-loader-{tracy,normal}-luncosim.gz`. The artifact
manifest records original identities; the compression manifest records restored
hashes and current archive paths.

The retained render-only throughput executable SHA-256 is
`44768b127d9f464ad74f12b51a3d69e14f4a05f4b3b71739d220bf9c09c258a2`.
Owned PID 1269633/API 4169 measures High, 1280×720, no Vsync/throttle, a
12-second warmup and three six-second View/Builder/View windows. First full
readiness is 3.038 s. Retained 60 Hz app-frame observations have p50
30.658/31.163/30.754 ms and p95 33.363/33.981/33.033 ms, 363 rows per window;
these can repeat a rendered frame. Native 120-step physics-ring p50 is
0.935/0.918/0.912 ms and p95 1.188/1.104/1.093 ms. This run establishes no
overall loading, visual-FPS or physics gain. Readiness, diagnostics, camera
pose/audit and physics progress/topology pass. The recording gate passes 15
checks and sandbox smoke passes seven; the reviewed screenshot retains
geometry and shadows. API Exit is zero and the port closes. Runtime evidence
is in `target/image-loader-perf/image-loader-render-only-final-20261009.*`.
The GPU remains near 99% utilization at about 405 MHz and 20 W; short normal
observations do not establish uncontended hardware-independent acceptance.

An earlier normal run before render-only extraction is retained in
`target/perf/image-loader-final-20261009.*`; it had no authored scenario
verdicts. Separate logs retain a wrong smoke asset request and an early
readiness race, followed by a successful corrected seven-check smoke run.
Neither failure is counted as negative-case coverage. The asset tests above
are the intentional negative cases.

Disk recovery preserved and SHA-verified all 665 performance-evidence files
before cleaning this checkout's generated `target/` outputs. `target/perf`
now links to `/var/tmp/luncosim-optimization-perf-20261009/evidence`; large new
captures stay on the home filesystem. Archive/compression manifests retain
artifact identities. Fresh standard-profile builds and the focused test pass
after the clean. Source, sibling worktrees and shared caches are preserved.
No external library, authored USD, authoritative simulation value or ordered
tick boundary changes in this step. Image worker completion controls
presentation asset availability, not authoritative simulation input order.
The exact production replay above strengthens determinism evidence for this
change; smoke gates alone do not prove it. Full loading, visual-FPS, physics-tail and
determinism acceptance remain open. No commit or push was made.
Source-admission validation reuses the passing replay after documentation-only
edits; formatting and final review are recorded separately.

The same capture identifies native `RenderMesh` extraction at 24.128 ms maximum
and 36.226 ms total in the first five traced seconds. Current CPU consumers
include editor mesh picking (`lunco-luncosim-edit-ui::selection` and the UI's
native `MeshPickingPlugin`), horizon baking and mesh-terrain collision admission.
Native render-only mesh extraction removes attributes/indices from main-world
storage, so changing all mesh usage flags would break those readers. This is
an ownership investigation requiring per-source attribution and retained
reader semantics before an optimization, not evidence that the image usage
choice can be applied indiscriminately to meshes.

## Startup owner attribution — 2026-10-09

The owned High-quality sandbox capture
`target/image-loader-perf/startup-owner-phases-20261009.tracy` uses executable
SHA-256 `1a292ba6d28c114cdc5c941ec5639d1c07c46cc0161e9da62b3f64a509b4d254`,
PID 1304802/API 4174 and collector 1304801. Listener ownership, readiness,
diagnostics, stable camera pose/audit and advancing stable physics topology
pass. The reviewed image retains geometry/shadows; both processes exit zero
and the API closes. The raw capture, exact executable and source snapshots are
retained. Readiness at 4.132 s is profiler-run evidence, not loading acceptance.
GPU utilization is 99% at 405 MHz/~20 W, with this app the recorded GPU process.

Internal spans separate engine completion snapshot from document installation
and physics channel retention from kinematics/contact collection. The engine's
14 snapshot calls total 6.762 ms, maximum 3.235 ms. Seven per-document installs
total 8.120 ms, maximum 3.791 ms. The outer engine maximum is 11.239 ms;
its longest call is outside completion installation. Port-validation command
flush maximum is 2.613 ms, versus 29.984 ms in the earlier capture with no
validation implementation change. That variability does not prove a fix.
Physics telemetry's maximum is 20.782 ms; 17.637 ms lies inside one source's
metadata-discovery/registry-retention interval. One later non-discovery source
also reaches 7.661 ms. The capture localizes wall time, not allocator versus
scheduler CPU service. Registry discovery/growth needs further attribution
before choosing a mechanism. The parsed-document install is not established as
the dominant startup blocker. Per-event exports and aggregate evidence are in
`target/image-loader-perf/startup-owner-phases-analysis-20261009.json`.

These diagnostic spans use the native debug filter documented in the profiling
skill; ordinary info-level runs do not enable them. No new cache, asynchronous
commit path or physics scheduling decision was added. A separate owned
`BindingStatus` inventory found 104 bound wires and 102 distinct sources:
only two reads are redundant. That workload does not justify source fan-out
indexing. Evidence is
`target/image-loader-perf/cosim-source-fanout-20261009.json`.

The final normal build passes without warnings. Its SHA-256 is
`9112d26d8e24086ed63aa48284edb99f67653cb801ba28adc3b162c9972c4b6b`.
One focused serial four-rover replay passes 24 exact-state/negative-control
checks (PID 1312511/API 4175), exits zero and closes its port, with production
scenario and tracked reference unchanged. This supplements the ten-profile
source-admission replay; it is not a new full replay matrix. Evidence is
`target/image-loader-perf/startup-owner-normal-replay-20261009.json`.
Touched Rust files were formatted once after implementation and validation;
`git diff --check` and the 43-entry skill catalogue pass. Owned ports
4173/4174/4175 and the collector listener are closed. No commit or push was
made. Full product loading, visual-FPS and physics-tail acceptance stays open.

## sRGB mip filtering — 2026-10-09

`lunco-materials::rgba8_mip_chain` decodes the 256 possible sRGB byte values
once per colour chain instead of repeating the nonlinear transfer for every
sample. It reuses the same native f32 transfer function, sample order,
summation, encoding, alpha averaging and mip extents. Linear/normal chains do
not build the table. This is shared pixel math consumed by shader-raster
asset loading and terrain preparation; it adds no policy, hook,
dependency, API, cache lifecycle or simulation scheduling boundary.
The [canonical shader contract](../architecture/shader-layers-and-params.md)
and profiling skill describe the owner and evidence boundaries.

Seven native tests pass, including invalid inputs, normal/linear filtering,
non-power-of-two/rectangular extents and an sRGB comparison covering 65,536 byte pairs
against the transfer formula. The pure standard-library module was compiled
directly with the repository toolchain using
`rustc --edition 2024 --test -C opt-level=2 crates/lunco-materials/src/image_mips.rs -o target/perf/image-mips-mechanism-20261009`,
then `target/perf/image-mips-mechanism-20261009`;
the production builds separately validate package integration. A direct
before/after comparison also verifies every byte of a 44,739,244-byte,
13-level synthetic 4096×2048 chain. Its source and verdict are retained in
`target/perf/image-mips-full-byte-comparison-20261009.{rs,log}`.

The isolated same-toolchain/same-input A/B/B/A workload runs three repetitions
per process. Before median filtering is 291.95/290.28 ms; after is
88.08/88.46 ms, about 70% lower. Input construction and output verification
remain outside the timed filter. Full outputs have matching level counts,
sizes and checksums; the separate direct byte comparison above is stronger
than checksum identity. Source, executable identity and timings are retained
in `image-mips-bench-*-20261009.*`. These are owner-workload results, not
loading or FPS acceptance.

Temporary per-phase spans identify the production startup spike as base
snapshot copying: two 4096×2048 RGBA8 images copy 64 MiB on the app thread,
with one copy taking 51.14 ms. Event handling, request discovery, polling and
installation are small; mip filtering already runs off-thread. Owned before
PID 1079602/API 4161 has two worker jobs at 427.80/382.16 ms; after PID
1087314/API 4162 has 154.63/162.27 ms. Both install two results and pass
readiness, diagnostics, camera-audit stability, physics progress/topology and
visual shadow/geometry checks. Their verified owned 8086 listeners and
collector connections identify the captures. Both apps/collectors exit zero
and the API ports close. The after snapshot copies are 6.75/5.15 ms; no
snapshot mechanism changed in that measurement pair, so this variability
does not prove a copy reduction. The owned-loading step above removes that
resident snapshot at its asset owner.

Raw captures remain at `/var/tmp/luncosim-optimization-perf-20261009/shader-image-phase-20261009.tracy`
(210,152,621 bytes, SHA-256
`3fb1849379e39febcbca369fbad674401497ab65150eedc68efbfb77a7219c8c`)
and `/var/tmp/luncosim-optimization-perf-20261009/image-mips-decode-after-20261009.tracy`
(208,668,283 bytes, SHA-256
`d7a95fc5594bdde6cb5af1bb51354a5571700ade000e9de1e8ebae41fdbcd745`).
Profiler executable SHAs are
`559131e0b348318bd731244dc4b01786a0c9ad6f4c398423e9363910d0647574`
before and
`0bb1a807cd8543396eb37fec245956e5e9d1b842cfcdb32150fa7b9ce230d3f8`
after. Sources, filtered inclusive/self exports and the phase comparison are
in `shader-image-phase-*` and `image-mips-decode-phase-pair-20261009.json`.
The temporary phase spans are removed.

The final normal `target/debug/luncosim` SHA-256 is
`115d08da22cc2cbf3e2522831d840cb24d166c2c6d06ac38586fadc53137ca90`.
Owned PID 1093436/API 4163 measures High, 1280×720, no Vsync/throttle and three
six-second View/Builder/View windows. Active-camera pose/projection,
readiness, diagnostics and physics progress/topology pass. Retained 60 Hz raw
app-frame observation p50 is 30.69/30.90/31.04 ms and p95 is
33.60/33.84/33.62 ms; native 120-step physics-ring p50 is
0.915/0.921/0.941 ms and p95 is 1.029/1.195/1.156 ms. Readiness first clears
at 2.56 s. This short observation does not prove an app-level loading, visual
FPS or physics gain. After measurement, the explicit recording gate passes
15 checks and sandbox smoke passes 7. Screenshot review preserves scene
geometry and shadows. API Exit returns zero and the port closes. Runtime
data and verdicts are in `image-mips-decode-final-20261009.*`.

Only the new math owner was formatted once after validation. Subsequent
source changes are whitespace and a binder comment, with no behavior/input
change. Targeted builds, `git diff --check` and the skill catalogue pass.
The previously validated admission normal artifact is archived intact under
`/var/tmp/luncosim-optimization-perf-20261009/modelica-telemetry-admission-luncosim`.
An older task-owned archived profiler executable is losslessly gzip-compressed
to fit that archive, with recovered bytes verified before retiring its
uncompressed copy. The compression/archive manifests retain paths and hashes.
Only this task's production package outputs were cleaned between links;
cleanup preserved source and shared caches. No external library, authored USD
or authoritative simulation value changed in this step. No commit/push was made. Full loading,
visual-FPS, physics-tail and determinism acceptance remain open.

## Telemetry recording and admission — 2026-10-09

`lunco-modelica-telemetry` checks live history existence only when the scalar
catalog is at its configured limit. Below that limit it records directly and
reads `SignalRegistry::scalar_count` after acceptance, removing one repeated
path hash per variable without another cache or API. A full catalog still
admits existing channels, including after the configured limit is lowered;
removed history must pass admission again. The generic registry/producer seam
`channel_limit_uses_live_registry_admission_after_removal` passes (one test,
six filtered), including removal and zero-limit rejection. No public command
removes that registry history, so that lifecycle boundary is tested at the
resource seam. The production history gate remains authored in Rhai.

The matched admission Tracy capture is
`/var/tmp/luncosim-optimization-perf-20261009/modelica-telemetry-admission-tracy-after-20261009.tracy`,
327,897,859 bytes, SHA-256
`c3f72eadd837b19992026e5124cd8d472b01fe9c0defede8c4a524576c569e36`.
Profiler executable SHA is
`01b7e3bab72bee34c2d4b867a8a77fa46ebbb99f3bf76b796a505be17071d225`.
Owned PID 1061356/API 4159 and collector 1061352 verify the owned 8086 listener
and connection. The camera pose/projection, readiness, diagnostics, physics
progress/topology and screenshot shadow/geometry guards pass. Both processes
exit zero and the API port closes. In the same activation +3.3 through +9.8 s
windows, Modelica retention self-time p95 is 1.91/1.38/1.35 ms before and
1.93/1.22/1.26 ms after. Total owner time per 6.5 s window is
59.79/37.42/32.16 ms before and 58.19/30.20/30.68 ms after. Call counts differ,
so the lower per-call means alone cannot establish improvement. This single
pair does not prove consistent retention or app-throughput gain. Exact
sorted-index percentiles and wall-normalized totals are in
`target/perf/modelica-telemetry-admission-retention-pair-20261009.json`.

The normal admission artifact is archived at
`/var/tmp/luncosim-optimization-perf-20261009/modelica-telemetry-admission-luncosim`, SHA-256
`b6a168c2cdb7cc3b2c7664e89ed59696fa1dae8eeb196a1c65a98603c1facccd`.
Its owned PID 1072035/API 4160 measures High, 1280×720, no Vsync/throttle and
three six-second View/Builder/View windows with the same camera and runtime
guards. Retained 60 Hz raw app-frame observation p50 is
29.78/29.77/30.01 ms and p95 is 32.33/32.51/32.56 ms. The native 120-step
physics-ring p50 is 0.923/0.930/0.947 ms and p95 is 1.178/1.181/1.127 ms.
Readiness first clears at 2.89 s. These are short-run observations, not proof
of a loading, visual-FPS or physics-throughput gain. After measurement,
explicit 10 Hz/eight-sample recording passes the 15-check history gate and
seven-check sandbox smoke gate. API Exit returns zero and the port closes.
Data and verdicts are in `modelica-telemetry-admission-final-20261009.*`.

Filtered system self spans beginning in the capture's first four traced
seconds identify shader-raster preparation at 61.39 ms total,
with one 55.47 ms call. The later phase capture attributes the app-thread
spike to resident base copying; the owned-loading step above removes that
copy at the asset boundary. The export covers system spans, not constructors or the
loading critical path. The unfiltered attempt terminated with status 143
without usable output; the filtered export passes and retains 274,218 early
rows as owner aggregates in `modelica-telemetry-startup-attribution-20261009.json`.

Only the newly edited telemetry owner was formatted once after validation;
formatting changes whitespace only. The canonical telemetry contract and
profiling skill are current, `git diff --check` passes, and all 43 skills
validate. Eight task-owned JSON evidence files and a completed archived seam
executable are losslessly gzip-compressed, with recovered-byte hashes checked
before the originals were retired; the compression manifests retain both
paths and identities. An unreferenced incomplete `.mold` output from the
task's earlier disk-full link was retired with a hash manifest after verifying
no live producer/app. Only this task's production binary outputs were cleaned
for profiler/normal linking. Source and shared caches remain intact. No
external library, authoritative simulation value, actor ordering or clock
boundary changed in this step. No commit/push was made. Full loading,
visual-FPS and physics-tail acceptance remain open.

`lunco-signal::SignalRegistry` admits and appends a rate-paced sample through
one mutable history lookup. The public recording API and its consumers are
unchanged. The shared append owner applies backwards-time clearing, retention,
publisher reactivation and catalog notifications at the same accepted-sample
boundary. Invalid/not-due samples leave those facts untouched. Native f64 sample
values, timestamps, recording rate, simulation ordering and equations are
unchanged. The [canonical contract](../architecture/telemetry-subsystem.md)
records the ownership boundary.

Filtered CPU events from the admitted primitive-sharing Tracy capture identify
periodic recording batches: physics retention self-time p95 is
1.02/1.02/1.00 ms and Modelica retention p95 is 1.78/1.44/1.41 ms in its settled
View/Builder/View windows. Between-batch medians are much smaller. This is
instrumented owner attribution, not raw physics acceptance. The filtered export
and `target/perf/telemetry-retention-before-20261009.json` retain those windows.

The isolated registry workload uses 2,000 cached long signal identities and
200 accepted/rejected recording batches, checks each result and retained tail,
and runs seven repetitions per process in A/B/B/A order. Before medians are
88.75 and 63.69 ms; after medians are 51.30 and 50.61 ms. The warm comparison is
about 20% lower. Baseline drift and different dependency feature closures limit
this to the isolated owner workload; it does not establish production retention,
physics or frame throughput. Source, binary hashes and all timings are retained
in `target/perf/telemetry-recording-bench-*-20261009.*` and the retirement manifest.

The focused registry seam passes steady-value recording, rejected-sample
retention/activity isolation, backwards-time segments, owner preservation,
catalog revisions and invalid input rejection. The production build passes.
An owned fresh sandbox session, PID 1031772/API 4157, explicitly configures
10 Hz/eight-sample retention through `ControlTelemetry`. Its bounded exact-name
physics/Modelica history gate passes 15 checks, including steady values,
advancing/paced timestamps, retention and an absent-channel negative. Sandbox
smoke passes 7 checks. Readiness and retained runtime diagnostics pass; API Exit
returns zero and the port closes. These authoritative verdicts are in
`target/perf/telemetry-recording-final-gates-v2-20261009.{json,app.log}`.

The normal executable is archived at
`/var/tmp/luncosim-optimization-perf-20261009/telemetry-recording-luncosim`, SHA-256
`eda53523606b38f9621cf8cb4789cc2ee051b3d90a5ccb9570be4fc39f00a1a8`.
The prior mesh-complete artifact is
`0e3295fe99ebf7ad3cd74e30abe8c9641e2ebc63736804b16a75717d8c9fd50a`.
Owned PIDs 1003237/API 4156 and 1027667/API 4157 compare High, 1280×720,
no Vsync/throttle and three six-second windows. Both verify actual active-camera
pose/projection, readiness, diagnostics and physics progress/topology. Retained
60 Hz raw app-frame observation p50 is 30.18/30.28/30.53 ms before and
29.09/29.51/30.13 ms after; physics-service p50 is 0.947/0.942/0.919 ms before
and 0.945/0.966/0.938 ms after. Full readiness first clears at 2.50/2.47 s.
Window GPU clocks are 405/405/397 MHz before and 405/390/386 MHz after, with
about 20 W observed draw. This short pair does not establish loading, visual-FPS
or physics-step gain. The measurement JSONs retain failed post-measurement test
attempts; use the separate fresh gate file above for final verdicts. Summarized
artifact/source/session identity is in `telemetry-recording-result-20261009.json`.

Only this newly edited Rust owner was formatted after validation. Completed
task-owned seam/benchmark executables and the reproducible ordered-shadow CPU
CSV were retired to fit the production link; hashes/logs and original raw
captures remain. No external library changed, no simulation owner changed in
this step, and no commit/push was made.

The matched recording-only Tracy capture is
`/var/tmp/luncosim-optimization-perf-20261009/telemetry-recording-tracy-after-20261009.tracy`,
322,449,762 bytes, SHA-256
`b889b29542ef4713b56aeb2b0320e07ad8be9437019849cd44cd03e90eeb66ef`.
Its executable SHA is
`ef3e80294565cff5eb504627be80305dbf6748d6c9c4f2e0f61de399ea561e7e`.
Owned PID 1044338/API 4158 and collector 1044334 verify the owned 8086 listener
and connection. Actual active-camera pose/projection, readiness, diagnostics,
physics progress/topology and screenshot shadow/geometry checks pass. Both
processes exit zero and the API port closes. The filtered CPU export uses
perspective activation +3.3 through +9.8 seconds, matching the baseline rule;
Rhai print markers are not present in the baseline Tracy messages.
Physics retention p95 is 1.00/0.93/0.97 ms and Modelica retention p95 is
1.91/1.38/1.35 ms. Means and tails move in both directions; this does not
establish a consistent production retention improvement. Filtered exports and
`telemetry-retention-after-20261009.json` retain the attribution. The full
loading, visual-FPS and physics-tail targets remain open.

## Shared analytic primitive preparation — 2026-10-09

`lunco-usd-bevy-mesh::PrimitiveMeshAssets` now shares content-only CPU tasks
and immutable native mesh handles. Canonical f64 dimensions, axis and
primitive tessellation counts key the content; per-prim source admission
remains at the existing stage/path/generation/full-profile boundary. The cache
holds weak handles, releases retired readers/assets through events, and leaves
physics geometry and authoritative scheduling unchanged. Quality changes
replace shared assets; point instances follow their last inherited handle
after retessellation while retaining appearance and private edits. Horizon UV
installation and NURBS edits detach from shared assets before mutation. The
[canonical contract](../architecture/render-decoupling.md#immutable-primitive-mesh-assets)
records the owner and why existing terrain/globe/material caches do not cover it.

Three focused resource/component seams pass: shared work/cancellation/native
retirement and quality-key replacement; horizon UV isolation with native
shadow reception; and point-instance inheritance/private-edit retention.
Synthetic point-instance children lack a public entity identity, so their
binding is verified at the component seam. The authored production sandbox
sharing gate fails 36 of 42 checks on the archived pre-change executable and
passes all 42 on the new one, including unlike-dimension isolation. The
PointInstancer fixture passes 4 checks and sandbox smoke passes 7. Logs are
`target/perf/primitive-sharing-*-20261009.log` and matching `.app.log` files.
The mesh test build exposed a stale curve-test import already present in HEAD;
its import now names the current invalidation helper.

The unprofiled pair reduces distinct native mesh assets from 206 to 32 across
206 public mesh entities, an 84.5% reduction. Before SHA is
`6f2fabc94a4a2a17130f0b5e321e6e7548bef7240e87134963b3ea2d6572d26b`;
after SHA is `604b49767819c7dba49816b5ff7e0274126962ae64446f1f993f48785f7dcbe1`.
Owned PIDs 976154/API 4151 and 977516/API 4152 use High, 1280×720, no Vsync,
no throttle and View/Builder/View windows. Retained raw app-frame observation
p50 is 29.25/30.26/30.16 ms before and 30.64/31.07/31.43 ms after; physical
service p50 remains about 0.91–0.94 ms. Full readiness first clears at 2.03 s
before and 2.17 s after. GPU median clocks are 397/390 MHz under the 20 W cap.
This pair demonstrates asset reduction, not a loading or frame-time gain.
The frame channel samples at the fixed 60 Hz ceiling and may repeat a rendered
frame; the driver collects it during each window to avoid its 240-observation
retention truncating the window. The two runs verify camera roles/viewport but
do not retain pose/projection audits. Screenshots preserve framing, geometry
and shadows on visual inspection. Data, workloads and terminal results are in
`target/perf/primitive-sharing-{before,after}-full-window-20261009.json`.
The earlier `primitive-sharing-before-20261009.json` retains only four-second
history tails and is labelled accordingly.

The final normal mesh-only artifact passes a fresh 42-check sharing gate and
seven-check smoke gate on PID 1005715/API 4156. Its exact artifact and normal
window measurements are recorded in the telemetry section's baseline above
and `primitive-sharing-final-gates-20261009.{json,app.log}`. A post-measurement
2.32-second probe retains twenty snapshots of 206 native mesh transforms and
twelve spotlight transforms; every transform changes. An unchanged-world-pose
shadow cache therefore has no observed reuse opportunity in this scene. This
does not prove anything about an independently designed relative-frame cache.

The final scheduling change orders point-instance handle resolution after
primitive retessellation in the same frame. Its production Tracy build passes.
Owned PID 991254/API 4155 and the verified 8086 listener produce the intact raw
capture `/var/tmp/luncosim-optimization-perf-20261009/primitive-sharing-tracy-after-20261009.tracy`,
326,334,875 bytes, SHA-256
`60172695213c53a6eaf26da3757d07777e8384b4463fc9b5180aec18af4beac6`.
Executable SHA is `977a70387cf28b7e9f5665b65a8a010df33c657a9ef416194e16a21af654af9f`.
All three windows verify active-camera pose/projection, readiness, retained
diagnostics and physics progress/topology. Visual QA retains ground/object
shadows and geometry. Shared spotlight early spans total 17.78/17.96/18.18 ms
per camera event; directional early spans add 4.90/4.94/4.97 ms. All four shadow
groups are present, with roughly twelve shared views per camera event; total
shadow cost is 22.70/22.92/23.16 ms. The pre-change capture reports
22.82/23.09/23.64 ms. These clock-affected diagnostic differences do not
establish a shadow-time gain. All twelve settled headlights have positive
120,000 lm input and enabled shadow maps, so inactive lights do not explain
this workload. No native draw-count reduction is claimed from mesh IDs.

GPU/message CSVs and `primitive-sharing-tracy-summary-20261009.json` retain
this attribution. The 277 MiB derived CPU CSV filled disk space during the
summary write; it is retired because this GPU analysis does not consume it,
and the raw capture remains intact. The metadata update lost its final outer
workload row/completion fields. Original prefix/source hashes and 47 complete
rows are recovered; completion fields are verified from driver output, runtime
JSON and the raw capture. The partial file is retained separately and recovery
is explicitly marked in the `.meta.json`. The runtime JSON's independent
complete workload monitor remains available. Only completed task executables
and this reproducible CSV were removed; source and shared caches remain intact.
Owned ports 4148–4155 are closed. Loading, visual FPS, physics-tail and full
performance acceptance remain open. No external library changed.

## Shared-shadow caster inventory — 2026-10-09

A fresh owned unprofiled sandbox baseline uses the archived production
executable `6f2fabc94a4a2a17130f0b5e321e6e7548bef7240e87134963b3ea2d6572d26b`.
PID 917411/API 4144 passes View/Builder/View readiness, retained diagnostics,
physics progress, topology, and exact within-window `SceneCameraAudit` equality.
High quality, 1280×720, no Vsync and no throttle are unchanged. Sampled frame
p50 is 31.67/31.68/31.91 ms; physics-service p50 is 0.946/0.928/0.925 ms.
Continuous workload records identify the owned GPU process and a median GPU
clock of 397 MHz. These are short sampled observations under the existing
20 W cap, not a full-frame telemetry distribution or general FPS acceptance.
`target/perf/empty-shadow-baseline-20261009.json` contains the camera audits,
workloads, executable hash and terminal session result.

The empty-depth-map reuse experiment reports zero settled reused attachments
and zero empty clears, with twelve shared views taking the native path.
Its three windows pass the same runtime checks. Their frame p50 values
30.27/31.50/30.97 ms do not demonstrate a cache benefit because no map was
reused. The experiment's generic submission/invalidation seam passes, but its
code is removed. The result and raw counters remain in
`target/perf/empty-shadow-result-20261009.json` and the matching `after` app log.
No speed gain is claimed from this experiment.

An owned settled caster probe, PID 926412/API 4146, reads native main-world
visibility lists and mesh data once. All twelve shared views are rover
headlight **spotlights**, with 4–73 candidate meshes per light,
1,056–51,309 vertices and 3,090–211,380 indices. Every view includes the
four-vertex ground plane. Sky and globe meshes are absent. These counts are
CPU visibility candidates, not post-GPU-culling draw counts or per-mesh GPU
timings. The canonical inventory and its source log are
`target/perf/shadow-caster-inventory-20261009.json` and
`target/perf/shadow-caster-probe-all-local-20261009.app.log`. Three bounded
perspective windows pass readiness, diagnostics, physics progress/topology,
and camera stability. The probe is removed and both touched render sources
match their pre-probe hashes exactly. Owned ports 4144–4146 are closed.

The restored production build passes. Its exact executable SHA-256 is
`12b18c84ad138a61cb55ca8b38dcac56b95ff234444ddfc845671a47ffc3fd50`.
An owned run of that artifact, PID 930644/API 4147, passes all three bounded
perspective windows, readiness, diagnostics, physics progress/topology, and
within-window camera equality. Ground and object shadows match the baseline
on visual inspection. Frame p50 is 30.15/30.98/31.40 ms; these unchanged-source
observations also show why small differences cannot establish an optimization
gain. The process exits zero and port 4147 closes. Build/source-restoration
and exact-artifact runtime evidence are recorded in
`target/perf/shadow-investigation-restored-20261009.json` and its runtime file.

The caster inventory identifies duplicate primitive mesh identity as a concrete
application-owned preparation/upload issue. Its implemented ownership contract,
verification and measured limits are recorded above. The twelve populated
spotlight shadow passes remain the principal measured GPU cost.

## Shadow GPU attribution — 2026-10-09

`lunco-render-bevy` composes native shadow systems between begin/end timestamps
through Bevy's existing render diagnostics recorder. Installation occurs only
for diagnostics-enabled render hosts, after native plugin registration. The
native delegate retains its implicit dependency set; PBR's system-local
before/after rules are reapplied explicitly. Begin validates every native
parameter before opening a span, and the read-only pipe keeps begin/native/end
on one CPU thread and queues their render contexts in that order. Ordinary
builds retain the native systems. No external source, simulation schedule,
clock, equation, precision, shadow draw, map resolution or quality setting
changes.

The focused schedule seam passes. It covers dependencies targeting the native
set and those owned by the native system, a single delegate call, paired-thread
execution, deferred buffer order, and nonmatching-view admission without an
orphan span. The production Tracy build passes. The owned corrected capture
verifies PID 886934, API 4142 and the matching 8086 profiler connection. View,
Builder and View pass readiness and physics-progress/topology checks. Ground
and object shadows, geometry and framing match the baseline screenshot on
inspection; this is visual QA, not a pixel equality assertion.

The corrected GPU capture measures 22.82/23.09/23.64 ms of total shadow work per
camera event. Shared point/spot early passes account for 17.79/18.06/18.53 ms
and directional camera passes for 5.01/5.02/5.09 ms. Late spans contribute only
0.0167/0.0171/0.0165 ms in this fixture. Opaque-pass means are 2.50/2.50/2.59 ms.
All four groups are present, with twelve shared views per camera event apart
from window-edge partial frames. This identifies shadow work as the principal
GPU lead in the sandbox. GPU samples report roughly 98–99% utilization at
382–450 MHz and 20 W with the software power cap active; the profiler consumes
CPU. These are attribution results, not product FPS or uncontended hardware
acceptance, and they establish no new speed gain.

The canonical corrected evidence is
`target/perf/shadow-diagnostics-ordered-tracy-summary-20261009.json` plus its
`after-20261009.meta.json`, screenshot, raw capture and workload samples.
Tracy executable SHA-256 is
`f001e2b49ce8f85d5facd0333739b1ed76f205af9d1404f7bac3d5c438ec0da2`;
capture SHA-256 is
`f169f3ebf7719a89d25beee054d68398ddaecb22aac46ffeecc73d1eb8ec174e`.
Only evidence whose metadata admits it after visual QA may inform attribution.
The ordinary production build passes after focused production-package cleanup
resolved a disk-full link. An owned unprofiled session, PID 891558/API 4143,
passes the same three readiness, physics-progress and topology windows; its
screenshot preserves shadows. Sampled frame p50 is 30.78/31.67/31.44 ms and
physics-service p50 is 0.924/0.935/0.931 ms. These are short observations without
a continuous workload or exact-pose audit, not paired FPS acceptance. Process
health appears at 0.773 s and clear readiness at 2.426 s; this is a warm sandbox
launch, not full Twin loading acceptance.

The current unprofiled executable SHA-256 is `6f2fabc94a4a2a17130f0b5e321e6e7548bef7240e87134963b3ea2d6572d26b`.
Both current executables and the passing generic seam-test binary are archived
with verified hashes. Superseded task-owned executable copies were released;
all raw captures and measurements remain. Source, Cargo registry and shared
sccache were preserved. Owned API ports 4141–4143 and the profiler listener are
closed. Changes remain uncommitted and unpushed.

The deterministic replay matrix recorded below remains valid for the unchanged
simulation owners. New loading, visual FPS and raw physics-tail acceptance
remain open. The next rendering investigation must distinguish useful shadow
draws from avoidable preparation or empty-view work while preserving image
quality and the native dependency contract.

## Change-aware structural fingerprint publication — 2026-10-09

`PortTopologyState::observe_if_changed` extends the existing typed structural
state owner. First observation derives a key; subsequent sample-only changes
reuse auxiliary declaration and Modelica unit fingerprints. Their own component
changes derive fresh keys. Newly admitted `SimComponent` owners refresh both
facts, including edits made while that participant was absent. Removal observers
retire each component's key. The composed fingerprint and publication boundary
are unchanged; direct backend inspection and writes still derive the live
contract independently of the publication pass. No policy, hook, scheduler,
equation, timestep, substep, precision or external-library source changed.

The focused generic resource tests pass 2/2. They cover cold observation,
unchanged-owner derivation suppression, unit edits, same-size declaration
replacement, sample-only changes, removal and re-admission. Direct backend keys
change before structural publication when the live unit is edited. The owned
Modelica AST dev dependency supplies the canonical metadata fixture type; it
adds no production dependency or alternate parser. The complete production
matrix matches unchanged main exactly: ten profiles, 1,120 state rows, all 438
Modelica variables per participant and 360 authored Rhai checks including the
deliberate mismatch. Temporary source instrumentation is restored and the old
tracked reference is unchanged.

Settled Tracy checker means fell from 0.244/0.204/0.199 ms to
0.0164/0.0138/0.0140 ms per View/Builder/View frame, approximately 93% lower.
Output publication remains 0.102/0.091/0.090 ms per fixed tick. The listener and
connection are verified against owned PID 828341, API 4140. The screenshot
retains scene geometry and shadows. GPU samples remain approximately 99%
utilization at 375–405 MHz and 20 W, with the software power cap active. These
are attribution results; profiler CPU/GPU overhead and different device clocks
prevent treating the instrumented frame distribution as product acceptance.

The unprofiled headless comparison reuses the unchanged prior artifact's valid
sandbox run. Realtime cadence remains near 60 steps/s. Maximum-speed throughput
changes from 303.32 to 307.95 steps/s, which does not establish a reliable gain.
Realtime fixed-tick p50 is 1.592/1.633 ms versus 1.570/1.587 ms previously; this
short pair shows no fixed-tick improvement. The optimized owner runs in
`PostUpdate`, outside that metric. New clean visual-FPS, loading and raw
physics-tail acceptance remain open.

Evidence is retained in `target/perf/structural-observation-result-20261009.json`,
`structural-observation-replay-matrix-result-20261009.json`,
`structural-observation-tracy-summary-20261009.json` and the linked raw files.
Production and Tracy builds pass. Touched Rust files were formatted once after
validation; diff and skill-catalogue checks pass. Disk-full interrupted the
initial test build;
all 272 performance-evidence entries were verified before and after checkout
build-output cleanup, and the successful focused run followed that clean.
Source, registry and shared sccache data were preserved. Superseded task-owned
executable archives were released; their recorded hashes and measurements
remain in the retained evidence. The measured unprofiled executable had
SHA-256 `bd450f4a856a62ad0123cddc7485d8179c44d6040636d60a3e1b448585d1198b`;
its superseded archive was released after the current artifacts above passed.
Its raw measurements and hash remain in the evidence.
Owned API ports 4138–4140 and the profiler listener are closed. Changes remain
uncommitted and unpushed.

Shadow GPU attribution is recorded above. Modelica telemetry sampling spikes
remain a separate measured CPU lead. Full loading, visual and physics
requirements are not yet achieved.

## Validated slot-backed snapshot publication — 2026-10-09

`lunco-port-core::ScalarPortMap::upsert_samples` retains destination slot hints
for borrowed named snapshots. Each reuse checks the live layout and exact name;
source order or cardinality does not establish identity. Clone discards hints,
and clear/compaction retire them through the existing slot owner. Modelica and
scripted publication consume this mechanism at their existing fixed-step
boundary. Modelica's reflected map, native `f64` samples, input/output ordering,
equations, timestep, substeps and external-library sources remain unchanged.
This is generic container work, with no new scheduling, policy or hook.

The unprofiled sandbox pair reduced realtime fixed-tick service p50 from
1.715/1.700 ms to 1.570/1.587 ms (8.45%/6.67%). Realtime cadence remains near
60 steps/s. Maximum-speed fixed-tick p50 fell from 1.076 to 1.041 ms; throughput
changed from 300.27 to 303.32 steps/s, too small to establish a robust throughput
gain. Ordinary desktop CPU activity was not continuously audited during this
pair. These are short headless observations, not visual or loading acceptance.
The exact before/after hashes and complete diagnostics are retained in
`target/perf/sample-copy-result-20261009.json` and `sample-copy-{before,after}-20261009.json`.

A separate owned Tracy capture measures output-publication means of
0.108/0.100/0.095 ms in settled View/Builder/View windows, compared with
0.281/0.267/0.254 ms in the earlier diagnostic capture. Propagation remains
0.155/0.146/0.144 ms. The capture verifies the profiler listener and connection
against PID 701374 on API 4137. GPU samples report approximately 99% utilization,
360–382 MHz and 20 W, with the software power cap active; only the owned app
appears in the sampled GPU process inventory. The profiler itself consumes CPU.
Exported opaque-pass medians of 2.818/2.818/2.941 ms cover that pass, not the full
GPU frame; the exported GPU inventory contains no shadow-pass timings. Do not
sum nested spans or treat instrumented frame/physics numbers as acceptance.
Raw capture, workload audit and window statistics are retained through
`target/perf/sample-copy-tracy-after-20261009.meta.json` and `sample-copy-tracy-summary-20261009.json`.

The complete maintained replay matrix matches unchanged main `28c56ac25` exactly:
all ten profiles and 1,120 physics/Modelica/articulated rows, with all 438 Modelica
variables per participant. Production Rhai passes 24/36/72 checks for the
4/8/20-rover scenes respectively, including the deliberate-mismatch negative
control. Five passing profiles were retained after a disk-full capture
interruption; only the interrupted profile and four remaining jitter profiles
were resumed. Temporary row logging is restored byte-for-byte and the older
tracked reference is unchanged. `sample-copy-replay-matrix-result-20261009.json`
records the complete evidence. This is same-machine maintained-matrix evidence,
not cross-machine or whole-session replay acceptance.

The generic slot-copy contract test passes, including source reordering,
same-size replacements, destination removal/reinsertion, clone, clear,
compaction, signed zero and full-precision values. Production and Tracy builds
pass. The two touched Rust files were formatted once after validation; diff
and skill-catalogue checks pass. The inspected sandbox screenshot retains
visible scene geometry and shadows. The measured unprofiled executable is
restored at `target/debug/luncosim`
with SHA-256 `49d6fde6204f4f9c9dda627bd88dc6dbffcca436ad2c565f006f2cd075a4a2f7`.
Owned API ports 4134–4137 and the Tracy listener on 8086 are closed. Changes remain
uncommitted and unpushed. Loading, visual FPS and raw physics-tail targets remain
open. The port structural checker costs 0.244/0.204/0.199 ms per settled
render frame. Its source recomputes unchanged signal-unit metadata while
processing changed sample components. Modelica telemetry retention also has
sampling spikes; its system body and command flush are exported separately.
The change-aware publication section above records the checker optimization
and its live-edit verification.

## Settled Apollo quality and complete current-main replay — 2026-10-09

`TerrainLodStatus.derived` now exposes the existing terrain owner's active,
ready, total and pending map-preparation facts. The query is read-only and
fails visibly if its required visual owner is unavailable; no new simulation
hold, worker, policy or registry was added. Performance and inspection guidance
uses this state alongside selected-cover fulfilment. Surface camera audits use
existing `world_pos`/`world_rotation` reads in the active simulation frame:
composed root coordinates move with celestial ancestors and cannot establish
that a camera is stationary over the terrain.

The owned High 1280×720 Apollo View/Builder/View run kept 256/256 selected tiles
resident, zero pending stream work and one ready derived-map product throughout
all three 15-second windows. Fifteen checkpoints per window verified terrain,
quality, viewport identity and the exact active-frame camera pose. The inspected
screenshot retains visible lunar terrain, rocks, route labels and shadows.
Frame p50 is 4.701/5.343/4.648 ms; p95 is 8.485/9.275/8.796 ms. Full physics-step
p50 is 0.439/0.422/0.438 ms and p95 is 0.577/0.572/0.615 ms, at 60.158/60.110/
60.210 steps per wall second. API health appeared at process-clock 0.648 s,
clear simulation readiness at 1.938 s, and the first observed fully prepared
terrain at 3.002 s. These warm process-clock observations do not certify the
separate app/Twin startup targets or a before/after loading improvement.

Concurrent workload sampling observed only the owned process on the NVIDIA
GPU; ordinary desktop CPU activity remained present. GPU utilization was
33–52%, clocks 1170–1312 MHz and draw 19.47–19.86 W, with the software power cap
active throughout. This is audited product timing under that device constraint,
not an unconstrained GPU benchmark. Engine revisions were sampled at 20 Hz;
the frame distribution does not observe every rendered frame. The measured
process was PID 666570 on API 4132, with exact artifact
`598df781dea0b34249c3676e13268615af9e9c6ae032941a65c21a854cf9ed1c`.
Raw data, screenshot and summary are in `target/perf/apollo-settled-active-frame-20261009.{json,png}`
and `settled-terrain-quality-result-20261009.json`.

The complete maintained ten-profile replay comparison against fresh unchanged
main `28c56ac25` is exact on this machine: 4/8/20 rovers at serial/default Compute
widths, plus 0.25/0.5 jitter at both maintained seeds. All 1,120 selected and final
physics/Modelica/articulated rows match, every participant retains all 438
Modelica variables, and production Rhai passes 24/36/72 checks for the 4/8/20-rover scenes
respectively, including the deliberate-mismatch negative control. The prior unchanged four-rover serial
comparison is reused; the other nine profiles were captured from the archived
unchanged-main artifact and compared through the production reference query.
Serial/default trajectories also match exactly within both artifacts for each
rover count. No tolerance, equation, timestep, substep or external-library source
changed. This establishes current-main trajectory preservation for the maintained
matrix on this machine; cross-machine and whole-session replay remain separate
requirements. The older checked-in reference is unchanged and still differs
from unchanged main; the current-main diagnostic reference lives only in
`target/perf/full-current-main-trajectory-reference-20261009.json`.

`full-main-replay-matrix-result-20261009.json` records commands, PIDs, hashes,
all profile comparisons and width comparisons. Temporary row logging was
restored byte-for-byte, and the old tracked reference hash remains
`4f1785e86deb5e05083561007ec8e08891ca5a0d19c82e9edcdb8f70c92e3496`.
The production build passes; the touched terrain query was formatted once after
runtime verification, and diff checks pass. The skill catalogue check passes.
All owned API ports 4130–4133 are closed, and no task runtime is left running.
Changes remain uncommitted and unpushed. Settled visual and physics tails still
miss the stated performance targets; loading and sandbox visual acceptance remain
open. The slot-backed snapshot-publication section above records the subsequent
owner optimization and its separate verification evidence.


## Apollo telemetry admission and current-main replay — 2026-10-09

The USD telemetry projector now reuses the composed network-membership cache
to admit a member's self-targeted declaration while its generated surface is
pending. The existing alias map binds the sampled output to its generated
wrapper. A metadata Scope without a surface still requires an explicit target.
The exact Apollo scene now reports zero retained runtime errors or warnings.
Its production Rhai ownership check passes 6/6, including one live solar channel
owned by the electrical wrapper, authored units and unknown-name rejection.
The document API authored, saved and reopened the negative fixture; the
production `telemetry_member_target_negative` gate passes 5/5, including a
target diagnostic, no retained invalid channel, and malformed-filter rejection.

`ListTelemetryChannels` now accepts an optional exact `name`, validated before
reading the registry and filtered before metadata materialization. Apollo's
full catalog contains 1,321 channels; a targeted read retrieves its one channel
without transporting the full dictionary into Rhai. The complete catalog stays
available when `name` is omitted. The retained signal registry remains the
single catalog owner, with no new index or presentation policy.

The owned High-quality 1280×720 Apollo View/Builder/View run measures frame
p50 4.546/5.418/4.662 ms and p95 8.005/8.969/8.208 ms. Full physics-step p50 is
0.442/0.419/0.439 ms and p95 0.630/0.549/0.628 ms. Process-clock clear readiness
was 2.492 s; API health appeared at 0.892 s. These are observations, not a
before/after speedup or acceptance of the separate app/Twin startup clocks.
The inspected screenshot shows the terrain, rocks and route with shadows;
terrain-derived completion and concurrent workloads were not audited during
the windows, so this is not settled visual-quality acceptance. The exact
measured artifact is `25fa4a8d696913006a06a9a1c68f027be9f330b8f135c7b3f3200e472bada18c`,
retained under `/var/tmp/luncosim-optimization-perf-20261009/telemetry-member-luncosim`.

A fresh unchanged local-main `28c56ac25` build supplies the complete current
four-rover serial trajectory. The optimized production artifact matches every
one of its 56 physics/Modelica/articulated rows through tick 780, retains all
438 Modelica variables and passes all 24 authored checks, including deliberate
mismatch rejection. This covers the admission-order, port-discovery and buffer
changes for this profile. Other thread-width, rover-count and jitter profiles
remain required. The older checked-in reference is unchanged; its physics and
later Modelica states also differ from current main, beyond the four added
guidance variables, so refreshing only the count would not establish replay.

Evidence is in `target/perf/main-trajectory-determinism-result-20261009.json`,
`telemetry-member-verify-20261009.json`, `telemetry-member-negative-20261009.{json,log}`
and `apollo-telemetry-after-20261009.{json,png}`. The unchanged main artifact is
`9c8c5560d1c89b004ee6176353449dd51887dd110fb645829c995dd262668ce3`.
All 31 task-file hashes and the tracked diff were verified after restoring the
scoped backup stash. The verified optimized executable is restored at
`target/debug/luncosim` with hash
`bbdd2420b536afd8b0f72e6834db0eddf83bea1153e9579fbe7bd4d4880add2d`.
All task API ports are closed. No external-library sources, numerical equations,
timesteps, physics substeps, quality settings or determinism tolerances changed.
Changes remain uncommitted and unpushed; loading, sandbox visual FPS, physics
tails and full replay acceptance remain open.

## Worker response ownership and current trajectory comparison — 2026-10-09

`lunco-modelica-worker::worker::bridge` now transfers validated in-flight input
samples into the accepted-step record and moves response samples into the
bounded UI queue. Existing observable-variable names reuse their map keys.
The accepted record still owns a separate output snapshot. Session/step/endpoint
validation, detected-symbol/output precedence, UI sample order and `f64` values
are unchanged; this is buffer ownership within the existing admission mechanism,
with no policy hook, scheduling or equation change.

The nine worker bridge lifecycle tests pass, including stale-session rejection,
exact signed-zero/full-precision values, input/output buffer transfer and reset
retirement. The production build passes, and Earth-balloon passes 3/3 at tick
720. Raw results are in `target/perf/response-buffers-{tests,build}-20261009.log`
and `response-buffers-earth-20261009.{json,log}`.

The four-rover serial production comparison captures all 56 existing exact-state
rows: physics at ticks 0/1/180/360/540/720, Modelica at 0/1/180 and final tick
780, articulated bodies at 11/80 and final tick 780, plus final physics. All 438
Modelica variables per participant are retained. The pre-port-change artifact,
post-port-change artifact and current buffer-change artifact match every row;
the latter two pass all 24 scenario checks, including the deliberate mismatch
negative case. The diagnostic reference lives only in `target/perf`; the
checked-in reference remains unchanged and still rejects the current 438-variable
model because it expects 434. Temporary Rhai row logging was restored after
capture. `response-buffers-determinism-result-20261009.json` records exact binary,
source and reference hashes. This proves this serial four-rover trajectory across
the port/buffer changes, not the complete multi-profile determinism matrix or
the earlier admission change against unchanged main.

The unprofiled sandbox still sustains 60 steps/s in realtime. Fixed-tick medians
are 1.692/1.748 ms versus the prior 1.694/1.695 ms; max-speed capacity is 289.93
versus 297.19 steps/s. This run does not establish a throughput improvement from
buffer transfer. It removes demonstrated copies, while output publication remains
the larger measured fixed-step owner. The current production artifact is
`0e35a5d19a5abf5d0ebd4626cb9d919e694c9d646c1fa2bbce9be1a6c8c8f872`.
The measured sessions on API ports 4123/4127/4128 exited. Loading, visual FPS,
raw physics cost, full replay and Apollo measurement admission remain open.

## Selective live port declaration discovery — 2026-10-09

`lunco-port-core` owns `PortDeclarationQuery`, consumed by every backend's
`declare_ports` callback. Inspection collects the complete declaration rows in
their existing order. A named query borrows the exact name and causality side,
retaining only its first matching direction; map-backed owners use their name
index directly. The fixed propagation consumer no longer builds and copies the
owner's complete surface to validate one resolved target. Live owner precedence,
metadata, writability, bounds, revision and slot validation still run before the
exclusive commit. This is a generic discovery mechanism, with no new policy
hook, scheduler, cache or numerical-state change.

The owned unprofiled sandbox pair used the same scene and topology. Realtime
fixed-tick service medians fell from 2.222/2.272 ms to 1.694/1.695 ms. Max-speed
capacity rose from 271.04 to 297.19 steps/s; its fixed-tick median fell from
1.519 to 1.084 ms. Realtime cadence remained near 60 steps/s. The exact before
and after binary hashes, commands and raw samples are in
`target/perf/ports-result-20261009.json` and `ports-{before,after}-20261009.json`.

A separate owned Tracy capture measured propagation means of
0.143/0.140/0.137 ms in settled View/Builder/View windows. The earlier diagnostic
capture measured 0.626/0.645/0.620 ms. These profiler captures have different
cadence and are attribution evidence; the unprofiled pair establishes the
fixed-tick service saving. Modelica output publication now costs approximately
0.25–0.28 ms per tick in this capture, exceeding propagation.

The port-core resource/registry seam suite passes 20 tests, including exact-name
and direction selection, owner precedence, invalid values, atomic rejection,
and stale/removed locators. Production Rhai passes the Earth-balloon propagation
gate 3/3 at 720 ticks and the negative duplicate-owner gate 11/11. The production
and Tracy builds pass without new warnings. Format and diff review are complete.

High-quality 1280×720 rendering still measures roughly 30–31 ms frame medians;
full Avian step medians remain approximately 0.91–0.93 ms. The GPU remains near
98–99% utilization with its software power cap active at about 20 W. Screenshots
retain the same visible scene and shadows, and camera inventories were recorded;
no exact camera-pose audit was performed. Warm clear-readiness observations were
3.046 s before and 2.138 s after on the process clock, but this pair does not
certify the separate full app/Twin readiness targets or a cold-loading saving.
The existing exact multi-rover replay reference mismatch remains open. No
external libraries, equations, timestep, substeps, visual quality, f64 state,
authoritative ordering or commit boundaries changed.

The current unprofiled executable is restored to the measured after artifact
(`73ae720a788f2027cbc62df859a5ffa9b36dc3f2bd418e55b4526656df75806e`).
All owned API ports 4122–4129 and the profiler listener on 8086 are closed.
Changes remain uncommitted and unpushed. The next measurement scope is Modelica
output publication and response/retention work; full Apollo performance also
needs the generated-domain telemetry target admission issue resolved at its
USD co-simulation owner. Loading, visual FPS, raw physics-cost and exact replay
acceptance remain open.

## Causal completion admission — 2026-10-09

The Modelica execution host admits worker completions in `First`, after message
rotation and before `ClockProjectionSet` samples the causal hold. Compile intent
stays in the `Update` lifecycle cycle. The worker keeps its existing session,
source, step, communication-endpoint and all-participant checks; fixed-clock
overstep, timestep, solver settings and physics equations are unchanged. This
placement is an engine admission invariant rather than a changeable Rhai policy.

An owned unprofiled A/B from `28c56ac25` measured the same sandbox topology
(50 bodies, 33 colliders, 20 joints). Headless realtime cadence increased from
51.4–51.5 to 60.1–60.2 steps/s in the two measured windows. Median fixed-tick
work remained approximately 2.2 ms. Max-speed capacity was 270 versus 263
steps/s; this diagnostic does not establish a raw solver or physics-cost gain.
Rendered High-quality, 1280×720 View/Builder windows increased from roughly
46 to 60 physics steps/s. Median frame time remained around 30–31 ms, and
median full Avian step cost remained around 0.92 ms. The GPU was software
power capped at about 20 W. Visual FPS and the 0.5 ms physics target remain open.

The eight focused worker bridge tests and the execution-host ordering test
pass, including stale-session rejection before clock projection. The production
Earth-balloon Rhai gate passes 3/3 checks at 720 ticks. Its declaration and the
Python-chain test declaration now name the entities they read. The chain reaches
a failure verdict in this default build, which does not enable its optional
Python backend; it is not admission validation evidence. Exact multi-rover replay
already fails on unchanged main because the reference expects 434 Modelica
variables while the current model exposes 438. The reference is unchanged;
full replay acceptance remains required.

Raw evidence is under `target/perf/solver-handoff-{main,after}-20261009.json`,
`target/perf/sandbox-{main-28c56ac25,admission-after}-20261009.json`, and the
`admission-*-20261009.log` files. All owned ports 4122–4129 are closed.
These runs establish a cadence improvement, not completion of the loading,
visual FPS, raw physics-cost or determinism objectives. The selective declaration
discovery section above records the subsequent port-propagation improvement.

## Griffin compile and load measurements — 2026-10-07

Prepared-solver reuse now keys the successfully seated strict source closure, with generated runtime identities normalized and participating library bytes exact. An unrelated authored overlay no longer invalidates this solver cache. Native logs distinguish memory hits, disk hits and lowering misses.

In the owned Griffin runs, the identical FLIP source key `0bfee35bccb31102` lowered in 4.373 s on a miss and loaded from disk in 13.4 ms on a subsequent process. The warm full visual scene materialized in 5.7 s; individual prepared-solver disk lookups were approximately 12–26 ms. Cold AttitudePropulsion lowering took 105.6 s during a concurrent focused Cargo check, so that value is diagnostic rather than uncontended acceptance. Compilation itself was about 0.1–0.2 s per generated model; cold solver lowering remains the dominant preparation cost. The API responded during the preparation hold, and simulation remained at tick zero. These measurements do not establish sustained FPS or faster cold lowering. Further optimization should profile the lowering owner and measure a clean cold/warm pair, preserving deterministic admission and the actual model equations.


> Status: Open · Worktree: optimization · Scene: Summer Space School Apollo

## Objective

Current acceptance is at least 150 FPS in Summer Space School and sandbox at
High visual quality, with the full Avian physics step below 0.5 ms per fixed
step. Use `PhysicsTotalDiagnostics.step_time`, surfaced as
`engine-health.physics_step_ms` and the recent one-per-step history returned by
`PhysicsPerformance.step_time_samples_ms`, as the acceptance metric; record its
p50/p95/p99/max and use Tracy subzones for attribution. Measure three startup
milestones separately: the app becomes ready in less than 2 s of process launch;
each Twin scene mounts in less than 2 s of the accepted Twin-open request; each
Twin reaches full readiness within 2 s of that request. App and Twin readiness
use separate clocks and separate evidence. App readiness has a 2 s process
clock; full Twin readiness has a 2 s Twin-open clock. Scene mount is a separate
2 s Twin-open checkpoint.

For an auto-opened Twin, use the `StartupScene opened` event as the Twin-open
origin. Preserve authored visuals, deterministic physics cadence, BigSpace
frame semantics, and responsive UI; do not meet these targets by reducing
quality, dropping authoritative ticks, or changing simulation semantics.
Use process launch as the app clock and Twin-open as the Twin clock.

For measurement, app-ready requires both the window-created event and a
successful `/api/health` response within 2 s of process launch; `/api/health`
by itself proves liveness.
Twin scene-mounted requires the live canonical stage and mounted scene root.
Full Twin-ready requires the scene load to finish, the scene-participant
lifecycle-ready event, and completion of every startup readiness producer's
initial registration pass. At that point, all active readiness items must drain
with `readiness_tracked=true`, `ready=true`, `world_hold=false`, `faulted=false`,
and `pending_count=0`. Poll readiness during startup; count the first sample
after the admission pass that satisfies all these conditions as the transition
only if the state remains clear for the next five seconds. The first green
`/api/ready` sample alone is not sufficient: current startup can register
scene, physics, and Modelica waits after an earlier green response. The
five-second soak validates the transition; report the elapsed target from the
first qualifying sample, not from the end of the soak. If a later wait appears,
reject the earlier sample and measure again after that wait clears.
Historical entries that record `/api/ready` samples describe that API response
only unless they track every startup producer through completion and verify no
later regression.

## Current implementation

### 2026-10-07 — integration and evidence refresh

Optimization was fast-forwarded to local main `7a20f4f8c` before the saved
changes were reapplied. The integrated tree includes `af6e0f7bf` and the
current pressure-fed propulsion models. The old Griffin failure below needs
a new run against this solver; it is neither a current failure verdict nor
evidence that the solver update fixes it.

The focused Modelica worker, SysML UI, USD domain and scripting test-target
compile check passed. Integration updated the worker test to the pinned
document runtime owner and added the current optional member-path input to
eight inline hook-synthesizer contexts. Main's typed bounded persistent-cache
rejections remain authoritative.

A measured egui layout regression showed that an expanded header allocated
24 pixels against a 21-pixel virtualized row stride. Both virtualized tree
consumers now use the shared header-only disclosure path. Its layout seam
passes for open and closed branches. The editor diagnostic now records orbit
yaw/pitch and rejects changed viewport geometry, late readiness waits, runtime
faults, topology changes and physics stalls between one-second checkpoints.
Four negative Python guard tests pass. The source-isolation Rhai gate has a
checked-in production invocation supplying its exact source and selection
parameters through `RunScenarioAsset`.

The merged production binary passed sandbox smoke (7/7) on owned API 49128.
Source-isolation gates passed (8/8 each) on owned API 49127 for both an already
open source and `assets/vessels/rovers/skid_rover.usda` outside the running
Twin's folder. The new-source screenshot shows the rendered rover and the
selected `/SkidRover/Motor_RR` row revealed through its hierarchy. These
sessions exited through their API and released their ports. Logs and exact
screenshot metadata are under `target/perf/*merge-20261007*`.

The first longer paired Editor diagnostic on API 49129 preserved camera,
viewport geometry, Twin, preview population and physics topology, advancing
219–233 physics steps per four-second window. Sampled frame medians were
7.27–11.35 ms; recent physics steps were approximately 1–3 ms. Other checkout
builds and a simulator overlapped this run. It also exposed
`telemetry-event-overflow`: the no-actor driver returned before draining its
neutral inbox. That run is excluded from acceptance. The scripting owner now
discards unobservable idle traffic, preserves another language's pending
consumers, and retains overflow fault semantics and sequence numbering. The
idle-drain, overflow-diagnostic, reverse-completion and pre-start event seams
pass. The diagnostic driver checks `RuntimeDiagnostics` at each measurement
checkpoint so a nonterminal event-delivery hold invalidates the window even
when physics continues and readiness stays green.

The post-fix paired run waited 90 seconds before measuring and completed all
eight windows with zero retained runtime errors at every checkpoint. Camera,
geometry, Twin and topology checks stayed stable and each window advanced
72–189 physics steps. Logs, samples, screenshots and 29 process-workload
snapshots were stored under `target/perf/*final-20261007*`; those artifacts
are absent after this checkout's target cleanup. Concurrent builds
substantially affected this run (sampled frame medians 22.89–98.18 ms); it proves
healthy idle event admission and guarded editor operation, not FPS or physics
acceptance. The regenerated merged runtime command reference is unchanged at
262 commands. The post-fix production source-isolation gate also passed 8/8
with diagnostic findings included in its assertions; its screenshot reveals
`/SkidRover/Motor_RR` in the virtualized tree. That owned API 49127 session
exited and released the port. That evidence was stored in
`target/perf/source-isolation-final-pass-20261007.*` and is now absent.

Declaration-scoped Modelica initialization is fixed in the compiler's DAE phase
and editor compile dispatch. The production scoped-defaults gate passes 20 checks
including integration, override, reset, expressions, arrays, and batch execution.
The bounded post-merge Griffin
probe remained held during cold preparation; earlier landing and engine-command
verifications timed out without verdicts. Twin acceptance remains open.

Earlier dated measurements are historical diagnostics. Their raw captures,
logs and screenshots are absent from this checkout's current `target/`, so
they cannot be re-inspected here. Uncontended FPS/physics and startup
acceptance remain required; no historical number is promoted to current
acceptance by this merge.

### 2026-10-05 — bounded editor and live-exchange work

Four application-owned changes preserve the existing mechanisms and policy:

- `lunco-sysml-ui` compares the active Twin identity, root, manifest, and file
  index before treating a workspace notification as projection invalidation.
  SysML source, analysis, and evidence changes still invalidate the view.
- `lunco-doc-bevy` checks every exact file origin before resolving aliases;
  SysML source links use that canonical registry lookup. There is no parallel
  path index, and document origins remain authoritative after rename/close.
- `lunco-scripting` borrows unchanged script documents during preparation and
  copies worker inputs only for admitted compile/retry work. Eligibility,
  submission order, result admission, and lifecycle commits are unchanged.
- `lunco-port-core` prepares a resolved input write once and commits within
  that exclusive World boundary. It avoids a second live validation and a
  single-element batch allocation. Validation ordering, rejected-write results,
  f64 values, precedence, stale handles, and batch atomicity are retained.

The named SysML workspace-focus, document-origin lookup, resolved-input safety,
and reverse-worker-completion lifecycle seam tests pass. The last test forces
`beta` to finish before `alpha` and verifies ordered `alpha, beta` commits.
These are focused mechanism checks, not whole-runtime determinism acceptance.

Owned High-quality Tracy sessions on API `4101` exercised Summer Space School's
`traverse_apollo15.usda` and lunar-base-model's `griffin_flip_visual.usda`.
The Griffin Requirements, Traceability, and Structure screenshots retain 424
requirement elements, 23 sources, and 342 structural elements. With ten USD
editors retained, the trace contains no workspace-only SysML rebuild during
editor opening/focus. The one remaining workspace-only call is Twin admission
at trace time 1.109 s, taking 0.055 ms. The baseline had 12 such calls totaling
450.319 ms (37.527 ms mean). Mounted counts remain 17 bodies and 18 colliders;
the failed Griffin participant holds its dynamic bodies, so these counts do
not certify full Griffin physics behavior.

Whole-capture self-time diagnostics, in milliseconds per call:

| Owner work | Apollo baseline / updated | Griffin baseline / updated |
| --- | ---: | ---: |
| Port propagation | 0.5235 / 0.3367 | 2.7584 / 1.7674 |
| Rhai compile preparation | 0.0809 / 0.0539 | 0.2920 / 0.2832 |

These captures have different startup cache state and UI-window durations;
they are owner-level diagnostics, not product FPS acceptance. Another terrain
simulator was active throughout; a sibling Cargo build also overlapped the
aborted UI attempt, whose frame numbers are excluded. Griffin later reaches
tick 3600 without further steps, so its retained physics history after that
point is not fresh measurement of five/ten-editor windows. The maintained
`usd_editor_tabs.py --compare-first` driver records advanced steps per window
and keeps the first Visual camera fixed as more source documents stay open.

Apollo's full-ready transition was 10.526 s after Twin open in the cold baseline
and 1.756 s in the warm updated capture, validated by a five-second soak.
The baseline lowered three solve models (largest worker span 8.482 s); the warm
capture used disk cache and has no lowering spans. The cache was not changed,
so this is not evidence of a cold-start optimization. Both app window and API
health were available within two seconds of process launch.

The historical Griffin run was blocked by unavailable `PressureFedValve.mo`
and `PressureFedCombustionChamber.mo` assets required by MainPropulsion.
UI-only captures retained that explicit `program_failed` item. Both assets
are present after the current main integration; full Griffin readiness still
needs a fresh verdict. The exact isolated FLIP scene is the narrower
lunar-base-model full-readiness fixture.

Artifacts are under `target/apollo-{baseline,post}-20261005.json*` and
`target/griffin-editors-{baseline,post-complete}-20261005.json*`; the corresponding
self-time/rebuild exports are in `target/*self.tsv` and
`target/griffin-editors-post-complete-rebuilds.tsv`. The updated Apollo
`editor_preview` window requested an incorrect source path and is excluded;
its four View/Builder/Editor/return windows are valid. Unprofiled acceptance and
production behavior checks are recorded separately below when complete.

### 2026-10-05 — sandbox loading priority

Loading was the first acceptance gate, before FPS or Griffin ramp unfolding
work. The sandbox baseline reached full readiness in 12.170 s from
launch. Repeated warm launches varied from 3.033 to 9.611 s despite identical
generated source text: generated compile requests captured unrelated open
Modelica documents as siblings, so asynchronous publication changed the
persistent solve-cache key. The production document origin also uses the
reserved `generated/` filename namespace. Structural sharing now recognizes
that authoritative runtime provenance, and generated compiles carry only their
complete generated source plus the existing admitted class roots. Authored
multi-document compilation remains unchanged. Numeric equations and graphic
coordinates remain part of structural identity.

Both focused `lunco-modelica-worker` tests selected by `generated_` pass. Warm
diagnostic launches use disk solver IR without lowering. The subsequent owned,
unprofiled High-quality sandbox run still took 5.290 s from launch, so the
two-second loading target is not accepted. A sibling simulator remained live;
no other session was stopped to improve this number.

The vehicle projector now checks its existing current-generation topology
classification before charging ordinary visual-only prims to simulation
admission. The simulation-bearing prefix remains 32; independently bounded
ownerless markers publish in stable order. Missing/stale topology and runtime
instances cannot use that fast path. The named
`ownerless_admission_requires_current_noninstance_topology` test passes,
including reverse candidates, marker bounds, stale facts, and instance
exclusion. The owned unprofiled sandbox run with this change reached stable
full readiness in 3.311 s from launch (health at 0.719 s), and the production
Rhai smoke scenario passed all seven checks. Subsequent preserve-edits
`RestartScene` operations reached stable full readiness in 1.143 and 0.989 s;
these hot reloads do not certify the two-second fresh-launch target.

The sandbox scenario now declares the wheel/pose entities it reads. Its
physical-wheel inspection uses the same reflected `PhysicalWheel` component
registered by the vehicle plugin. Earlier application-owned diagnostic
scenario launches failed 1/7 checks and are not passing behavior evidence;
scene USD queries require the real Twin-owned simulation route. The task-local
driver obtains that validated owner from the existing tool-library query
instead of assuming a Twin ID. The Twin-owned production run passes wheel
realization, composed topology, ground/ramp, visual metadata, wiring,
oscillator, and fixed-joint checks (`TESTS_OK 7`). This is not the separate
serial/default-thread determinism comparison.

Domain discovery now excludes ordinary content-GID lifecycle edges, which do
not enter the shared Modelica instance namespace. The focused
`domain_discovery_observers_admit_only_identity_dependent_namespaces` test
passes content exclusion, path admission, instance identity add/remove, and
unsettled/derived provenance. Stage/source and instance-projection events keep
their existing invalidation routes. The owned production sandbox smoke still
passes 7/7 after this change; its unprofiled launch took 4.083 s, followed by a
1.163 s preserve-edits restart. A diagnostic warm launch confirms seven
initial network discoveries and no duplicate discoveries, reaching readiness
in 2.647 s from launch / 1.896 s after Twin open. These measurements do not
yet accept a two-second fresh-process launch, and another simulator remained
running during them.

The bounded initial-publication change passes
`initial_domain_publication_requires_a_hold_and_distinct_roots`. It batches
only roots without an installed projection that still own their initial
fixed-clock hold; live replacements retain the one-per-Update limit. The
owned unprofiled, uncontended High sandbox run reached readiness in 2.815 s
from launch / 1.465 s after Twin open, with a 0.758 s preserve-edits restart.
The seven production Rhai checks pass and the generated source is exactly
equal to the previous passing run. This restores the under-two-second scene
loading milestone, not a two-second complete fresh-process launch. FPS and
Griffin ramp tests follow that scene-load milestone.

The separate filtered startup Tracy capture contains seven synthesis calls
(81.998 ms total self time), a 0.228 ms maximum publication-system body, and
877.257 ms renderer-plugin construction. Source-root and compile publication
remain ordered. Its repeated Core3d schedules include Bevy auxiliary local
light shadow views. The subsequent API camera audit reports one active scene
camera and zero USD previews; that audit used the Tracy-enabled executable
and is not unprofiled performance acceptance.

Griffin ramp unfolding follows the restored scene-loading milestone. No
ramp-motion FPS claim is made from static inspection or held-propulsion captures.
All application outputs removed during disk recovery were generated in this
checkout; authored source, sibling worktrees, and shared caches were retained.

### 2026-10-06 — admitted Apollo FPS and source-preview isolation

The separate owned, unprofiled High Apollo run has no competing simulator or
Cargo build. API-deduplicated frame medians were 4.567 ms in View, 7.274 ms in
Builder, and 4.259 ms in Editor; the movement/rotation window measured 5.101 ms
median and 9.856 ms maximum. Native input produced 89.660 m of displacement and
a 90-degree turn. Physics advances throughout; whole-step medians were
0.389–0.420 ms, but tails exceed 0.5 ms. These sampled windows do not certify
every rendered frame or the complete 150 FPS / 0.5 ms target. The first Apollo
launch after the cache-key change took 8.554 s; a later diagnostic launch shows
all three solver IR disk hits. All-Twin two-second loading remains unaccepted.

The initial ten-editor driver incorrectly used `OpenFile` for external USD
sources. Its first open replaced the active Twin and stopped physics. That
window is rejected, not reported as editor FPS acceptance. USD browser actions
now dispatch the existing `OpenUsdSourceDocument` inspection intent. The named
browser-outbox seam test passes; non-USD browser actions remain available to
their owner. The driver uses the same source-only command and rejects windows
without new physics steps. The authored `usd_source_isolation.rhai` production
gate covers successful preview and invalid-source isolation.
The owned Apollo production run passes all seven isolation checks
(`TESTS_OK 7`, `USD_SOURCE_ISOLATION: PASS`), including new physics steps during
preview preparation and rejection without changing the document count. Its
registered scenario URI is `lunco://scenarios/tests/usd_source_isolation.rhai`;
native filenames are not scenario asset identities. The earlier native-path
launch was rejected by the asset owner and produced no verdict.

The corrected unprofiled ten-source comparison retains 624 bodies, 618
colliders, one dynamic body, and advancing physics throughout. Its same-camera
Visual medians range from 5.631 to 8.887 ms; ten previews measure 8.359 ms.
Authored/composed text medians are 8.326/8.648 ms. This is not yet editor FPS
acceptance. The separate 82-second Tracy capture contains no recurring parked
preview camera rendering or tree re-projection. Local-light shadow views are
separate from that camera observation. The sidebar Prim tree and Twin browser
cost about 0.7–0.8 ms each per paint in the one/five/ten-preview windows.

The library browser now consumes its owner's manifest revision, and the Prim
tree caches open rows and paints only the scroll viewport. Their named
manifest-revision and visible-row seam tests pass, including replacement,
empty inventory, descendant reveal, unrelated path rejection, and independent
preview disclosure state. The source-isolation production gate now also
checks preview selection alongside invalid-source rejection. These presentation changes
do not change authoritative clocks or simulation admission.

The eight-check source gate uses the shared Rhai task sequence and reads
`InspectUsdSelection` in the exact preview lease. Generic scene selection IDs
are not the preview selection contract. Initial pending selection is waited
for, not counted as a missing authoritative result. The settled production
screenshot shows the selected Motor RR row revealed and highlighted.

A fresh 93-second Tracy capture after virtualization measures Prim-tree paint
self time at 0.232 ms mean (0.771 ms before). Browser paint remains about
0.739 ms mean. These are whole-capture diagnostics with different window
durations, not an unprofiled FPS gain. Its paired one/many windows retain the
same recorded camera target and distance. The maintained driver now explicitly
frames the settled source, records focused view geometry per window, and
rejects an equal-camera comparison if its pose changes. Reopening an explicitly
closed preview uses `OpenUsdPreview`, not a source-document open. The earlier
comparison without recorded poses cannot prove an equal-camera editor-count
effect; its physics-advancement evidence remains valid.

The subsequent unprofiled guarded paired run retains exactly the same camera
pose and advancing physics. Warm one/ten-preview medians are 5.622/8.614 ms,
so the editor-count penalty remains. Per-window exports of the fresh Tracy
capture show about three shadow/Core3d passes per frame with one preview and
17 with ten, while Prim-tree paint stays near 0.26 ms. The parked previews'
local-light shadow-view work is the next rendering-owner target; these counts
do not justify disabling shadows in visible views. Artifacts are
`target/apollo-editor-paired-camera-guard-20261006.*` and
`target/apollo-editor-virtual-paired-20261006.tracy`.

The rendering-owner trace confirms that parked lights have already lost their
`ExtractedPointLight`, but their native shadow-view trackers retain graph roots.
The adapter now retires that tracker through Bevy's cleanup observer when the
light remains absent at deferred commit. Main-world lights and documents are
untouched. The named retirement seam test passes, including active-light
retention and a fresh tracker on re-extraction. The shadow-filter seam covers
incremental extraction, source shadow-intent changes, layer-disjoint unbounded
spots, and camera reactivation; all eleven filter tests passed before the
focused source-intent assertion was extended and passed separately.

In `target/apollo-editor-shadow-lifetime-fixed-20261006.tracy`, the warmed
one/ten-preview windows each execute three shadow/Core3d roots per frame, rather
than three/17. The counts are 795/265 and 693/231 shadow-pass/preparation calls;
the initial ten-preview window has one call crossing its sampling edge. These
are profiler diagnostics, not FPS acceptance. The settled screenshot after
text-to-Visual return retains the same assembly and lighting.

The separate uncontended High-quality run in
`target/apollo-editor-shadow-lifetime-acceptance-20261006.*` records warmed
one/ten-preview medians of 5.838/5.984 ms, with the same guarded camera pose and
advancing physics (624 bodies, 618 colliders). The editor-count penalty is now
0.146 ms rather than 2.992 ms. This is scoped acceptance of the warmed editor
count, not overall FPS acceptance: p99 is 10.423/10.743 ms and the later
text-to-Visual return median is 8.273 ms.

The subsequent unprofiled sandbox run reaches full readiness 2.216 s after
process launch and passes all seven production smoke assertions. Its first
View/Builder windows have medians of 11.676/11.630 ms; a foreign Cargo build
overlaps later windows, which are excluded from acceptance. The final screenshot
was not published before shutdown, so it is not visual evidence. The diagnostic
driver now waits for a newly published screenshot before closing the session.

The fresh sandbox Tracy capture
`target/sandbox-fps-tracy-lifetime-20261006.json.tracy` attributes 0.316 ms per
frame to duplicate standalone Modelica topology publication for co-simulation
entities. That publisher now matches the standalone backend's
`Without<SimComponent>` scope. The named generic resource-seam test proves
co-simulation exclusion, unchanged live samples, and standalone membership
invalidation. The updated production build passes the seven sandbox smoke
assertions with no runtime errors in
`target/sandbox-standalone-owner-final-20261006.json*`. Window creation and API
health occur 1.914/1.924 s after process launch; full readiness occurs 3.637 s
after launch, or 1.541 s after the `StartupScene opened` Twin origin, and remains
clear for the five-second soak. All four screenshots are published before API
shutdown; the settled return-to-View screenshot retains the scene and shadows.
The concurrent sibling Cargo test affects the frame windows, so they do not
establish a clean FPS gain from the final topology-publication change.

The final diagnostic capture
`target/sandbox-standalone-owner-tracy-20261006.json.tracy` measures the
standalone topology publisher at 0.0051 ms mean over 1,673 calls, compared with
0.3158 ms in the preceding sandbox capture. The sibling Cargo build still
affects whole-frame times; this is owner-cost attribution, not FPS acceptance.
The earlier disk-recovery run retained original baselines, paired shadow-root
captures and final sandbox exports at that time. Those artifacts are not
present in the current checkout; reproducing them requires fresh profiling.

The bounded serial sensor startup run reports `TESTS_OK 58`, first tick 1, and
actor order PASS, but also reports two Twin-scoped policy manifests under the
generic test folder and an undeclared sensor actor read. It is not accepted as
a clean determinism gate. The cross-run serial/default comparison remains open;
the ordered reverse-worker seam and exact sandbox generated-source comparison
do not substitute for that runtime acceptance.

The diagnostic Griffin run uses this checkout's owned production executable
with `LUNCO_ASSET_ROOT` explicitly selecting local main's committed asset
library. It reaches admission after 49.672 s, then stops physics at step 264:
the older solver reports non-finite `network_system.NozzleDesign.expansion_ratio`.
The new physics-step guard rejects the baseline before any unfold command;
no ramp FPS result is claimed. The committed solver/propulsion update is now
integrated through main; a fresh Griffin run remains required. No physics
model was substituted, changed, or bypassed to obtain a performance number.

### 2026-09-30 — skip hidden status history and performance HUD samples

The status bar previously cloned and rebuilt the complete discrete event
history on every frame while its popup was closed. It now snapshots history
only after the History popup is confirmed open, before the popup contents are
painted; opening the popup still shows the current history on that frame. The
frame-time history copy and p99 calculation also run only when the performance
HUD is enabled. These remove closed/hidden UI work; no frame-time gain is
claimed without a clean run and measurement. Focused
`cargo +nightly-2026-02-27 check --locked --offline -j 4 -p
lunco-workbench` passed; no tests or runtime profile were run.

### 2026-09-30 — recheck retained Builder and Twin stall spans

Re-exporting `builder-current-plotted-20260930.tracy` with
`tracy-csvexport -u -f workbench` found 7,382 `render_workbench` calls at
0.349 ms p50 and 0.949 ms p99, with an 11.058 ms maximum at trace time 2.310 s.
The trace does not contain per-panel spans; this isolated early maximum cannot
be assigned to a panel or explain the reported same-session View/Builder
penalty.

The retained `sss-open-apollo-20260930.tracy` and
`sss-open-apollo-warm-20260930.tracy` exports show nine and twelve
`sync_twin_overlays` calls, with 4.231 ms and 2.578 ms maxima. They do not
reproduce the earlier 261–278 ms calls. Those earlier stalls were attributed
to history-gap `UsdStageProjectionPlan::from_recipe` work and a dependent-stage
refresh; later source changes moved immutable plan preparation through bounded
async admission. The 2026-09-29 First Drive trace recorded a 1.481 ms maximum,
but it did not exercise the same dependent-layer startup case. These
profiler- and workload-affected traces remain diagnostics, not acceptance
results.

### 2026-09-30 — classify the later Builder-named capture

The retained `builder-lineplot-postalloc-20260930.tracy` message stream records
a First Drive startup on API port `4109`, but no explicit `rover_build`
activation. Its export has 7,435 Egui passes at 0.695 ms p99 / 2.912 ms max
and 7,436 `render_workbench` calls at 0.725 ms p99 / 1.684 ms max; it contains
no `workbench_panel_render` or `line_plot` spans. Treat it as a startup/UI
diagnostic only, not Builder or graph attribution. The same log reports the
scenario telemetry inbox reaching its 4,096-event cap at 72.343 s; this is a
separate event-delivery diagnostic with no established link to Builder frame
cost.

A clean production rebuild was attempted after checkout-local `cargo clean`.
The partial `target/` reached 4.5 GiB as available filesystem space fell to
48 KiB, so the build was stopped before a new binary or runtime sample existed.
A second checkout-local `cargo clean` removed those partial outputs and
restored 4.3 GiB free. Sibling worktrees, active simulator sessions, and shared
caches were left untouched.

### 2026-09-30 — share fixed-weld lookup across mobility force systems

The fixed-step mobility pass previously scanned all fixed joints and allocated a
dynamic-body set separately for suspension and drive. It now refreshes one
retained-capacity set before the ordered force chain and shares it across both
consumers. Rollback replay refreshes the same owner resource before replayed
forces, preserving the live-joint decision at each deterministic tick boundary.
This removes one duplicate joint scan and set construction from each force
pass. `cargo +nightly-2026-02-27 check --locked --offline -j 4 -p
lunco-mobility` passed. No tests or post-change profile were run, so no
frame-time or physics gain is claimed.

### 2026-09-30 — profile Summer Space School Twin admission

After the authorized local `cargo clean`, the Tracy-featured production build
`cargo build -p lunco-luncosim --features tracy -j 4` passed in 4m53s. A
windowed High-quality app used owned API port `4105`; Tracy capture started
before the app. The first recorded `/api/health` and empty-world `/api/ready`
responses succeeded, but process launch was not timestamped, so this run does
not establish the app-start target.

`OpenTwin` for
`/home/rod/Documents/models/summer_space_school/space-school-twin` was accepted
at 02:44:50.989 local time. The app log recorded the Twin open at 02:44:51.004,
the doc-backed default scene mount at 02:44:51.095, and physics admission
complete at 02:44:52.462. `/api/ready` still had two `program_compile` items at
02:45:23.429 and first reported all readiness fields clear at 02:45:33.074;
the transition is bounded to 32.4–42.1 seconds after acceptance and was not
captured exactly. Readiness remained clear at 02:45:47.715. This misses the
2-second full-Twin-readiness target. Sandbox admission and physics-step
acceptance were not measured.

The 90.4-second trace contains 4,848 frames and 20,779,197 zones. Five
`modelica_solve_preparation_lower_for_live` worker spans took 7.90–13.50 seconds
(the other measured spans were 0.84 seconds and 0.036 seconds). These ran on
the dedicated Modelica solve-preparation pool, which logged four workers.
Physics admission completed around 1.5 seconds; slow solve lowering remained
on the Twin readiness path. The trace shows CPU-heavy off-thread preparation,
not a UI-thread wait on those worker spans. It does not show whether persisted
prepared-solver entries would hit on a repeated launch; verify that before
changing the cache or lowering contract.

During the post-ready portion, 64 one-second `--log-diag` latest-frame samples
were 40.49 ms p50, 83.43 ms p95, 115.36 ms p99, and 180.19 ms max. They were
collected with Tracy attached and two other LunCoSim sessions active, so they
are contention-affected diagnostics, not a clean FPS result. The owned app
exited through typed API `Exit`; port `4105` closed. The other sessions were
left untouched. The raw trace is
`scripts/perf/captures/sss-open-apollo-20260930.tracy`.

This does not reproduce the screenshot's exact Telemetry-dragged graph source.
The retained Builder capture with eight Modelica plot signals measured Graphs,
Telemetry, and Entities panel bodies below about 1.1 ms p99; the broad Builder
frame penalty remains unlocated. In that capture, `prepare_telemetry_catalog`
peaked at 0.076 ms, and the source already copies at most 64 channel rows per
Update before building the tree on the compute pool. Those paths are not the
current high-cost lead. No renderer code changed. The handover remains
uncommitted. Visible Telemetry rows now reuse a cached drag payload with an
`Arc<str>` path, materializing a `String` only when a plot or canvas drop is
committed. The focused `lunco-viz` check passed; this allocation reduction has
no measured FPS result. The source change is local commit `9f9a301c1`,
fast-forwarded into local `main`; nothing was pushed. No renderer code changed.

### 2026-09-30 — reuse stable workbench anchor keys

The title-bar and dock layout previously created a fresh anchor vector and
allocated key strings for stable menu, toolbar, window, and panel anchors on
every egui pass. The workbench now retains that scratch vector in its system
local, borrows compile-time keys, and publishes registered panel anchors by
`PanelId`. Static anchor maps retain capacity across the existing per-frame
clear, while dynamic menu and perspective labels keep their owned-key path.
Guided anchors still reflect the active layout each pass, so closed panels do
not leave stale targets. This removes repeated small allocations from the UI
cycle; it is a focused steady-state reduction and is not claimed to explain the
full reported Builder/View FPS gap.

The latest instrumented A/B used First Drive with the exact visible Graphs
series `Dish Gimbal · angular acceleration [rad/s²]` in Builder, then switched
to View in the same process. In the stable Builder window,
`render_workbench` was 2.887/5.017/6.791/19.107 ms and the full Egui primary
pass was 3.405/5.897/8.344/24.101 ms (p50/p95/p99/max). In View those values
were 0.891/1.614/2.164/4.163 ms and 1.640/3.189/4.363/6.592 ms. Individual
Builder panel bodies, including Graphs, stayed below 1.5 ms p99. The capture
was Tracy-instrumented and overlapped active Terrain, Tutorials, and
sysml-integration sessions; these values are diagnostic only. The trace did
not contain the line-plot history subzones, so it does not isolate graph history
work. The captured frame intervals and physics values were noisy and do not
count as acceptance. The full reported FPS penalty remains open; clean
uncontended Builder/View acceptance has not been demonstrated.

This was the 180 s capture `target/builder-dish-graph-180s-20260930.tracy`
from the optimization checkout on API port 4101. It included active Terrain,
Tutorials, and sysml-integration workloads. The owned app accepted typed API
`Exit`, and port 4101 was released. A later authorized `cargo clean` removed
13.0 GiB of local target outputs, including the raw trace after its metrics
were extracted; sibling worktrees and shared caches were untouched.

### 2026-09-30 — warm Twin cache and plotted Builder diagnostics

An owned Tracy production session opened the Summer Space School Twin on API
port 4106. The 90.3-second capture,
`scripts/perf/captures/sss-open-apollo-warm-20260930.tracy`, contains 7,311
frames and 28,262,833 zones. The Twin-open event was logged at
01:47:03.772 UTC; the composed document scene began mounting at 01:47:04.149,
and physics admission completed at 01:47:06.793 (3.02 seconds after Twin-open).
This is the physics-admission milestone, not full Twin readiness. The first
`/api/ready` response was taken before the logged Twin-open event and is not
readiness evidence for that Twin; the exact full-readiness transition was not
timestamped in this run.

The Tracy capture recorded six prepared solver lookups: five persistent-cache
hits and one worker-side `lower_for_live` miss lasting 422.064 ms, followed by
a 4.373 ms cache save. Solver lookup work totaled 58.09 ms with a 37.60 ms
maximum. The miss remained on the worker path. Two other simulator sessions
were active on ports 4113 and 49376, so the capture is diagnostic under
contention, not startup or FPS acceptance.

A separate owned Builder session used API port 4108 and First Drive. Its
90.4-second Tracy capture,
`scripts/perf/captures/builder-current-plotted-20260930.tracy`, contains 7,384
frames and 33,505,303 zones. The screenshot confirms an active graph for
`network_system.Shaft_RR.phi`; its eight latest retained samples were all zero.
This does not reproduce the screenshot's non-flat `Dish Gimbal · angular
acceleration` signal. The Egui primary pass measured 0.688 ms mean and 16.919 ms
maximum across 7,382 calls. This capture did not contain the per-panel workbench
spans, so it cannot attribute that maximum to Graphs or another Builder panel.

After Tracy stopped receiving data, API samples in the same Tracy-enabled
process produced Builder A at 11.70 ms median / 12.88 ms maximum, View at
9.17/13.66 ms, and Builder B at 12.74/24.62 ms. Median physics samples were
1.22, 1.31, and 1.28 ms respectively. Two other simulator sessions remained
active. These samples neither reproduce nor clear the previously measured
1.73x Builder penalty and do not meet the 150 FPS target; they are not clean
acceptance data. Earlier captures' low panel-body timings cannot fill the
missing panel attribution for this run.

Commit `783d52a95` stores the graph repaint path's three temporary lists in
`SmallVec`s
with inline capacity eight and moves each freshly built label directly into
its immediate-mode plot item. Retained point buffers remain borrowed from the
existing cache. `cargo +nightly-2026-02-27 check --offline -j 4 -p lunco-viz
--features lunco-viz/ui` passed. This reduces common-case list allocations and
one label clone per series repaint; no post-change FPS delta was measured.
Plot geometry and renderer code were not changed. The current Builder/View FPS
gap and graph-specific stalls remain open, and this handover remains
uncommitted.

### 2026-09-30 — First Drive trace after plot allocation change

The rebuilt production Tracy binary ran First Drive on owned API port 4109
with High quality, uncapped presentation, and `--log-diag`. The 90.4-second
capture, `scripts/perf/captures/builder-lineplot-postalloc-20260930.tracy`,
contains 7,437 frames and 34,238,157 zones. The API accepted requests to
activate `rover_build` and add `network_system.Shaft_RR.phi`, but no screenshot
or binding query confirmed those requests took effect. Three other simulator
sessions were active on ports 4113, 4153, and 49376, and diagnostic logging was
enabled. This is a contention-affected First Drive trace with Builder state
unverified, not a clean FPS result.

`EguiPrimaryContextPass` measured 0.685/1.126/1.583/8.379 ms mean/p95/p99/max.
`run_egui_context_pass_loop_system` measured 0.969/1.553/2.165/9.460 ms, and
`render_workbench` measured 0.397/0.660/0.975/6.515 ms. The 7,437 captured
frames over 90.4 seconds average about 82 FPS, below the 150 FPS target; this
does not quantify Builder's penalty. The plot and workbench source contain
child spans, but Tracy exported no
`workbench_panel_render` or line-plot snapshot/build spans from this run, so
the tail is still unattributed. This result does not measure a before/after
delta for the inline-allocation change.

The same process logged one scenario event-inbox overflow at 02:51:40.297 UTC.
The bounded queue reached its 4,096-event limit and latched delivery; the event
producer and event mix were not captured. The collector previously cloned each
incoming event before discovering that a faulted/full inbox would reject it.
Local commit `ab13d36cf` defers that clone until after the capacity check while
preserving the existing public owned-event API. `cargo check --locked -j 4 -p
lunco-scripting` passed. No tests were run, and this removes post-latch copy
work without yet identifying or stopping the producer that filled the inbox.
The requested checkout-local `cargo clean` removed 13.9 GiB; the focused check
rebuilt 297 MiB of target outputs and 11 GiB remained free. No renderer code
changed; the handover remains uncommitted.

### 2026-09-29 — Builder tree repaint work

Builder tree rendering now retains the open-row index across repaints. The
shared tree branch reports disclosure changes; Entities rebuilds its flattened
rows only when the asynchronous scene-tree revision or expansion changes, and
Telemetry rebuilds its visible-row index only when its catalog/focus/filter/display key or
expansion changes; its structural tree receives descriptor patches. Telemetry row descriptors share immutable `Arc` data. Latest telemetry
samples and selection remain live paint inputs. SpawnCatalog now owns a sorted
category-to-entry-index projection and revision, so opening a Spawn category
does not clone catalog rows or rescan every entry; formatted labels are kept in
the palette until that revision changes. Ports now retain filtered expanded and
collapsed row indexes by topology/filter/expansion and publish expanded-port
sampling requests only when the set changes. The shared workbench and profiling
guidance now state this tree-cache contract. The Editor Prim tree borrows the
selected-entity list and reads its per-preview comparison snapshot without
cloning; it writes a new snapshot only when selection changes.

These are source changes only. No Builder runtime comparison has been captured
after this change; measure Builder and View in the same owned session and scene
before claiming an FPS improvement. The 3D renderer was not changed.

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
when connectors exist. The authoring-review view model now compares the
selection, controlled target, and displayed camera target/mode it actually
consumes. This avoids reopening the full view projection when camera
reconciliation merely mutably borrows `SceneViewport` or changes camera pose
without changing the followed target; in-place target changes still wake the
view. The regression covers both paths. In the pre-change 60.35 s Tracy
capture, the producer ran 1,756 times (1,713 after the first 10 s); in the
post-change 55.33 s capture it ran 48 times (11 after the first 10 s), reducing
the observed call rate from 29.1/s to 0.87/s. This is profiler attribution,
not clean FPS acceptance; a settled unprofiled Apollo run remains necessary
before claiming an overall FPS improvement. The before/after captures are
`summer-space-school-optimization-ccbfe504-settled-20260924.tracy` and
`summer-space-school-optimization-ba7550-authoring-view-gate-20260924.tracy`
in the ignored `scripts/perf/captures/` directory.
The shader-look binder also reuses loaded WGSL stage verdicts by asset ID and
stage, invalidating them on shader asset events so shared terrain looks do not
revalidate identical source; see the dated note in the [200 FPS
handover](open-200fps-performance-handover.md). No measured FPS delta is
claimed.

The 2026-09-26 Apollo startup trace found an admission feedback loop: while
`BindingEpochDirty` stayed true for pending participants, `settle_binding_epoch`
called `BindingRevision::open_epoch` every frame. Each call advanced the
revision, waking both the full connection binder and causal-participant graph
projection. Over the 25.35 s diagnostic capture, those systems ran 855 and 860
times and consumed 523 ms and 334 ms of self time respectively. An open epoch
is now idempotent; topology and actual Modelica lifecycle transitions request
reconciliation. The same capture measured authored-runtime projection at 43
calls / 1.652 s self time. Source inspection found that each projection batch
recomputed a stage-wide Modelica membership set already computed independently
by co-simulation wiring. Both consumers now use one shared cache in
`lunco-usd-bevy-core::program`, keyed by stage asset, generation, and instance,
with teardown cleanup. This removes the repeated scan by construction; a
post-change Tracy count is still required to quantify the reduction. The trace
included profiler and concurrent machine load and is not clean FPS acceptance.

### 2026-09-26 — Apollo startup projection and frame outliers

The latest Tracy-enabled production run used the authored
`traverse_apollo15.usda` scene, High quality, X11, `--no-vsync --no-throttle`,
and owned API port 43862. Its 45.34 s capture is
`scripts/perf/captures/summer-space-school-celestial-time-gate-b045cecc4-20260926.tracy`
(2,235 profiler frames; 10,843,346 zones). Other simulator and Griffin-test
workloads were active, so all timings below are diagnostic, not clean
acceptance measurements. Sandbox was not measured in this capture.

The app window appeared about 1.50 s after launch and the Twin directory opened
about 1.86 s after launch. `/api/ready` was still false with 12 pending items at
about 20 s, then was true with zero pending at capture end; the exact
ready-transition time was not sampled. Fast window/Twin-open response is not
full-Twin readiness.

Tracy attributed ten calls to the exclusive
`lunco_usd_bevy_runtime_core::twin_projection::sync_twin_overlays` system:
mean 55.18 ms, maximum 277.76 ms. The two largest calls were 260.85 ms at
trace time 3.136 s and 277.76 ms at 3.597 s, shortly after the Twin opened; a
later call at 25.126 s took 12.00 ms, and the other seven calls were at or
below 0.404 ms. Because this system holds `&mut World`, those startup calls
block the app thread and are a strong candidate for the observed UI stall.
Source inspection shows staged USD projection/build and dependent-stage asset
refresh work in this path, but the system-level trace does not identify which
sub-operation consumed the two long spans. Attribute that exact work before
choosing a fix; preserve the deterministic projection/commit boundary.

In the same instrumented/contention-affected run, 240-sample full-frame
telemetry was 16.21/27.48/32.43/36.18 ms p50/p95/p99/max (about 62.2 FPS at
the median). Whole fixed-tick service was 6.91/9.48/10.67/14.50 ms, with nine
service-budget exceedances; this is not Avian-only solver time. Selected Tracy
self-time means were 0.530 ms for Bevy preprocess bind-group preparation,
0.404 ms for Workbench rendering, and 0.397 ms for GPU-cluster preparation.
Cluster preparation also had a 142.62 ms outlier at trace time 39.790 s and a
25.64 ms outlier at 12.682 s. Schedule self times overlap and must not be
summed as a serial frame budget.

The follow-up capture attributes the largest `sync_twin_overlays` stall to
history-gap snapshot recovery; the dated note below records the cause and
cursor fix. Re-profile the same production scene with a matching post-change
owner-span export, then run an unprofiled, uncontended FPS/physics acceptance
session. Keep other sessions untouched. At handover drafting, ports 4101 and
4317 and a long Griffin test were still active; recheck before launching and
do not control those sessions.

Commit `199bb4f15` contains the scenario driver's `peer_target` contract update
and exposure-fact handling for the `UsdDocumentProjection` owner. The
Tracy-featured production build passed with those changes. Current history has
merge `672642fc2` with `199bb4f15` as its second parent. A read-only
`git ls-remote origin refs/heads/main refs/heads/optimization` on 2026-09-27
reported `origin/main` at `672642fc2` and `origin/optimization` at
`fecc1c689`. The `optimization` checkout is 1,739 commits ahead of the latter.
No fetch, push, or merge was performed for this handover; the handover file
remains uncommitted. At the 2026-09-26 disk check, `/home` had 2.0 GiB
available and a Cargo test/build was active in sibling `lunar-soil`; do not
overlap such builds or clean their outputs.

### 2026-09-26 — initial Twin projection and readiness milestones

The follow-up Tracy capture
`scripts/perf/captures/sss-sync-twin-overlays-ops-20260926.tracy` isolated the
startup stall to the history-gap recovery branch: one
`usd_twin_projection_rebuild_history_gap` call took 253.56 ms, of which
`UsdStageProjectionPlan::from_recipe` took 251.46 ms. Live canonical-stage
rebuild and scene-visual refresh together took under 0.6 ms. The cause was an
initial persistent-layer cursor at the current document generation paired
with a missing view cursor at zero, even though the disposable view layer was
empty; the bounded operation ring therefore looked expired. Projection now
starts the view cursor at the persistent snapshot generation when the view
layer is empty, while retaining history replay/rebuild when it has opinions.
The Tracy-enabled production build passed.

The post-change 15.25 s Tracy capture is
`scripts/perf/captures/sss-sync-twin-overlays-postfix-readiness-20260926.tracy`
(115 frames; 501,320 zones). Its `tracy-csvexport -u -f 'usd_twin'` export had
no matching CPU-zone rows, so it supplies no numeric post-change comparison.
The app window-created log event was at 1.11 s, `/api/health` responded at
1.22 s, and the Summer Space School Twin directory opened at 1.62 s. These
were profiler- and contention-affected diagnostics, not acceptance results.

The same capture run first returned `/api/ready` green at 1.90 s with zero
pending items, then regressed to active scene, physics, and Modelica waits. It
returned green again at 25.32 s, regressed to two pending items at 25.91 s, and
first remained green after all observed waits cleared at 26.36 s. An unprofiled
but still contended run showed the same false-green pattern: API liveness at
2.30 s; first green at 2.84 s; `scene_load`, `participant_init`, and ten
`program_compile` items appeared from 4.13–6.38 s. After those drained, two
late `scene_load` / `USD connection binding` items appeared at 38.11 s; the
first green state that stayed clear for five seconds was 38.62 s from app start
(about 36.0 s from the startup Twin scan). The current API's
`readiness_tracked` means the registry resource exists, not that every startup
producer has completed its first pass. Do not use its first green response as
full Twin readiness.

That unprofiled run's 240-sample whole fixed-tick service was
6.44/9.19/15.48/38.87 ms p50/p95/p99/max, with three budget exceedances; the
whole fixed-loop service was 6.34/13.31/14.69/20.29 ms. Neither is Avian-only
solver time. The timings and readiness run had other simulator work and the
Griffin test active, so they are diagnostic.

The sandbox was then measured in a Tracy-enabled diagnostic run using
`scenes/luncosim/sandbox_scene.usda` on owned API port 43631. The capture is
`scripts/perf/captures/sandbox-readiness-20260926.tracy` (12.28 s, 616 frames,
2,944,829 zones; readiness times below are from process start). The app window
appeared at 1.19 s, `/api/health` responded at 1.42 s, and the Twin directory
opened at 1.36 s. `/api/ready` first turned
green at 1.69 s, then reported `scene_load` at 2.42 s and later as many as 17
pending items. Full readiness first stayed clear at 15.93 s from app start
(about 14.57 s from Twin-open), confirmed for 3.2 s; the scene-participant
lifecycle-ready log event was at 15.64 s. Thus the early green response is not
a valid full-Twin milestone in this run either.

The 52-row `usd_twin` owner-span export from this sandbox capture recorded
`usd_twin_projection_document_sync` 13 times (0.525 ms total, 0.432 ms max);
no exported projection span exceeded that maximum. This is consistent with the
history-gap fix, but it is still a profiled, contended diagnostic.

The same capture's main-thread CPU zones show
`derive_causal_barrier_participants` ran in 593 of 616 frames (142.08 ms total,
0.240 ms mean, 0.744 ms max). The epoch readiness check called an idempotent
`BindingRevision::open_epoch()` through `ResMut` on every unsettled frame;
Bevy marks that resource changed on mutable dereference, so the `Changed`
condition rebuilt the full causal graph repeatedly. The readiness check now
touches the binding revision only when the epoch actually opens or seals. The
follow-up sandbox capture after this guard,
`scripts/perf/captures/sandbox-binding-revision-gate-20260926.tracy` (22.31 s,
1,177 frames, 6,159,755 zones), recorded 17 calls and 7.058 ms cumulative time
for `derive_causal_barrier_participants` (0.415 ms mean, 2.252 ms max). The
earlier capture had 593 calls in 616 frames and 142.08 ms cumulative time.
The reduced call count verifies that the readiness check no longer rebuilds
the causal graph every unsettled frame. Capture durations and contention differ,
and the new mean and maximum are higher, so this is not a clean timing A/B.

Sandbox whole fixed-tick service (153 samples) was
4.91/7.10/43.30/70.32 ms p50/p95/p99/max, with three budget exceedances;
whole fixed-loop service (240 samples) was 4.45/7.40/29.20/71.06 ms. Neither
measures Avian-only solver time. The run also logged a Bevy mesh-allocator
use-after-free error with 15 identical messages suppressed; its attribution
was not investigated here. Tracy overhead and concurrent workloads make these
readings diagnostic only. The 5 s full-Twin target remains unmet for both
Apollo and sandbox.

A further Tracy run of Summer Space School with the revision guard used
`scripts/perf/captures/sss-readiness-after-binding-gate-20260926.tracy` (40.37 s,
2,912 frames, 13,709,791 zones). The Twin-open event was at 1.041 s from trace
start; scene participants became lifecycle-ready at 23.559 s, about 22.52 s
after Twin-open, exceeding the five-second target. The first API readiness poll
was late, at 25.06 s after Twin-open, and showed `ready=true`, no hold/fault,
and zero pending work; that state remained clear through the 45 s poll end.
The exact `/api/ready` transition was missed. The run was profiler-affected
and the tutorials simulator on port 4101 remained active, so it is diagnostic
only. Typed API `Exit` was accepted, and the owned API/Tracy ports (43631/8086)
were closed afterward.

The same trace shows the `LunCo` Modelica source root (85 documents) ready at
trace time 2.63 s. A model compile finished at 4.58 s, but its compiled
lifecycle response was not applied until 11.51 s; additional lifecycle
responses arrived in batches through 23.23 s. The reported compile durations
were 0.01–0.15 s. This narrows the remaining wait beyond compiler execution,
but the worker/result queue and startup admission path still need attribution.

The same trace recorded two long app-thread `sync_twin_overlays` calls shortly
after Twin-open. At 0.71 s after open, a history-gap recovery took 244.16 ms,
including 242.55 ms in `UsdStageProjectionPlan::from_recipe`. At 1.64 s after
open, refreshing one dependent stage asset took 256.51 ms; the outer document
sync took 257.66 ms. The first call shows the earlier empty-view cursor fix did
not cover every startup document. The second rebuild follows a change to the
`lunokhod2.usda` component layer. Both costs still block the app thread and need
owner-level follow-up; do not mark the projection stall fixed.

Next, inspect why the root document still reaches `ops_since(None)` during its
first projection. Identify the `lunco-usd-bevy-stage` async preparation path
for history-gap and dependent-stage rebuilds, keeping canonical-stage commits
at their current owner boundary. Then poll both readiness milestones from
launch/open through a five-second stable interval, and
attribute the remaining Modelica, participant, and late deferred-prim waits.
Recheck external sessions before any clean acceptance run; do not stop or
reuse their ports. The latest owned
API/Tracy ports (43631/8086) are closed. At the latest check, API port 4101
was active in the tutorials worktree; ports 4317 and 3743 were free. A process
scan found no Cargo build, test, or check process and no Griffin test. `/home`
had 1.6 GiB free and this checkout's `target/` occupied 28 GiB. Recheck disk
and sessions before the next build or clean acceptance run.

### 2026-09-26 — solve-lowering and physics diagnostic

The follow-up Tracy production run used this checkout's `target/debug/luncosim`
with the `tracy` feature, the authored Apollo scene, High quality, X11,
`--no-vsync --no-throttle --log-diag`, and owned API port 4317. Its capture is
`scripts/perf/captures/sss-modelica-prep-zones-20260926.tracy` (45.38 s,
3,150 frames, 15,591,458 zones; 123 MB). The capture started before the app.
The other Summer Space School session on port 4101 stayed active, so all
measurements are contention- and profiler-affected diagnostics.

The trace recorded the window-created event at 1.544 s and `StartupScene
opened` at 2.041 s from trace start. The canonical stage opened at 3.804 s,
1.76 s after Twin-open. Scene participants became lifecycle-ready at 19.473 s,
about 17.43 s after Twin-open. Readiness was sampled with 12 pending items near
startup and sampled green about 42 s from process start. The transition and
five-second stable interval were not sampled; this does not establish the 5 s
target. The scene-mount time is under 2 s in this diagnostic, but is not a clean
acceptance result.

New worker zones isolate `lower_for_live` as the long Modelica preparation
stage. Rumoca compile calls took 0.05–0.15 s; solver lowering took 4.370 s for
Ackermann, 0.481 s for Rover, 8.159 s and 8.446 s for the two rocker-bogie
systems, 9.446 s for Electrical, and 0.056 s for CommsLink. Repeated
EarthTracker lowerings took about 13 ms. Preparation queue waits were 11–338
microseconds. The worker had been disabling persistent prepared-solve reuse
after every source-root change. `ModelicaCompiler` computes a deterministic,
content-sensitive revision over sorted root identities and file contents, so
the disk key can safely distinguish those admitted source sets. Rumoca lowering
also selects value-only versus differentiable solve IR by solver mode, while
the disk record did not include solver identity. The prepared-solve cache now
keys disk entries by solver identity, bumps its record version, and clears only
the worker-local models on root changes. A warm retry still missed five of the
seven persistent entries: the key included worker-local library generation
and, for units with sibling documents, runtime sibling-document identities and
order. Those are not stable source identity; the prepared key now relies on the
separate admitted-library content revision and hashes primary and sorted
sibling source text without runtime document IDs. A corrected-key run
generated new entries and therefore still paid cold lowering. The following
warm Tracy run confirmed persistent reuse: there were no
`modelica_solve_preparation_lower_for_live` zones, and eight disk lookups
averaged 2.30 ms (6.96 ms max). This removes repeated warm-lowering cost; cold
lowering remains part of first-open startup. The Tracy zones bracket job queue
wait, cache lookup, lowering, result delivery, and the live-stepper commit.

The same capture's owner spans exposed two more app-thread costs shortly after
Twin-open. `usd_twin_projection_rebuild_history_gap` took 272.74 ms; the nested
`UsdStageProjectionPlan::from_recipe` took 270.73 ms, while opening the live USD
stage took only 3.65 ms. That projection plan was used only to validate a
replacement and then discarded before the canonical stage was rebuilt. The
rebuild path now prepares a replacement `CanonicalStage` before retiring
stage-owned ECS state, then commits it without the throwaway plan traversal.
The dependent-stage asset refresh still took 228.22 ms and needs its own
follow-up because it retains a refreshed projection plan. Rebuild the Tracy
binary and capture again to measure both paths; neither code change nor warm
cache behavior has runtime confirmation yet.

In the later trace window (after 25 s), Avian's full physics schedule measured
2.80 ms mean, 2.71 ms p50, 3.57 ms p95, 4.23 ms p99, and 5.97 ms max across
904 samples. Its nested substep schedule measured 1.41 ms mean, 1.34 ms p50,
1.89 ms p95, 2.25 ms p99, and 3.64 ms max. These are overlapping schedule
spans rather than an Avian-only solver measurement. On demand, the last 240
`engine.frame_time` samples measured 30.88 ms p50, 64.23 ms p95, 92.09 ms p99,
and 128.11 ms max; `engine.fps` measured 32.73 median. They are diagnostics,
not clean FPS or physics acceptance.

The owned session reported `/api/ready` green before exit. Typed `Exit` was
accepted and the owned API/Tracy ports (4317/8086) were closed. Port 4101
remains an external tutorials-worktree session and was left untouched. The
window event was under 2 s in this profile, but there was no timestamped
health sample at the startup boundary, so app readiness remains unverified.

### 2026-09-27 — warm Modelica cache and Twin readiness timing

The corrected cache keys were exercised in two Tracy runs of the same authored
Apollo scene. The first run generated entries for the new identity and recorded
seven `lower_for_live` jobs totaling about 30.7 s; it is a cold-cache run for
those corrected keys. The warm follow-up,
`scripts/perf/captures/sss-modelica-cache-key-fix-warm-20260927.tracy` (40.3 s,
2,410 frames), recorded no `modelica_solve_preparation_lower_for_live` zones
and eight persistent disk lookups (2.30 ms mean, 6.96 ms max). Scene
participants became lifecycle-ready 6.08 s after Twin-open. The first sampled
clear full-readiness state was at 11.35 s after Twin-open and stayed clear for
more than five seconds. This is profiler-affected, contended diagnostic
evidence, not a clean acceptance measurement.

The same warm capture recorded 14 `sync_twin_overlays` calls with a 3.47 ms
maximum, substantially below the earlier 261/278 ms startup calls. This
supports the history-gap recovery fix for that workload. It does not establish
unprofiled frame performance. The full `PhysicsSchedule` averaged 2.680 ms and
reached 10.666 ms max over 1,479 samples. Avian's nested
`run_substep_schedule`, which executes all eight production substeps, averaged
1.346 ms and reached 6.036 ms max over the same 1,479 ticks. That inclusive
span is the eight-substep execution block, including schedule dispatch; it is
not constraint arithmetic alone or the full `PhysicsSchedule`. The observed
block already exceeds the 0.5 ms per-step goal in this profiler-affected
diagnostic, before treating it as an acceptance measurement. The sandbox was
not measured.

### 2026-09-27 — rootless collider propagation gate diagnostic

The Tracy-enabled production binary captured
`scripts/perf/captures/sss-collider-idle-gate-20260927-230219.tracy` (35.37 s,
1,905 frames, 10,456,130 zones; 79.77 MB) on Apollo at High quality, with
`--no-vsync --no-throttle`, API port 4317, and the same `traverse_apollo15.usda`
scene. A candidate early return skipped the all-collider query when no transform
or hierarchy inputs were dirty, while including newly added collider transforms
in that invalidation set. The window-created and `StartupScene opened` logs were
at 23:02:35.661Z and 23:02:35.908Z. Process start was recorded only to whole
seconds, so the run cannot prove either sub-2-second launch milestone.
`/api/ready` was queried only after the profile had been running for over 20 s
and returned green; its transition and a five-second stable interval were not
captured.

For settled zone events (`t >= 10 s`), the candidate run's
`propagate_collider_transforms_rootless` durations were 0.0733 ms mean,
0.0162 ms p50, 0.1839 ms p95, 0.2184 ms p99, and 0.3500 ms max. The preceding
warm capture measured 0.0659/0.0072/0.1653/0.1975/0.4359 ms respectively.
Full `PhysicsSchedule` p50/p95 were 2.678/3.520 ms in the candidate run and
2.606/3.426 ms in the preceding capture. Ports 4101 and 4102 were active and
CPU-heavy during the candidate run; these are not controlled A/B or acceptance
results. The candidate showed no reliable timing improvement and was removed.
Both runs remain diagnostic; the 150 FPS and sub-0.5 ms targets are still open.

Further settled-window export (`t >= 10 s`) puts CPU `Main` at 13.473/20.513/
24.657 ms p50/p95/p99, `Render` at 8.634/10.738/14.374 ms, and `Update` at
2.372/7.736/9.585 ms. The individual GPU `main_opaque_pass_3d` zone measured
1.610/2.052/2.916 ms; this pass is not the whole GPU frame. These contended,
profiler-instrumented timings point to CPU-side render/schedule attribution.
`present_frames` was only 0.110/0.158/0.202 ms p50/p95/p99, so this run did not
show a present/v-sync wait. In the same window, `update_globe_lod` had 137 events
at 3.635/4.199/4.569 ms p50/p95/p99, while `sync_modelica_outputs` had 1,175
events at 0.352/0.838/0.996 ms. The globe system can cause intermittent hitches
but its low event frequency does not explain the whole-run median frame time.
External CPU-heavy simulator sessions and Tracy instrumentation affected this
capture; it still does not establish the clean bottleneck or product FPS.

Two warm, unprofiled runs logged readiness transitions more closely. In
`sss-modelica-admission-breakdown-20260927.log`, auto Twin-open was at
22:12:42.196906Z. Physics admission remained held from 2.05 s until 12.63 s
after Twin-open, with five dynamic bodies carrying `ShouldBeDynamic`,
`PhysicsStatePending`, and `PhysicsInitializationPending`. Late deferred-prim
and USD-connection-binding waits then invalidated an earlier green response.
The first green sample that remained clear for a five-second soak was 13.49 s
after Twin-open.

In `sss-modelica-terrain-gate-20260927.log`, auto Twin-open was at
22:17:46.137817Z. The readiness watcher missed the first clear transition; it
observed two late pending items at 6.84 s and first sampled clear at 7.35 s,
then remained clear for more than five seconds. Physics admission completed at
6.31 s and scene participants became ready at 6.32 s. The terrain gate itself
was brief: one DEM-generation request and two missing collider tiles cleared
within about 0.5 s total. Warm Modelica compilation completed within 0.08 s and
the persistent solver cache was hit by about 3.5 s. Transition-only diagnostics
now report terrain blockers and unresolved-joint count/entity IDs during
initial pose validation; the Tracy-featured build passed and the logs below
include the first transition capture.

App readiness has a 2 s target from process launch; full Twin readiness has a
5 s target from Twin-open. The 2 s scene-mount checkpoint is measured
separately. These
diagnostics show that warm Modelica lowering and the earlier overlay stall no
longer explain the remaining multi-second wait. The global unresolved-joint
barrier is now measured on the critical path; eight pending joints held pose
validation for 1.92 s in the latest run. That capture kept only transient ECS
IDs, so it could not distinguish unresolved body projection from pose seeding.
The USD physics joint owner now reports each authored joint and body path when
the wait reason changes, then reports when native topology projection clears
the wait. It distinguishes missing body endpoints, unseeded poses, and computed
drive inertia; `cargo check -j 4 -p lunco-usd-avian` passed. Use the next startup
log to attribute the wait before changing admission behavior.

In `sss-modelica-joint-gate-poll-20260927.log`, the app window and health
response were both available by 1.826 s from process launch. Auto Twin-open was
at 2.513 s from launch, and the live canonical stage reopened 1.394 s later;
the log does not independently timestamp the scene-root mount. The first
`/api/ready` green sample was only 0.371 s after Twin-open and then regressed
through pending counts 1, 2, 12, and 13, so it is not readiness evidence.
Physics admission completed 3.715 s after Twin-open, and scene participants
were ready at 3.723 s. A clear sample at 3.870 s regressed to two pending items
at 4.382 s. The first qualifying clear sample was at 4.622 s after Twin-open;
it stayed clear (`readiness_tracked=true`, `ready=true`, `world_hold=false`,
`faulted=false`, `pending_count=0`) for 5.192 s. This single no-capture run met
the 5 s full-Twin target. The build had the Tracy feature enabled, but no Tracy
capture client was attached.

This is still contention-affected diagnostic evidence, not clean acceptance:
ports 4101, 4102, and 4103 were active in external worktrees at run start;
4101 and 4102 remained active at the post-run check. The owned process accepted
typed `Exit`, returned code 0, and released ports 4317 and 8086. The earlier
unprofiled samples of 7.35 s and 13.49 s after Twin-open show substantial run
variance. Sandbox readiness and clean, uncontended acceptance remain
unverified. Do not stop or reuse external sessions; recheck before another
profile. After the production rebuild, this checkout's `target/` occupied
28 GiB and `/home` had 4.7 GiB free; no build cleanup was performed.

### 2026-09-27 — latest Apollo startup profile and readiness miss

The Tracy-enabled production binary ran the authored
traverse_apollo15.usda scene at High quality on X11 with no vsync or frame
throttle. The 50.33 s capture is
scripts/perf/captures/sss-joint-owner-wait-20260927-013225.tracy (3,879
frames; 15,958,773 zones). The app log is
scripts/perf/captures/sss-joint-owner-wait-20260927-013225.app.log. External
simulator sessions on ports 4101 and 4102 were active, so this is diagnostic
data, not clean acceptance.

The app window was created 1.399 s after process launch, and StartupScene
opened the Twin at 1.689 s. The runtime logged “mounting composed” 1.043 s
after Twin-open, but this run has no independent timestamp for the mounted
scene root. The API health and readiness watcher began only 60.32 s after
process launch (58.64 s after Twin-open), so the then-current 2 s app/API
milestone nor the first full-readiness transition was measured.

At 1.621 s after Twin-open, initial-pose validation was waiting on four
unresolved joint-topology entities. It resumed 11.341 s after Twin-open, a
9.720 s wait, and scene participants became lifecycle-ready at 11.520 s.
Because participant readiness is required by the full-readiness contract, this
run missed the 5 s Twin target. The first sampled clear /api/ready state came
much later, at 58.64 s after Twin-open, and remained clear through a 5.17 s
soak; it does not identify the actual transition time.

The settled Tracy distributions (events at or after 10 s from process launch)
were CPU Main 10.900/15.153/22.079 ms, Render 5.820/7.409/10.514 ms, Update
1.825/4.016/5.161 ms, and full PhysicsSchedule 2.370/3.075/3.875 ms at
p50/p95/p99. Avian run_substep_schedule, which executes the production
eight-substep block, measured 1.116/1.574/1.922 ms; this exceeds the 0.5 ms
goal in this profiler- and contention-affected diagnostic. sync_twin_overlays
had 12 calls over the whole capture, with a 5.209 ms maximum, so the earlier
261/278 ms calls did not recur in this run. A later live SimulationTimingProfile
query returned fixed-tick service p50/p95/p99/max of 4.161/5.490/6.933/8.769
ms over its 240-sample window, with 56 service-budget exceedances. That is
another contended diagnostic and is not the Avian solver measurement.

The same trace's five persistent Modelica solve-cache lookups totaled 7.02 ms.
Two missed and ran lower_for_live: preparation 3 for Traverse_Rover_System
took 0.370 s, and preparation 4 for Traverse_Rover_Electrical_System took
9.144 s. Preparation 4 waited only 26 microseconds in the worker queue; the
slow time was solver lowering, not queue admission. Its persistent cache entry
was saved by 13.224 s from process launch. The other three lookups hit disk and
did not lower. This is a cold, worker-side readiness cost; it is not a
synchronous app-thread stall, though the worker load can still compete for CPU.
The following warm-cache launch's eight solver-preparation commits took at
most 2.03 ms each. These commit timings do not establish the persistent lookup
outcome. Verify native preparation-owner disk-hit/miss logs for reuse evidence;
the readiness timings remain contention-affected.

The joint-wait path still needs attribution. The new wait-reason strings were
present in the running binary, but the app log contains no USD-Avian joint
wait/clear messages. Tracy records one build_usd_physics_joints invocation at
13.279 s from process launch, after the terrain wait had resumed; the capture
does not show why the four topology markers persisted or what cleared them.
Trace the marker lifecycle and the first fixed-schedule work before changing
the admission rule.

The owned app accepted typed Exit, exited successfully, and released ports
4317 and 8086. External ports 4101 and 4102 remained active and untouched.
After the clean rebuild and this run, target occupied 8.6 GiB and /home had
20 GiB free. Sandbox and clean, uncontended FPS/physics acceptance remain
unmeasured.

### 2026-09-27 — warm-cache Apollo readiness profile

The Tracy-enabled `target/debug/luncosim` ran the authored
`traverse_apollo15.usda` scene at High quality on X11 with no vsync or frame
throttle. The 40.32 s capture,
`scripts/perf/captures/sss-warm-cache-readiness-20260927.tracy`, contains 2,393
frames and 13,129,237 zones; the app log is
`scripts/perf/captures/sss-warm-cache-readiness-20260927.app.log`. External
sessions on ports 4101 and 4102 were active throughout, so these are diagnostic
measurements, not clean acceptance results.

StartupScene began opening the Twin scene at 23:51:31.053885Z. The live scene
root was spawned at 23:51:31.543880Z, 0.490 s later, within the 2 s scene-mount
target. Scene participants became lifecycle-ready at 23:51:37.197338Z,
6.144 s after Twin-open, missing the 5 s full-readiness target by about 1.14 s.
Physics admission completed at 23:51:37.189833Z. The exact `/api/ready`
transition was not captured: the first direct check was made more than a
minute after startup and showed `readiness_tracked=true`, `ready=true`, no
world hold or fault, and zero pending items. A later 5.057 s sample window
(47 polls, no request failures) stayed clear. This stable late sample does not
move the readiness transition earlier than the participant-ready event.

All eight Modelica solver-preparation commits were persistent-cache hits.
`sync_twin_overlays` ran 12 times, with a 5.586 ms maximum; the earlier
261/278 ms exclusive app-thread stalls did not recur. A live `QueryUsdPrim`
read of `/Traverse` succeeded. The live `SimulationTimingProfile` reported
fixed-tick service p50/p95/p99/max of 4.956/7.369/8.978/10.087 ms over its
240-sample window, with three service-budget exceedances. This is a contended
whole-tick diagnostic, not an Avian-only solver measurement. The capture's
2,393 frame records over 40.32 s are about 59.4 frames/s and are also
profiler- and contention-affected, not FPS acceptance.

The app window creation was logged at 23:51:30.786320Z, but process launch
was not timestamped and API/readiness polling began late, so the then-current
2 s app/API target was not measured. The owned app accepted typed `Exit` and
released ports 4317 and 8086; external ports 4101 and 4102 were left running.

### 2026-09-27 — joint admission and settled-window profile

The Tracy-enabled production binary ran the authored
`traverse_apollo15.usda` scene at High quality on X11 with no vsync or frame
throttle. Capture began before app launch on owned API port 4317 and Tracy port
8086. The resulting 30.56 s capture,
`scripts/perf/captures/sss-joint-startup-settled-after-index-20260927.tracy`,
contains 1,644 frames and 9,019,702 zones; the app log is
`scripts/perf/captures/sss-joint-startup-settled-after-index-20260927.app.log`.
External simulator sessions on ports 4101 and 4102 remained active, so these
are contention- and profiler-affected diagnostics, not acceptance results.

Process launch was timestamped at 00:49:46.863Z. The window-created event was
1.125 s after launch and `/api/health` responded at 1.222 s, within the
current 2 s app-ready target in this diagnostic.
`StartupScene opened` the Twin 1.420 s after launch; the asset root mounted
about 8 ms later. This run did not
independently timestamp the composed scene-root mount. The first full-readiness
sample that met all readiness conditions was 7.598 s after Twin-open. It stayed
clear for a 5.035 s soak, so this sample establishes a readiness transition but
misses the under-5 s Twin target by 2.598 s. Scene participants became
lifecycle-ready at 6.140 s after Twin-open, after physics admission completed
at 6.132 s.

Initial-pose validation waited on eight unresolved joint-topology markers
from 1.736 s through 5.450 s after Twin-open (3.714 s). `BindingStatus`
polling also observed pending joint admissions whose endpoints had not yet
entered physics islands; those admissions cleared before physics admission
completed. The authored-joint-to-marker mapping was not preserved in the app
log, so identify that mapping before changing the admission barrier.

In the settled Tracy window (`t >= 20 s` from capture start), the inclusive
Avian `run_substep_schedule` block (all eight production substeps) measured
0.712 ms mean, 0.509 ms p50, 1.758 ms p95, 2.186 ms p99, and 4.444 ms max
(942 events). Its median is just above the 0.5 ms goal and its tail is higher;
this inclusive schedule block is not constraint arithmetic alone. The full
`PhysicsSchedule` measured 2.721 ms p50 and 3.676 ms p95 over 471 events, and
is not an Avian-only solver measurement. The last 240 `engine.frame_time`
samples were 13.449/18.774/25.341/40.839 ms p50/p95/p99/max (74.4 median
FPS). CPU `Main` measured 13.064/18.651/26.770 ms p50/p95/p99; GPU
`main_opaque_pass_3d` measured 1.606/1.727/1.756 ms p50/p95/p99. These
settled readings remain profiler- and contention-affected, not 150 FPS or
physics acceptance.

`sync_twin_overlays` ran 14 times across the capture, with a 5.746 ms maximum;
the earlier 261/278 ms app-thread stalls did not recur. This supports the
history-gap recovery fix for this run but does not establish clean frame
performance. The owned app accepted typed `Exit`, exited successfully, and
released ports 4317 and 8086. External ports 4101 and 4102 were left running.
Sandbox readiness and clean, uncontended FPS/physics acceptance remain
unverified.

The same capture's startup Tracy export (`t < 10 s`) found several substantial
main-thread projection costs. `process_usd_sim_prims` ran 43 times for 1.269 s
cumulative (31.371 ms p50, 34.420 ms p95, 63.589 ms max);
`process_usd_cosim_prims` ran 83 times for 0.941 s cumulative (29.777 ms p95,
33.633 ms max). `project_domain_islands` ran four times for 0.826 s cumulative
(297.593 ms max). Its 15 nested `domain_synthesizer_live` spans accounted for
0.751 s, including three `domain_network_read` spans of 136.6–138.8 ms. Tracy
does not identify the USD root for those long reads. The eight Modelica
solver-preparation commits were cache hits, so cold solver lowering does not
explain this warm-cache capture's startup cost. These spans identify CPU work
to investigate; their totals overlap where nested and remain profiler- and
contention-affected diagnostics.

### 2026-09-27 — Apollo readiness and joint-admission diagnostic

An unprofiled High-quality Apollo run used owned API port 4317 while external
simulator sessions on 4101 and 4102 were active. It is a contention-affected
diagnostic, not clean acceptance. `/api/health` responded 1.737 s after process
launch and the window-created event was at 1.801 s. The auto-opened Twin's
`StartupScene opened` event was 2.106 s after process launch; the live scene root
spawned 0.614 s after Twin-open, within its separate 2 s mount target.

Readiness was polled from startup. The early green `/api/ready` sample at
1.737 s from process launch regressed as scene work registered. The first
qualifying sample with `/api/ready` clear and `BindingStatus` clear was 8.708 s
after Twin-open. It stayed clear for 5.104 s across 94 samples, so this is a
measured transition that misses the 5 s target by 3.708 s. The scene-participant
lifecycle-ready event was at 7.488 s after Twin-open; a later route-ribbon
pending pair cleared before the qualifying sample.

The joint diagnostic showed 18 pending USD joints and one pending differential
at about 1.45 s after Twin-open. At about 6.47 s, all 18 joint wrappers were
waiting for Avian body-island admission; each endpoint was disabled and
kinematic, with no `SolverBody` or `BodyIslandNode`. They drained at about
7.42 s after Twin-open. This aligns the joint-admission wait with the late
participant-ready event, but does not establish that it is the only remaining
readiness cost. The app child ended before typed API `Exit` could be sent; its
process and ports 4317/8086 were verified closed. External 4101/4102 sessions
were left untouched.

### 2026-09-27 — post-index Apollo startup diagnostic

The Tracy-enabled production build included the per-read Modelica boundary
index. Its 15 `domain_network_boundary` spans fell from 424.034 ms total to
2.463 ms (99.4% lower); `domain_network_read` fell from 478.208 ms to
59.402 ms. `domain_synthesizer_live` fell from 837.700 ms to 395.118 ms, and
`project_domain_islands` from 918.333 ms to 479.580 ms. These are overlapping
CPU-zone totals from separate profiler-affected captures, not end-to-end time
savings or acceptance measurements.

The post-change capture is
`scripts/perf/captures/sss-domain-read-phases-after-index-20260927.tracy`
(15.69 s, 409 frames, 2,204,324 zones) with the matching app log. It used the
same High-quality Apollo Twin on owned port 4317 while external sessions 4101
and 4102 remained active. Startup readiness and `BindingStatus` were polled
every ~0.35 s, adding diagnostic work; treat the result as contention- and
profiler-affected. `/api/health` first responded 1.774 s after process launch,
the window-created event was at 1.797 s, and `StartupScene opened` was at
2.331 s. The scene root spawned 0.664 s after Twin-open.

The first qualifying full-readiness sample, with `/api/ready` and
`BindingStatus` clear, was 8.026 s after Twin-open and stayed clear for 5.026 s
across 14 samples. This misses the 5 s Twin-readiness target by 3.026 s. The
scene-participant lifecycle-ready event was at 6.750 s after Twin-open. The
app-health/window and scene-mount clocks met their diagnostic targets; they do
not substitute for the separate full-readiness target.

The app log shows initial physics-pose validation waiting on eight unresolved
joint-topology entities from 1.916 s to 5.956 s after Twin-open (about 4.04 s).
Physics admission completed at 6.741 s and scene participants became ready at
6.750 s. The startup trace recorded 10 `activate_dynamic_bodies` calls above
2 ms in its first 10 s, with a 7.711 ms maximum; the joint-admission batch
system itself stayed below 0.2 ms. These observations put joint/body topology
and activation on the critical path, but do not yet identify why those eight
topology markers remain unresolved for four seconds. Keep the topology and
initial-pose safety gates intact until that wait is attributed. The readiness
poller also observed late readiness work after physics admission, so joint
admission alone does not explain the full 8.026 s transition.

The owned app exited with code 0 after typed API `Exit`; ports 4317 and 8086
were verified free. External sessions 4101/4102 were left untouched. Sandbox
and clean, unprofiled acceptance were not measured.

### 2026-09-27 — post-index unprofiled readiness confirmation

A Tracy-featured production binary with no capture client attached ran the same
High-quality Apollo scene on owned API port 4317. External sessions 4101 and
4102 remained active. The readiness monitor queried `/api/ready` and
`BindingStatus` every ~0.35 s, so this is an unprofiled but polling- and
contention-affected diagnostic. Its log is
`scripts/perf/captures/sss-after-index-unprofiled-20260927.app.log`.

The window-creation log event was 1.208 s after process launch and `/api/health`
first responded at 1.418 s. `StartupScene opened` was at 1.489 s after launch.
The first qualifying sample with all readiness fields clear and no pending
binding or physics-admission work was 11.685 s after Twin-open; the state
stayed clear for 5.152 s across 14 samples. This misses the 5 s full-readiness
target by 6.685 s. This run did not timestamp the mounted scene root
independently; the preceding post-index capture measured it at 0.664 s.

The initial-pose validator waited on two unresolved joint-topology entities
from 2.604 s to 8.918 s after Twin-open (6.314 s). The log exposed only the
transient ECS IDs `2506v0` and `2502v0`, not their authored USD paths. The
binding monitor later observed 18 pending native joint admissions; physics
admission completed at 10.178 s after Twin-open and scene participants became
ready at 10.204 s. The route ribbon and geometry were incrementally projected
at 10.667 s after Twin-open; readiness then briefly regressed to two late
items before the qualifying sample. The joint-topology hold and later scene
work both need attribution before another gate change.

During the five-second soak, the authored escape policy logged a body exiting
the world bounds and paused seven connected bodies. This readiness run does
not establish physics correctness, solver time, or FPS. The app accepted typed
API `Exit`, exited with code 0, and released port 4317; external 4101/4102
sessions were left untouched.

### 2026-09-27 — joint-topology cache profile and strict Twin readiness

The production build without the Tracy feature ran High-quality Apollo on
owned API port 4317. The window-created event was 0.803 s after process launch,
`/api/health` responded at 0.861 s, and auto-opened `StartupScene opened` was
1.059 s after launch. These app window/API timings are within the current 2 s
app-ready target. The full Twin target is separately measured within 5 s from
Twin-open; its clock does not start at process launch or at scene mount.

The strict readiness gate included scene-participant lifecycle readiness,
every readiness producer's initial registration, zero pending readiness items,
and clear `BindingStatus` (`wait_open=false`, `dirty=false`, no awaiting,
pending joints, admissions, differentials, models, or holds). All 18 revolute
joints were admitted and 112 connections were present. The first sample meeting
the full gate was 5.797 s after Twin-open and stayed clear for a 5.01 s soak.
This misses the Twin target by about 0.80 s. Participant lifecycle readiness
arrived at 4.724 s; at 5.087 s, a deferred RouteRibbon material and USD
connection binding were still pending. The live scene root was not timestamped
independently in this run; the latest separate scene-mount measurement remains
0.664 s in the prior post-index capture.

The late readiness item provides the strongest current startup lead:
`assets/scenarios/route_follow.rhai::on_visualization` calls
`waypoint_editor::ensure_ribbon`, which creates a transient referenced
`/Traverse/Route/RouteRibbon/Looks/RouteRibbon_Mat` prim after startup scene
projection has begun. Attribute the late prim, material residency, and
connection-binding registration before changing the readiness or physics
admission gates.

The initial-pose wait log records transient entity IDs without their authored
joint paths. `project_pending_joint` now logs the USD path and entity when it
installs `PhysicsJointTopologyPending`, so the next startup capture can map the
wait to authored joints. `cargo check -j 4 -p lunco-usd-avian` passed; the
current production binary predates this diagnostic and must be rebuilt before
using that mapping.

The matching Tracy diagnostic capture,
`scripts/perf/captures/sss-joint-topology-cache-fix-apollo-20260927.tracy`,
ran for 60.3 s (4,053 frames, 22,105,149 zones) with external simulator
sessions on ports 4101 and 4102 active. It is contention- and
profiler-affected diagnostic evidence, not acceptance. The
`JointTopologyIndex` now reuses composed joint and wheel-attachment facts by
canonical stage generation and invalidates on stage-asset events; ECS prim
projection revisions no longer trigger repeated full-stage scans. During the
first 10 s,
`process_usd_sim_prims` system self time fell from 1,347.650 ms in the prior
capture to 141.153 ms (about 89.5% lower total self time); the longest call
remained 62.036 ms. `sync_twin_overlays` took 0.210 ms total across 14 calls,
with a 0.0464 ms maximum, so the earlier 261/278 ms stalls did not recur.
Other startup costs remain in `process_usd_cosim_prims` (1,104.868 ms total
self time across 79 calls) and `rewire_usd_connections` (176.185 ms across 43
calls, 141.815 ms maximum). These overlapping system spans are leads, not
end-to-end savings.

In the settled Tracy window, `PhysicsSchedule` self time was
0.3629/0.483/0.5825/0.8893 ms p50/p95/p99/max; this is not an Avian-only solver
measurement. `run_substep_schedule` self time was
0.0226/0.0395/0.0516/0.1242 ms, also not the full solver block. GPU
`main_opaque_pass_3d` was 1.614/1.751/2.853 ms p50/p95/p99 (3.759 ms max).
The capture and concurrent workloads affect these diagnostics; clean FPS and
full physics-step acceptance remain unmeasured. The `PhysicsTotalDiagnostics`
step time is the authoritative whole-step number; solver subzones help locate
its cost.

The no-Tracy run logged a renderer slab-allocator use-after-free error after
readiness, with 15 repeated instances suppressed. Track that separately from
the readiness miss. The app exited successfully through typed API `Exit` and
released port 4317. External ports 4101 and 4102 were left untouched; sandbox
readiness and uncontended acceptance were not measured.

### 2026-09-27 — follow-up exposure and physics schedule spans

The Tracy capture `scripts/perf/captures/sss-visualization-hook-boundary-20260927.tracy`
ran for 40.32 s with external simulator sessions on ports 4101 and 4102 active,
so these results remain profiler- and contention-affected diagnostics. In the
settled window, the inclusive eight-substep `run_substep_schedule` span measured
1.339/1.865/2.224/3.654 ms p50/p95/p99/max across 934 calls. This is schedule
latency including nested systems and dispatch, not exclusive solver CPU time.
Because this substep block alone exceeds 0.5 ms, the full physics-step target
remains unmet. A clean run must record the `PhysicsTotalDiagnostics.step_time`
percentiles and use solver zones to attribute its cost.

`publish_exposure` ran 535 times and accounted for 944.1 ms of self time over
the capture. In the settled window it ran 333 times at
1.637/2.346/2.862/4.339 ms p50/p95/p99/max. It is a bounded, change-driven UI
publisher rather than per-frame work, so attribute its branch cost before
changing its cadence or removing facts. The Main schedule was
13.056/17.897/21.398/31.066 ms p50/p95/p99/max in the settled window, above the
6.67 ms frame budget for 150 FPS. The GPU opaque pass median was 1.613 ms. The
capture averaged about 60.7 FPS, which is diagnostic and not production
acceptance.

In this same capture, `process_usd_cosim_prims` ran 88 times, all in the first
10 s. Its call distribution was 0.016/35.384/36.046/37.526 ms p50/p95/p99/max,
with 1,241.2 ms total self time. Source review found that each admission batch
rebuilt a local Modelica-network membership set by scanning the composed stage,
while another consumer already uses the shared stage-generation-and-instance
cache. The current working-tree change makes participant admission use that
same cache and instance key, and adds spans to separate index construction
from per-prim reads. The post-change production build passed, and the follow-up
capture below shows a large diagnostic reduction in this system. The two
captures differ in duration and workload, so this is not a controlled A/B; the
trace also does not attribute the full system cost to membership scans alone.

### 2026-09-27 — shared Modelica membership-cache follow-up

The Tracy-featured production build passed. The follow-up capture,
`scripts/perf/captures/sss-cosim-membership-cache-postfix-20260927.tracy`, ran
for 20.32 s (1,070 frames; 5,775,251 zones). The profiler and external
simulator workloads on ports 4101 and 4102 were active, so treat these timings
as diagnostics rather than clean performance acceptance.

`process_usd_cosim_prims` ran 74 times, all within the first 10 s. Its event
distribution was 0.053/0.283/0.458/0.892 ms p50/p95/p99/max, with 6.736 ms
total self time. The prior capture recorded 88 calls, 35.384 ms p95, and
1,241.2 ms total self time. The new
`usd_cosim_membership_index_build` span ran twice (7.104 µs total; 4.168 µs
max). The `usd_cosim_process_prim_read` span ran 804 times (1.776 ms total
self time; 0.001/0.003/0.014/0.101 ms p50/p95/p99/max). These results support
the shared-cache change as a strong startup lead, but the captures have
different durations and workloads and do not establish a clean A/B result.

The app log records physics admission complete and scene participants ready
about 4.2 s after Twin-open. The API health check succeeded and a later
`/api/ready` sample was green, but full readiness was not sampled continuously
through the transition and five-second soak. This run therefore does not
verify either readiness target. The app log also records a renderer
slab-allocator use-after-free after readiness; investigate it separately from
this startup timing result.

### 2026-09-27 — local gravity provider-pose reuse

Source review found that `compute_local_gravity` recomposed a shared gravity
provider's world pose for every moving consumer. The system now caches each
provider pose by entity for one invocation and clears the scratch map before
the next active pass. No pose survives a FixedUpdate or crosses scene state.

The matched headless, maximum-speed Apollo captures are
`scripts/perf/captures/sss-gravity-body-pose-cache-baseline-20260927.tracy`
(30.34 s; 5,264,813 zones) and
`scripts/perf/captures/sss-gravity-body-pose-cache-20260927.tracy` (30.35 s;
5,120,718 zones). Both used the same scene, command line, and profiler window;
other simulator sessions on ports 4101, 4102, and 4147 remained active.
In the settled windows (`t >= 10 s`), `compute_local_gravity` fell from
0.2767/0.4018/0.4798/0.9324 ms to 0.2108/0.3186/0.3894/0.5542 ms
p50/p95/p99/max across 2,037 and 2,010 calls. Mean time fell from 0.2954 ms
to 0.2264 ms (23.4%). This is a same-mode diagnostic A/B under CPU
contention, not clean acceptance or an FPS result.

The full `PhysicsSchedule` stayed near 1.9 ms p50 (1.8487 ms baseline;
1.9444 ms after), and Avian `update_narrow_phase` was 0.2938/0.4713 ms
p50/p95 before and 0.2940/0.4531 ms after. This local-gravity optimization
reduces environmental work but does not establish the 0.5 ms physics target.
Raw Tracy event exports for the settled eight-substep
`run_substep_schedule` block measured p50/p95/p99/max of
0.7336/1.1367/1.3252/1.8291 ms before and
0.7859/1.1561/1.3417/1.8049 ms after (2,038 and 2,010 events, respectively;
nearest-rank percentiles at `t >= 10 s`). That full production substep block
alone exceeds the 0.5 ms physics-step target; these profiler- and
contention-affected captures are diagnostic only.
The headless run opened the Apollo Twin 0.236 s after the process-start log
and completed physics admission and scene-participant startup about 4.46 s
after process start (4.22 s after Twin-open). `/api/ready` was sampled only
later, without a recorded transition or five-second soak. Both runs exited
through typed API `Exit`, and owned ports 4317 and 8086 were released.

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

### 2026-09-27 — symmetric narrow-phase filter lookups

`UsdCollisionFilter::filter_pairs` now checks the collider and body entities on
one endpoint. Both authored filtered-pair resolution and live-joint filtering
store each pair on both endpoints, so the reverse-side lookups repeated the
same component queries and set membership checks. The focused
`cargo check -j 4 -p lunco-usd-avian-filters` passed. This source change has not
been production-profiled; the active sessions on ports 4101, 4102, and 4147
were left untouched. The `update_narrow_phase` values above predate this edit
and cannot establish a runtime gain or the 0.5 ms target.

### 2026-09-27 — immediate no-Tracy Twin-readiness polls

Two current-source production runs used `target/debug/luncosim` without Tracy
or UI, with `--no-vsync --no-throttle --log-diag`, on owned API port 4317.
Readiness and binding status were polled from process start, and each run
exited through typed API `Exit`. The two tutorial sessions on ports 4101/4102
and the Griffin workload on 4147 were active, so these are CPU-contended
headless diagnostics. They cannot establish window creation, High-quality FPS,
or clean readiness acceptance.

For Apollo, `/api/health` responded 0.261 s after process launch and
`StartupScene opened` was logged at 0.349 s. The composed root spawned 0.648 s
after Twin-open. Physics admission and scene-participant lifecycle readiness
were logged 4.124 s after Twin-open. The first sample meeting the strict full
readiness gate was 5.594 s after Twin-open, then remained clear for 5.05 s; it
missed the 5 s Twin target by 0.594 s. An earlier green `/api/ready` sample
regressed as scene waits registered.

For sandbox, `/api/health` responded 0.242 s after process launch and
`StartupScene opened` was logged at 0.325 s. The composed root spawned 0.721 s
after Twin-open. Physics admission and scene-participant readiness arrived
15.605 s after Twin-open, and the first strict full-readiness sample arrived
at 16.138 s and stayed clear for 5.048 s. This misses the 5 s target by
11.138 s. The terrain initial-pose validation log shows a 14.30 s wait on
unresolved joint topology; Modelica lifecycle/solve-preparation completions
also continued during this interval. The run does not isolate which of these
gates the final readiness transition. The binding monitor reported 32 pending
joint admissions around the time physics admission completed.

At that revision, `PhysicsPerformance` returned HTTP 500 in both headless runs,
so they provided no whole-Avian-step samples. `PhysicsTotalDiagnostics` was
installed only by the UI plugin, which headless mode omits; the generic query
then had no timing resource. `ReadExposures` likewise provided no engine-health
timing values. The shared diagnostics owner and current headless measurements
are recorded in the follow-up below.

### 2026-09-27 — shared headless Avian timing history and per-scene diagnostic

`UsdAvianPlugin` now installs Avian's total-step and physics diagnostics for
both UI and headless hosts; the editor performance bridge only publishes the
current timing to `engine-health.physics_step_ms`. `PhysicsPerformance` reads
the retained `PhysicsTotalDiagnostics.step_time` history from Bevy's
`DiagnosticsStore` and exposes the most recent 120 samples as
`step_time_samples_ms`. The production Rhai scenario
`physics_initialization_policy` now asserts that current step timing and
history are available in a headless scene. The production build passed, as did
`LUNCOSIM_BIN=/home/rod/Documents/luncosim-workspace/optimization/target/debug/luncosim ./scripts/run_scene_tests.sh --no-build --exact physics_initialization_policy`
(6 ticks, 0.10 simulated seconds).

Two sequential no-Tracy production runs used this checkout's binary with
`--no-ui --no-vsync --no-throttle --log-diag` and owned API port 4317. Tutorial
simulator sessions remained active on ports 4101 and 4102, so these are
CPU-contended headless diagnostics. The runs queried `PhysicsPerformance` once
after scene startup; each response contained 120 retained step samples:

- Apollo (`traverse_apollo15.usda`): at 10.328 s after process launch,
  p50/p95/p99/max were 0.419/0.629/1.005/1.091 ms (mean 0.448 ms).
- Sandbox (`assets/scenes/luncosim/sandbox_scene.usda`): at 18.290 s after
  process launch, p50/p95/p99/max were 0.643/0.991/1.359/5.453 ms (mean
  0.735 ms).

Both distributions exceed the 0.5 ms goal at p95; sandbox also exceeds it at
p50. These timings are contention-affected and headless, and do not establish
clean physics acceptance or graphics FPS. A final `/api/ready` snapshot was
clear after each measurement delay, but readiness was not polled continuously,
so these runs do not establish the app or Twin readiness transition. Both
owned sessions exited through typed API `Exit` and released port 4317; ports
4101 and 4102 were left untouched.

### 2026-09-27 — settled CPU spans from the current Apollo Tracy capture

The existing capture
`scripts/perf/captures/sss-current-apollo-20260927.tracy` used a Tracy-enabled
debug, windowed, High-quality Apollo run on the RTX 5060 Laptop GPU. External
simulator sessions on ports 4101 and 4102 were already active, so these are
contention- and profiler-affected diagnostics. The app log
`scripts/perf/captures/sss-current-apollo-20260927.app.log` records process
start at `03:22:01.456Z`, window creation at `03:22:03.151Z` (1.695 s), and
`StartupScene opened` at `03:22:03.405Z` (1.949 s). This timestamps window
creation and Twin-open, not API health, mounted scene root, or full readiness.

`tracy-csvexport -u -f 'schedule{name=PhysicsSchedule}'` over the settled
portion (trace time >= 10 s) reports inclusive CPU spans of
2.016/2.587/2.945/3.920 ms p50/p95/p99/max (1,730 calls). The nested
`run_substep_schedule` system span was 0.987/1.344/1.535/2.026 ms
p50/p95/p99/max. These overlapping Tracy spans are diagnostic attribution,
not the authoritative Avian `PhysicsTotalDiagnostics.step_time` measurement
and must not be summed.

The GPU export contains events only from 2.429–5.212 s after launch, with no
settled GPU-pass window; it cannot establish the 150 FPS target. The separate
headless API samples above also exceed the 0.5 ms goal at p95. Clean, windowed
FPS and whole-step physics acceptance remain open.

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
  diagnostic tail was 92–147 FPS with 7.3–10.9 ms frame samples. The sandbox's
  first `/api/ready` green sample arrived in 2.8 s. These runs remain
  non-acceptance evidence and do not establish full Twin readiness.
- On the post-change production vehicle run (binary rebuilt after syncing
  `d7ad4ad0f`), the shared listing was scheduled at `01:56:35.675886Z`, the
  scene was spawned at `01:56:36.732963Z`, and scene participants were ready at
  `01:56:41.677245Z`. This is approximately 8.88 s from process start and is
  evidence that catalog enumeration no longer blocks the scene schedule, not a
  claim that the then-current 2 s loading target is met.
- The final rebuilt binary also completed a headless vehicle smoke: the shared
  listing was scheduled at `02:43:32.324864Z`, the scene spawned at
  `02:43:32.547703Z`, and participants were ready at `02:43:36.320508Z`.
  The run was terminated by its 10 s verification timeout after readiness; the
  port was free afterward. This confirms the new catalog path on the rebuilt
  binary, but the approximately 5.01 s process-to-readiness interval exceeded
  the then-current 2 s target.
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
- 2026-09-26, after the binding-revision and shared Modelica-membership changes,
  a fresh Tracy-enabled Apollo run used API 43132, X11, the authored
  `traverse_apollo15.usda` scene, and `--no-vsync --no-throttle`. Its 12.29 s
  startup capture is
  `summer-space-school-shared-membership-cache-optimization-20260926.tracy`
  (347,435 zones). `/api/ready` still had 10 pending items at 12.3 s and became
  ready at about 21 s from process launch. During this profiler run and active
  concurrent Griffin workloads, the 240-sample `SimulationTimingProfile`
  reported whole fixed-tick service p50/p95/p99/max of 6.66/8.94/10.92/13.09
  ms, with six lifetime budget exceedances; fixed-loop service was
  6.44/9.07/11.13/12.82 ms. Shared `engine.frame_time` history was
  26.93/81.76/108.71/123.45 ms. These are contention- and instrumentation-
  affected diagnostics, not clean startup or FPS acceptance; fixed-tick service
  is not Avian-only physics time. The post-change capture could not be decoded
  within a bounded CPU window, so post-change per-system counts remain
  unverified. The owned session exited through the API and ports 43132/8086
  were verified free; other sessions were left untouched.

### 2026-09-26 — borrowed manual-hold lookup in propagation

The fixed-step propagation path previously cloned the complete active
`PortHolds` snapshot, then allocated a cloned target-name key for every lookup.
`PortHolds` is indexed by entity and port and advances a revision only when an
intent actually changes. Propagation resolves borrowed names into a reusable
target-aligned value buffer when that revision or compiled wiring changes; the
steady physics tick reads the buffer by target index. The presentation snapshot
remains available to the port inspector. The recorded pre-change trace
attributed 18.9 ms of self time to `propagate_connections` across 103 calls;
this is a narrow cleanup, not an explanation for the overall FPS gap.

### 2026-09-26 — resolve readable input sources and retain propagation scratch

Readable `inputs:*` sources resolve independently from write-target ownership,
preserving same-named input/output causality. Modelica and control maps, Avian,
authored output maps, scene-property sinks, shader inputs, and catalogued
celestial-link outputs now resolve their wired endpoints to process-local slots.
The link reader reduces peers directly by class instead of constructing a
temporary class map per source read. Fixed propagation and rollback share
retained accumulator, hold, and diagnostic staging buffers; successful targets
carry a compiled index through the hot pass and share their name only when
updating the once-per-endpoint ledger. Terminal diagnostics share names as
`Arc<str>`, and warning keys are formatted only on first fault. This is a
source-level architecture change, not a measured FPS claim; clean post-change
tests and profiling are still required.

Shader-driven uniforms use predeclared `PortMap<Option<ParamValue>>` slots. The
fixed propagation path resolves a shader input once and updates its live slot
directly; topology fingerprints are cached by authored setters, not rebuilt by
hashing parameter names when a sample changes. This removes the ordered-set
slot walk and name-based live-value update from that path. It is source-level
evidence only; no FPS gain is claimed without a clean run.

### 2026-09-24 — conservative spotlight shadow relevance

`lunco-render-bevy` now disables only the extracted shadow-map flag for a
spotlight whose conservative finite-frustum bound misses every extracted 3D
camera on compatible render layers. Missing or malformed bounds and boundary
contacts retain the shadow map. Authored lights, direct illumination, shadow
resolution, and the High quality preset are unchanged. This removes irrelevant
shadow-view preparation after extraction; Bevy's main-world per-light caster
visibility work remains in place.

- Six focused shadow-relevance unit tests passed. The regular and
  Tracy-enabled production binaries both built, and the High-quality Apollo
  scene reached `/api/ready` with no pending work. Both owned sessions shut down
  through typed API `Exit`; their ports were released.
- The unprofiled diagnostic tail varied from 45–132 FPS and 7.6–22.1 ms per
  frame while other simulator sessions were active, so it is contention-affected
  and is not a clean FPS comparison.
- Tracy capture:
  `scripts/perf/captures/apollo-high-spotlight-20260924.tracy` (20.29 s, 518
  profiler frames, 3.82 million zones, 139.12 MB). The full-trace CSV exporter
  was stopped after several minutes at one low-priority core; no zone
  attribution or performance delta is claimed from this capture.
- The Tracy-run Bevy diagnostics still provide pass-level context (not a clean
  acceptance result): its final samples reported 10.4–15.8 ms frame times,
  while `main_opaque_pass_3d` was 1.66–1.69 ms GPU and 0.06–0.08 ms CPU. Its
  pipeline counters were stable at 509,746 clipper invocations and 411,695
  output primitives. Thus the opaque pass alone cannot explain the observed
  frame time; CPU work outside the instrumented render passes, waits, or other
  passes remain to be attributed. These measurements were collected under
  contention and must not be treated as a product FPS baseline.
- Source inspection of Bevy 0.19.1 found that
  `check_point_light_mesh_visibility` scans eligible mesh casters for each
  shadow-enabled point/spot light and rebuilds sorted per-light caster lists
  before extraction. The spotlight relevance adapter above runs after
  extraction, so it cannot currently skip that main-world work. This is a
  profiling lead, not an established hotspot: inspect this system and
  `prepare_lights` in the saved capture before changing lifecycle/order or
  shadow behavior.

This change preserves authored visual quality by construction, but a settled,
uncontended before/after FPS comparison and detailed Tracy zone inspection are
still required; the 150 FPS, sub-0.5 ms physics, and 5 s full-Twin readiness
targets remain open. Other simulator
sessions and an unrelated Cargo build were left untouched.

### 2026-09-25 — point-light cubemap shadow views

The Rhai-reuse capture has eight camera-schedule roots on each of 2,022 settled
frames: one primary 3D camera, one additional camera schedule with no `Core3d`
zones, and six auxiliary light-shadow views. The trace does not identify the
auxiliary views' owning lights; six roots are consistent with a point-light
cubemap, but that mapping needs runtime confirmation. Bevy's `camera_driver`
still spends a mean 3.36 ms per root frame across all view schedules, while
each auxiliary schedule costs about 0.28–0.45 ms median. On the render thread,
after excluding the first 14 seconds, the settled `Render` schedule is 8.02 ms
median / 10.11 ms p95 (2,022 samples); `RenderGraph` is 4.55 / 5.90 ms. These
are distinct thread/schedule spans, not costs to add together. `Render` alone
exceeds the 6.67 ms budget for 150 FPS.
The existing local-light filter only handled spotlights, despite already
running immediately before Bevy's `prepare_lights` on extracted render-world
data.

The filter now checks point-light range spheres as well as conservative
spotlight-cone bounds against every active extracted 3D camera and compatible
render layer. It disables only the extracted shadow-map flag when no output can
contain any geometry inside the light's influence volume. Invalid/non-finite
bounds and tangent camera intersections retain the authored shadow path. This
preserves direct lighting, authored settings, and all potentially visible
shadows. Focused geometric tests cover visible and disjoint point-light ranges.
The subsequent 30.32 s Apollo trace (470 profiler frames, recorded with other
simulator sessions active) retained the same six auxiliary shadow roots; no
Apollo shadow-root reduction or FPS gain is demonstrated. The trace is
diagnostic only, and a settled uncontended FPS run plus production visual
comparison remain outstanding. This makes the render-thread camera/view work,
not this point-light filter, the next measured target.

### 2026-09-24 — Summer Space School settled-frame profile

- Captured the production debug build from this checkout with Tracy enabled,
  High quality, `--no-vsync`, and `--no-throttle`, using the real
  `traverse_apollo15.usda` scene and API port 43442. The 40.33 s capture is
  `scripts/perf/captures/sss-optimization-20260924.tracy` (1,438 Tracy frames,
  11,219,291 zones, 285 MB). API readiness drained to zero pending items; the
  owned run exited through typed API `Exit`, and port 43442 was released.
- Session 43122 remained active throughout and was not inspected or changed.
  Consequently neither this trace nor the following FPS window is an
  uncontended acceptance result.
- Tracy self-time aggregates for recurring systems showed approximately
  0.63 ms/call for `drain_world_scripts` (1,437 calls), 0.58 ms for BigSpace
  `propagate_high_precision_channeled` (1,382), and 0.56 ms for Bevy's
  `check_point_light_mesh_visibility` (1,437). `check_dir_light_mesh_visibility`
  plus its command application accounted for another 0.82 ms/call combined;
  GPU preprocessing bind groups were 0.50 ms and GPU clustering preparation
  0.38 ms per call. These distributed CPU costs, not one dominant opaque pass,
  are the main measured render/update leads. The Tracy-run opaque GPU pass
  averaged about 2.02 ms; this is diagnostic only.
- `drain_world_scripts` is an exclusive Update system called every frame; its
  source drains `PendingWorldScripts` and returns immediately when the queue is
  empty. Its measured recurring cost makes avoiding the idle exclusive-system
  pass a high-priority A/B candidate. Bevy `Core3d` ran 9,547 times for 1,437
  root frames; the camera driver also runs auxiliary shadow-map views, so this
  count does not mean there are 6.6 output cameras.
- A separate, non-Tracy production run with the same scene and quality gathered
  16 post-readiness one-second diagnostic samples: mean 104.7 FPS (58.4–151.9),
  frame time 10.46 ms (6.78–17.13), and Avian step time 0.89 ms (0.73–1.08).
  The existing session still running makes these contention-affected, not a
  clean acceptance measurement. The 150+ FPS target remains unverified.

### 2026-09-25 — bounded worker pools and CPU render-path attribution

- Bevy's default task-pool budget was implicitly sized from 32 logical CPUs on
  this 16-core machine. The native API and asset listeners also each created a
  32-worker Tokio runtime despite already owning dedicated OS threads. Bevy is
  now capped at physical-core count; native transports use current-thread
  runtimes. The app process fell from 97 threads to 42. This reduces duplicate
  schedulers without changing render quality or simulation cadence.
- A fresh High-quality Apollo run used the production binary, the authored
  `traverse_apollo15.usda`, and API port 43599. Readiness drained to zero. Over
  a settled 29-second window, `frame_count` advanced by 3,391: **116.9 FPS**;
  the rolling frame-time diagnostic stayed around **8.4–8.9 ms**. This is
  better than the prior 10.46 ms sample, but remains below 150 FPS. A single
  host load-average snapshot was 3.2 and there was no other `luncosim` process.
  GPU SM utilization
  sampled 66–75%, so neither an uncontended GPU ceiling nor a CPU-only limit is
  established from utilization alone. The owned run exited through typed API
  `Exit`, and port 43599 was released.
- The separate Tracy diagnostic capture is
  `scripts/perf/captures/sss-physical-pool-single-api-tracy-20260925.tracy`
  (45.37 s, 2,282 profiler frames, 13.41 million zones, 294.23 MB). Its traced
  frame rate was only about 50/s, so none of its frame-rate numbers are product
  measurements. The exported GPU timestamp categories account for about
  2.5 ms/frame: 1.66 ms opaque, 0.26 ms bloom, 0.26 ms MSAA writeback, 0.17 ms
  clustering, and 0.18 ms tonemapping, upscaling, and other small passes. This
  is not the full GPU frame time: Bevy's shadow render passes have CPU Tracy
  spans but no corresponding GPU timestamp zones in this capture. Do not
  subtract 2.5 ms from the clean frame to infer a CPU-only remainder.
- CPU self-time attribution is distributed: Bevy `Render` averaged 3.67 ms
  (95th percentile 4.52 ms), `PostUpdate` 3.12 ms (p95 4.24 ms), and `Update`
  1.65 ms (p95 3.54 ms). These schedule spans overlap worker activity and must
  not be added as a serial frame-time budget. Recurrent leaf costs include
  `prepare_preprocess_bind_groups` at 0.51 ms/frame; directional shadow-caster
  visibility at 0.13 ms in the system plus 0.44 ms applying its deferred
  visibility commands; point-light visibility at 0.19 ms/frame; and workbench
  rendering at 0.25 ms/frame.
- The strongest single render-thread lead is Bevy's `queue_submit` zone:
  about 1.28 ms/call, with the trace recording 48 pending command buffers on
  each of roughly 2,208 submissions. Source confirms this is the call to
  `wgpu::Queue::submit`; the trace does not yet distinguish driver backpressure
  from CPU submission overhead, so this is a measured target, not a proven
  cause or a fix. Pass markers show six spotlight shadow maps plus four
  directional-light cascades each profiler frame, alongside one main opaque
  camera pass. `Core3d` ran 6.78 times per profiler frame; that count is neither
  the number of active output cameras nor the total number of shadow maps. The
  existing camera-scoped GPU-culling path is already active and intentionally
  leaves mesh/light shadow visibility on CPU.
- CPU shadow work is material: directional caster visibility used 0.13 ms in
  the system plus 0.44 ms applying deferred visibility updates, and point/spot
  caster visibility used 0.19 ms per frame. These paths traverse shadow
  casters for light views; their cost is distinct from recording the shadow
  draw passes and from GPU execution.
- The authored Apollo scene has one rover without headlight references, but the
  Twin opts into `usd.runtime_persistence = true`. Startup restored
  `.lunco/runtime/sim/scenes/traverse_apollo15.usda`, whose runtime layer adds
  two Rocker-Bogie rovers, one Ackermann rover, a probe, and two balls. A live
  `QueryUsdPrim` confirmed that all six headlights on those extra rovers are
  shadow-enabled `SphereLight`s (120,000 lumens, 90 m range). These are exactly
  the six spotlight shadow passes in Tracy. The live API reported 788 registered
  entities, and the current non-Tracy diagnostic run showed a rolling
  10.35 ms/frame (~104 FPS); this was inventory evidence, not a separate clean
  acceptance window. The overlay is persisted Twin state, not a loader leak; I
  left the sidecar unchanged.
- `SceneCameraAudit` found five camera candidates: the Avatar window camera is
  the only active camera; the three spawned-rover cameras and the 16×16 USD
  preview camera are inactive. The main opaque pass also ran once per Tracy
  frame. Rendering several full output cameras is therefore not the root cause.
- Preserve the persisted overlay and all six headlight shadows while testing
  camera-only stage admission on auxiliary roots. Measure the 48-buffer submit
  boundary and shadow-pass cost before considering any further render-path
  change; the existing GPU timestamp table does not cover shadow passes.
  Do not infer the full GPU/CPU split from that incomplete table. The 150 FPS
  target remains open.

### 2026-09-25 — auxiliary shadow-view stage admission

Bevy's camera driver runs `Core3d` for point/spot shadow roots as well as output
cameras. `lunco-render-bevy` now gates `Prepass`, `MainPass`, `EarlyPostProcess`,
and `PostProcess` stage sets off for `LightEntity` roots. Source inspection of
Bevy 0.19.1 confirms its early/late shared and per-view shadow passes are
separate systems outside those stage sets, so the depth-map path remains active.
The shared `prepare_preprocess_bind_groups` pass remains active too: it builds
view-specific bindings for each phase's work-item buffers and is needed by GPU
preprocessing/culling for shadow draws. The measured pre-change cost was about
0.514 ms/frame; this app-level gate does not bypass or cache it.

The same pre-change Tracy capture recorded one `wgpu::Queue::submit` call per
frame, with 48 pending command buffers, at 1.277 ms mean (about 1.936 ms p95).
The new gate may prevent camera-only systems on shadow roots from contributing
buffers, but it cannot remove the shared queue call. Post-change capture
`scripts/perf/captures/sss-shadow-schedule-after-20260925.tracy` contains 45.31 s,
861 profiler frames, and 5.32 million zones. The bundled CSV exporter did not
finish its first filter after 15 minutes and was stopped; the capture is
preserved, but no post-change submission count/time or schedule delta is
claimed.

The production UI binary built with the change, and the focused
`camera_stages_are_skipped_only_for_light_shadow_roots` test passed. A headful
High-quality Apollo run reached `/api/ready` with no pending work, and its
2560×1568 API screenshot showed the scene rendering. Under the two pre-existing
simulator sessions on ports 43117 and 43126, its diagnostics averaged about
89.9 FPS / 12.37 ms per frame and 1.05 ms per Avian step. Those sessions were
left untouched; this is contention-affected and not an acceptance comparison.
The separate unprofiled run and Tracy run both exited through typed API `Exit`,
and owned ports 43602, 43603, and 8091 were verified free. Clean post-change FPS,
buffer-count reduction, shadow pass inventory from the new capture, and the 150
FPS target remain unverified.

A later owned non-Tracy Apollo diagnostic run on API port 43604 reached
`/api/ready` with zero pending work and exited through typed `Exit`. It ran
while the existing sessions on 43117 and 43126 were still active; immediately
before launch, host load averages were 10.36/11.87/8.84 and NVIDIA GPU
utilization was 78%. Its rolling diagnostics reported 63.2 FPS / 18.67 ms,
with individual samples as low as 32.9 FPS / 30.40 ms; Avian averaged 1.13 ms
per step. This is severe contention evidence only, not a post-change acceptance
comparison or a clean estimate of the gate's effect. Port 43604 was released;
the other sessions were untouched.

### 2026-09-25 — Bevy render-queue submission boundary

Source inspection of Bevy 0.19.1 separates two queue submissions that are easy
to conflate. The CorePipeline render-graph `Submit` stage drains all
`PendingCommandBuffers` in one `wgpu::Queue::submit`; the pre-change Tracy zone
measured that call with 48 buffers. Bevy's outer `render_system` then always
creates a second encoder, asks the screenshot and GPU-readback subsystems to
append any pending copies, finishes it, and calls `Queue::submit` again. When
neither subsystem has pending work, those helpers append no commands, but the
empty encoder is still submitted. That second call is outside the measured
`queue_submit` span, so its cost is currently unknown.

The local shadow-root stage gate reduces camera-only Core3d work before the
batched graph submit. Safely removing the unconditional screenshot/readback
submission is a Bevy render-system ownership change: replacing Bevy's wrapper
from the application would also take ownership of frame presentation,
screenshot capture, and GPU readback ordering. Do not fork or duplicate that
wrapper in LunCoSim. A focused upstream change should submit the secondary
encoder only when screenshot/readback work was actually recorded, with
production coverage for idle frames and both request paths. First measure the
second call separately and keep the current post-change FPS and buffer-count
comparison open; source inspection alone does not establish a frame-time win.

### 2026-09-25 — directional caster-visibility scratch lifetime

The pre-change trace attributed 0.13 ms/frame to Bevy's
`check_dir_light_mesh_visibility` and another 0.44 ms/frame to applying its
deferred `ViewVisibility` updates. In Bevy 0.19.1, the visibility system
collects entity IDs in a `Local<Parallel<Vec<Entity>>>`, then `mem::take`s that
scratch into a `Commands` closure. The closure marks those entities visible
after the parallel system completes; the moved per-thread vectors are dropped
after application, so their capacity is not available to the next frame. This
matches the owner source's TODO to avoid the per-frame allocation.

The safe optimization belongs in Bevy's light-visibility owner: retain the
per-thread entity buffers in reusable storage through deferred application,
while preserving the current parallel caster tests and the ordering before
`MarkNewlyHiddenEntitiesInvisible`. Do not skip shadow visibility or cache
results in LunCoSim without a complete invalidation contract. The measured
0.44 ms is a target, not a promised saving; compare exact visible-caster sets
and frame time after an upstream implementation.

### 2026-09-25 — GPU-cluster preparation allocation lifetime

The September 25 production logs confirm that this Vulkan device selects
Bevy's GPU-clustering path. In Bevy 0.19.1,
`prepare_clusters_for_gpu_clustering` constructs fresh per-view
`ViewClusterBindings` and `ViewGpuClusteringBuffers` during render preparation,
reserves their working storage, and queues both components onto the view.
`upload_view_gpu_clustering_buffers` then writes those buffers, and
`prepare_clustering_bind_groups` creates four bind groups for each clustered
view on every render frame. The cluster contents do change with lights and
view state, but buffer capacity and bindings need not be discarded merely to
refresh that content; the owner can retain storage and rebuild bindings only
when their underlying buffer handles or layouts change.

The older Tracy table's `prepare_clusters` zone averaged 0.975 ms with one
approximately 39 ms outlier, but the zone has not yet been mapped conclusively
to this exact Bevy system. Treat this as a high-value source-level lead, not an
attributed saving. No `Resizing the view clustering ...` warning appears in
the checked September 25 Apollo logs, so increasing initial capacities alone
does not address the observed steady-state allocation path. The correct fix is
in Bevy's GPU-clustering owner; do not replace clustering with the CPU path or
alter cluster quality from LunCoSim. A new focused trace must identify the
exact system, and validation must compare lighting/cluster output as well as
allocation count and frame-time percentiles.

The current Bevy `main` source was checked on September 25 and still registers
these preparation systems every render frame and creates fresh per-view buffers
in `prepare_clusters_for_gpu_clustering`. Merely moving to a newer Bevy release
is therefore not yet an evidence-backed fix; the upstream owner needs a focused
reuse change and before/after measurements.

### 2026-09-25 — UI/physics CPU-tail architecture audit

Source review identifies real main-thread blockers, but does not yet attribute
the measured steady-state frame tail to one of them. The production 60 Hz
`Time<Fixed>` keeps integration delta constant; Bevy drains accumulated fixed
steps synchronously before `Update`. The time owner drains at most 64 complete
cycles per app update and retains all remaining admitted duration in the fixed
accumulator. This preserves time and numerical step size, but does not guarantee
wall-clock 60-tick/s service or isolate the next UI/input frame from a long
causal cycle. Dedicated simulation ownership remains necessary for that boundary.

Other verified tail-risk paths are:

- `drain_world_scripts` is an exclusive `Update` system that takes and evaluates
  the entire queued REPL batch against the live `World`. Rhai allows up to one
  million operations per invocation. The earlier Tracy capture attributed
  about 0.63 ms/call to this system, but did not publish p99/max time or separate
  the empty-queue call from actual evaluations.
- Scenario hooks run serially in the fixed simulation path and may invoke
  substantial live-world Rhai work. Their per-tick maximum and p99 costs are
  not currently available beside the Avian solver timing.
- Terrain visualization keeps worker joins out of `Update`, including during
  offline capture. Capture readiness holds virtual time at the advanced logical
  frame until the selected resident cover is ready; completed meshes publish in
  stable tile order under a per-frame budget. Capture-frame selection remains
  current, and queued terrain work remains part of readiness.
- Shared async admission allows four running jobs and prioritizes only queued
  jobs. It cannot preempt a running CPU task, and visualization owners still
  have direct submissions to Bevy's pools. Pool contention remains a source
  hypothesis, not a measured explanation of the clean 116.9 FPS result.

This rules out schedule labels or a lower catch-up cap as complete solutions.
The required architecture is one paced owner for the full causal simulation
world (Modelica, Rhai, event/coupling barriers, Avian, and authoritative state),
with the window/UI process exchanging typed tick-stamped commands and
nonblocking immutable snapshots. A monotonic real-time driver keeps fixed `dt`
and reports missed deadlines/backlog; unpaced tests and offline recording drive
those same ticks on demand. Renderer interpolation and visualization work
consume snapshots without waiting for that owner. Splitting Avian alone would
leave live-world causal reads/writes racing across threads.

This is a design finding, not an implemented worker split. Before claiming the
tail is fixed, add per-cycle p50/p95/p99/max and tick deadline/backlog evidence,
then separately remove unbounded one-shot evaluation batches, make capture
readiness asynchronous, and budget result application. A same-world
per-application-frame step limit is an interim overload policy only; it must
retain ticks and expose lag, and cannot guarantee UI progress through one
overlong synchronous tick. No FPS acceptance run was started for this audit;
the existing simulator sessions were left untouched.

The idle one-shot path now has a `PendingWorldScripts` run condition, so an
empty queue skips the exclusive drain system's execution. Its focused
`empty_repl_queue_does_not_schedule_its_exclusive_drain` test passed. This does
not bound active Rhai evaluation or queue depth and has no measured FPS delta;
it is a narrow idle-path reduction, not the UI-isolation fix. No new simulator
or FPS profile was started for this change.

### 2026-09-25 — reuse the prepared Rhai engine for one-shot requests

The prior settled Tracy capture recorded seven post-readiness invocations of
`drain_world_scripts`, each taking 79–370 ms on the exclusive application
thread. The requests were not labeled by workload, so these durations cannot
be assigned to a particular tool. Source inspection found that each queued
snippet rebuilt a Rhai engine, rereading native authored sources and compiling
the prelude even though `ScenarioDriver<RhaiScenarioRuntime>` already owned a
prepared engine. One-shot code and tool callbacks now borrow that engine;
`maintain()` refreshes tool modules only when their registry generation
changes, and scoped print capture preserves command stdout without replacing
the shared engine callback. Requests stay queued until authored runtime
preparation completes.

- `scripts/run_rust_tests.sh -p lunco-scripting-rhai-world --lib --filter
  one_shot_ -j 1` passed both engine-reuse/stdout and preparation-queue tests.
- `cargo build -j 4 -p lunco-luncosim --bin luncosim --features tracy` passed.
- In `scripts/perf/captures/sss-rhai-engine-reuse-20260925.tracy`, a controlled
  harmless `RunRhai` probe's exclusive drain span was 0.071 ms. This is not an
  apples-to-apples comparison with the earlier unlabeled callbacks and does
  not establish a clean FPS gain.
- The separate non-Tracy, High-quality Apollo run measured about 66.8 FPS over
  an 82.0 s diagnostic interval and roughly 2.3 ms average Avian step time
  (about 56 steps/s). The 150+ FPS and 0.5 ms physics goals remain unmet.
- The settled portion of the new trace (`t >= 14 s`; schedule spans overlap and
  must not be added) measured Render at 8.02/10.11/13.35 ms p50/p95/p99,
  FixedMain at 5.92/8.05/9.72 ms, and PhysicsSchedule at 2.79/3.67/4.32 ms.
  BigSpace high-precision propagation measured 0.294/0.529/0.662 ms p50/p95/p99;
  gravity computation was 0.308/1.090/1.309 ms and exposure publication
  1.179/1.671/2.093 ms. GPU-clustering preparation had a 54.7 ms maximum
  outlier despite a 0.043 ms median, so that tail needs focused attribution.

The callback rebuild was a severe outlier path, but it does not explain the
steady render cost. The remaining capture points to distributed Render and
main-world fixed/update work, with a clustering outlier worth isolating. No
visual-quality settings were changed. The clean run and profiler session both
used owned API port 4379 sequentially and exited through typed API `Exit`.

### 2026-09-27 — exposure publisher attribution and startup samples

A Tracy-enabled High-quality Apollo run used owned API port 4317 and
`scripts/perf/captures/sss-exposure-branches-20260927.tracy` (35.35 s,
10,618,276 zones). The companion log is
`scripts/perf/captures/sss-exposure-branches-20260927.app.log`. For settled
events (`t >= 10 s`, 402 calls), the main-thread
`exposure_publish_runtime_surfaces` span measured 1.592/2.610/3.173/3.599 ms
p50/p95/p99/max. Its enclosing `exposure_publish_control` span measured
1.603/2.616/3.183/3.619 ms. These spans overlap and must not be summed;
refreshing the authored runtime-surface roots itself was 0.001/0.002 ms
p50/p95. The surface-publication call is now the focused branch for deeper
attribution; this capture does not identify which work inside that call
dominates. The capture used temporary Tracy subspans in
`lunco-luncosim-exposures` and made no behavior change.

The window was created 1.13 s after process launch and `/api/health` first
responded at 1.84 s. `StartupScene opened` was logged 1.53 s after launch.
Physics admission completed at 6.32 s and scene participants became ready at
6.34 s after launch. `/api/ready` briefly reported no pending work at about
2.15 s, then regressed to 13 pending items; it drained at about 6.97 s,
regressed to two at 7.69 s, and returned clear at about 8.05 s. Relative to
`StartupScene opened`, that final clear sample is about 6.51 s. The polling did
not independently verify every producer's registration boundary or the strict
five-second stable-readiness condition, so it does not establish full Twin
readiness or the five-second target. This run also did not capture the exact
scene-root mount time.

The app log's settled rolling diagnostic ended near 75.8 FPS, 14.58 ms frame
time, and 2.38 ms Avian total-step time. Tracy and concurrent simulator work
affected these numbers; they are not FPS or physics acceptance. The app exited
through typed API `Exit`, and owned ports 4317 and 8086 were released. Ports
4101 and 4102 belonged to other active sessions and were left untouched.

### 2026-09-27 — nested runtime UI costs and readiness detail

A second windowed High-quality Apollo profile used `--no-vsync --no-throttle
--log-diag` and the same authored `traverse_apollo15.usda` scene. Its capture,
`scripts/perf/captures/sss-exposure-subbranches-20260927.tracy`, ran for 25.3 s
and contained 7,019,040 zones. In the settled window (`t >= 10 s`, 236 calls),
`exposure_publish_runtime_surfaces` measured 1.651/2.953/3.621/4.763 ms
p50/p95/p99/max. The nested subspans measured:

- Runtime UI properties policy: 0.951/1.728/2.231/2.799 ms.
- Runtime UI visibility policy: 0.441/0.879/1.040/1.476 ms.
- Runtime UI fact construction: 0.181/0.385/0.501/0.635 ms.
- Authored telemetry selection: 0.017/0.036/0.057/0.110 ms.
- Exposure writes: 0.017/0.035/0.057/0.076 ms.

The two Rhai policy calls dominate this settled publisher path. The publisher
is still invalidated by continuously moving vessel facts at its bounded
presentation cadence; these measurements do not justify lowering that cadence
or removing facts. They are a UI-path lead, not an explanation of Twin startup.
This run's API poll saw its final clear readiness state at about 6.76 s from
process launch, after regressions, and kept it clear through typed shutdown at
28.57 s. `StartupScene opened` was about 1.44 s after process launch, so this
profiler-affected windowed run reached the candidate full-ready transition
about 5.32 s after Twin-open. The exact scene-root mount was logged about
0.47 s after Twin-open. This narrowly misses the five-second Twin target and is
not clean acceptance; concurrent simulator sessions remained active.

A shorter headless Tracy follow-up recorded pending readiness items directly
in `/api/ready`. Its capture,
`scripts/perf/captures/sss-readiness-details-20260927.tracy`, lasted 13.76 s
(811,556 zones); the production Twin was Apollo, owned API port 4317, and
`--no-ui` was active. `StartupScene opened` was 0.383 s after process launch;
the scene root spawned 0.634 s after Twin-open. Physics admission and scene
participants became ready 3.76 s after Twin-open. After a transient all-clear
sample at 4.585 s from process launch, `/api/ready` registered `USD connection
binding` and `deferred prims`; both cleared by 5.268 s from process launch.
The final state (`readiness_tracked=true`, `ready=true`, `world_hold=false`,
`faulted=false`, `pending_count=0`) then remained clear through shutdown at
13.914 s. That is about 4.89 s after Twin-open and clears the five-second
threshold by only about 0.11 s. This is profiler- and CPU-contention-affected
headless evidence, not a clean or windowed acceptance run. It does not measure
app/window readiness, FPS, or physics acceptance.

The Tracy-enabled production build passed before these captures. Both owned
sessions exited through typed API `Exit`; ports 4317 and 8086 were released.
Ports 4101 and 4102 remained active for other sessions and were not touched.

### 2026-09-27 — overlay-sync subspan follow-up

After fast-forwarding `optimization` to `origin/main` at `277fced5d` and
reapplying the worktree, the current dirty production tree built with Tracy and
ran the authored Apollo scene on API port 4317 / Tracy port 8087. The capture
`scripts/perf/captures/sss-overlay-subspans-20260927.tracy` lasted 25.39 s
(1,525 frames, 6,446,368 zones, 57.22 MB); the app log is
`scripts/perf/captures/sss-overlay-subspans-20260927.app.log`. Apollo sessions
on ports 4101 and 4102 and a Griffin test on port 47123 were active at the
pre-run check. A separate Cargo build in another checkout began at 07:57:44
local, about 26 s before this app, and remained active in later checks. This is
contention-affected diagnostic evidence; those sessions and the build were left
untouched.

The child zones did not reproduce the earlier 260.85/277.76 ms
`sync_twin_overlays` calls. Across 12 calls, the longest outer system span was
4.545 ms. `usd_twin_projection_document_sync` had 32 calls and a 4.429 ms
maximum; the single `usd_twin_projection_rebuild_history_gap` took 2.905 ms.
The current rebuild path prepares a replacement `CanonicalStage` once before
retiring the stage-owned ECS state, instead of building and discarding a
separate validation projection plan. This capture records the changed path's
short duration but is not an isolated before/after.

The six `usd_twin_projection_refresh_dependent_stage_assets` spans totaled
0.228 ms, with a 0.054 ms maximum. Each corresponding app-log message reported
`dependent_candidates=0`, so the earlier expensive dependent-stage refresh was
not exercised in this run.

Tracy's per-event export also recorded one `process_usd_avian_prims` observer
call at 13.566 ms, 4.342 s after process start. Five more calls exceeded 1 ms
between 4.534 and 4.560 s; across 1,980 calls the mean was 24.1 µs. The
outlier is above the 6.67 ms frame budget, but the trace does not include the
prim path and this cluster occurred during scene projection. Attribute that
prim and its collider/body projection work in a fresh diagnostic before
changing the observer. This startup-only outlier does not establish a settled
150 FPS regression. The observer source now adds a nested
`usd_avian_project_prim` Tracy span with the stage ID and prim path, covering
composed-reader selection and physics extraction. Rebuild the Tracy production
binary before the next capture; this span is not in the saved trace.

The same capture links the late participant-ready event to cold Modelica
solver preparation. `lower_for_live` took 9.037/9.168 s for the two rocker
bogie systems, 5.658 s for Ackermann, 0.529 s for Rover, and 8.209 s for the
Electrical system. Their worker queue waits were 26–94 µs, while the associated
Modelica compiler calls logged 0.04–0.14 s. Electrical lowering ended at about
23.35 s from launch; scene participants became ready at 23.74 s. Within the
worker path, these spans point to lowering time rather than queue admission as
the dominant cold-start cost. Persistent-cache hits remain a separate warm
path; the cold full-readiness target is still open.

The capture identifies the total `lower_for_live` wall time but does not show
which Rumoca phase dominates. Rumoca emits per-phase `elapsed_seconds` debug
events on the `rumoca_sim::solve_lowering` target; the default app filter hid
them in this run. For the next CPU diagnostic, launch with
`RUST_LOG=rumoca_sim::solve_lowering=debug` and include those phase timings in
the capture notes before choosing a lowering optimization.

From the process start recorded in `/proc` (05:58:10.680Z), the window was
created at 2.435 s, `StartupScene opened` at 2.725 s, and the scene root spawned
at 3.865 s (1.140 s after Twin-open). Physics admission and scene-participant
lifecycle readiness completed at 23.732 s and 23.739 s after process start,
about 21.01 s after Twin-open. An early, untimestamped `/api/ready` read had 12
pending items; the timestamped poll later saw a clear state at 31.06 s. The
exact full-readiness transition and five-second soak were not captured, so this
does not establish the five-second target. The health endpoint responded, but
its sample time was not recorded alongside window creation, so app readiness
also remains unverified. The window itself was created at 2.435 s in this
profiled, contended run, missing the app target. Typed API `Exit` was accepted
and owned ports 4317 and 8087 were released.

### 2026-09-27 — path-bearing Avian projection capture

After the observer gained its `usd_avian_project_prim` span, the Tracy
production build passed with `cargo build -j 4 -p lunco-luncosim --bin
luncosim --features tracy`. The owned High-quality, windowed Apollo run used
X11, `--no-vsync --no-throttle --log-diag`, API port 49281, Tracy port 8089,
and `RUST_LOG=rumoca_sim::solve_lowering=debug`. Its 30.31 s capture is
`scripts/perf/captures/sss-avian-path-diagnostic-20260927.tracy` (1,193 frames,
6,770,061 zones, 56.64 MB); the app log is
`scripts/perf/captures/sss-avian-path-diagnostic-20260927.app.log`. The
matching per-event exports are kept beside the ignored capture. The production
process used this checkout's binary and working directory, accepted typed API
`Exit`, and released ports 49281 and 8089.

The path-bearing observer span appeared 818 times: mean 0.081 ms, p50 0.021
ms, p95 0.143 ms, p99 1.144 ms, and max 20.445 ms; ten events exceeded 1 ms.
The maximum was `/Traverse`, also the first observer event, at about 3.667 s
into the capture. The next events were `/Traverse/Rover` at 2.792 ms, a
rocker-bogie root at 1.710 ms, and the Ackermann root at 1.560 ms. `/Traverse`
is the authored scene-root `Xform`, not a rigid body. Its first observer call
also misses the stage's `CollisionGroupTable`
cache; that read enumerates composed prim paths and sorts collision-group
paths. The app log has no collision-group summary; the owner logs only
nonempty tables, consistent with this scan returning an empty table. At the
time, the trace only measured the whole observer; the child-span follow-up below
measures the table lookup directly. This outlier is startup-only and does not
establish a settled FPS regression.

The same capture recorded 14 `sync_twin_overlays` calls, with a 5.363 ms max
and 2.365 ms p95. It did not reproduce the earlier 260.85/277.76 ms calls.
The two component-layer refresh messages had `dependent_candidates=0`. The
main session on 3743 and Griffin requirements run on 47123 were active, so
this remains a contention- and profiler-affected diagnostic.

The window was created at 06:39:58.603Z; `StartupScene opened` was at
06:39:58.931Z, and the scene root spawned at 06:39:59.729Z (0.798 s after
Twin-open). Physics admission completed at 06:40:04.674Z and scene participants
became lifecycle-ready at 06:40:04.685Z, about 5.74/5.75 s after Twin-open.
That necessary lifecycle milestone is already beyond the 5 s target. The first
`/api/ready` sample was around 13 s after process start and a later sample near
29 s was still clear, but the transition was not polled continuously. The
`/api/health` check also came after its 2 s deadline. This run establishes
neither app readiness nor Twin readiness acceptance. No
`rumoca_sim::solve_lowering` phase events appeared, so it did not exercise the
cold Modelica lowering path.

The observer source now has child spans for collision-group lookup, physics
extraction, and compound-collider collection, and the parent span omits the
comma-containing stage debug field so Tracy's CSV export keeps its columns
aligned. The post-fast-forward follow-up below includes those spans.

### 2026-09-27 — post-fast-forward Avian and startup capture

After `optimization` was fast-forwarded to local `main` at `791cda5e3`, the
Tracy production build passed in 3m48s. The captured runtime tree includes that
fast-forward and the restored local work. Local `main` later advanced to
`1dce476e6` with documentation and scenario-fixture changes only; the profiled
runtime owners were unchanged. The owned High-quality, windowed Apollo run used
X11, `--no-vsync --no-throttle --log-diag`, API port 49282, Tracy port 8089 via
`TRACY_PORT=8089`, and
`RUST_LOG=rumoca_sim::solve_lowering=debug`. Its 35.32 s capture is
`scripts/perf/captures/sss-avian-child-spans-mainhead-791cda5-20260927.tracy`
(822 frames, 4,926,278 zones, 45.43 MB); the app log is
`scripts/perf/captures/sss-avian-child-spans-mainhead-791cda5-20260927.app.log`.
Per-event exports for the Avian spans, physics systems, Twin projection, and
overlay sync are beside the ignored capture. A Griffin requirements app on
port 47123 remained active, and host load was high, so this is
profiler- and contention-affected diagnostic evidence.

The 818 `usd_avian_project_prim` calls measured 0.026/0.126/1.029 ms p50/p95/
p99, with a 23.575 ms maximum at `/Traverse`. Its nested
`usd_avian_collision_groups` span took 23.529 ms on the same first root call;
physics extraction took 0.023 ms there. The collision-group lookup enumerates
all composed prim paths and filters/sorts collision-group paths before caching
the stage table. The next observer events peaked at 2.605 ms for
`/Traverse/Rover` extraction and 2.499 ms for compound-collider collection. The
first-use table scan is now localized as a startup frame hitch; the capture
does not show that it affects settled FPS or full Twin readiness.

The same capture's `sync_twin_overlays` span ran 13 times, with 0.250 ms p50
and an 8.334 ms maximum. The maximum contains an 8.190 ms
`usd_twin_projection_document_sync` span; 8.105 ms was in document-delta commit,
including 8.099 ms replaying incremental operations. The trace does not label
the eight individual operation kinds. This is the longest overlay event in
this capture; the earlier 260.85/277.76 ms calls did not recur here. All timings
remain affected by profiling and contention.

For physics, one `PhysicsPerformance` query at 30.083 s after process launch
returned 120 recent Avian step samples at step 627: p50 3.781 ms, p95 5.627
ms, p99 6.194 ms, and max 6.952 ms; all 120 samples exceeded 0.5 ms. The scene
had 284 bodies, 23 dynamic bodies, and 272 colliders. Tracy's
`avian3d::schedule::run_physics_schedule` measured 5.226 ms mean, 8.202 ms p95,
and 23.983 ms max. The narrow-phase update with the USD collision filter was
0.657 ms mean, 0.943 ms p95, and 1.484 ms max. These are useful hot-path leads,
not clean physics acceptance; parent and nested schedule spans overlap.

The Tracy self-time export for the same capture measured Avian narrow-phase
updates at 0.657 ms per physics step while the late-trace contact counter was
18–20. Velocity integration accounted for about 0.250 ms per fixed step and
position integration about 0.229 ms per step; each ran eight times per step.
Solver-body angular-inertia updates contributed about 0.208 ms per step.
These self-time aggregates remain profiler- and contention-affected and can
overlap through parallel work, so do not sum them as a wall-time budget. They
show the 0.5 ms target depends on both contact generation and repeated body
integration. A later USD contact-modification span had no calls in its Apollo
capture, so the hook's share remains unmeasured.

The process-relative startup monitor recorded the window at about 1.27 s and
the first `/api/health` response at 1.43 s, both inside the 2 s app target for
this diagnostic. The Twin opened at about 1.52 s. Its scene root spawned 0.572
s after Twin-open. Physics admission and scene participants became ready 5.664
and 5.679 s after Twin-open. `/api/ready` first returned clear before the Twin
opened, then registered scene, physics, program-compile, and participant
waits. A clear state after participant readiness first appeared at 5.895 s
after Twin-open but regressed to `USD connection binding` and `deferred prims`.
The first clear state that remained stable for five seconds began at 7.532 s
after Twin-open and was confirmed clear at 12.578 s. This misses the 5 s Twin
target; the early green response is not full readiness.

Typed API `Exit` was accepted, the owned process exited with code 0, and ports
49282 and 8089 were released. This capture does not measure sandbox or clean
150 FPS/physics acceptance.

Before another profile, the 2026-09-27 host recheck found ports 4101 and 4317
free, while the Griffin app remained active on 47123. A Rust build was running
in the sibling `usd` checkout and system load averages were 15.19/13.62/11.94.
No new performance session was launched under that contention; recheck again
before profiling.

The follow-up Tracy production build added a span around the USD
`UsdCollisionFilter::modify_contacts` hook and passed in 2m05s. The capture
below used the resulting `1dce476e-dirty` binary, before the later local-main
fast-forward to `c55f3de8a`.

### 2026-09-27 — contact-hook and readiness follow-up

The owned High-quality, windowed Apollo run used X11,
`--no-vsync --no-throttle --log-diag`, API port 49282, Tracy port 8089, and
`RUST_LOG=rumoca_sim::solve_lowering=debug`. Its 30.3 s capture is
`scripts/perf/captures/sss-avian-modify-contacts-20260927.tracy` (1,434 frames,
7,894,952 zones, 62.5 MiB); the app log is
`scripts/perf/captures/sss-avian-modify-contacts-20260927.app.log`. The Griffin
requirements app on port 47123 remained active, so these profiler timings are
contention-affected diagnostics.

The trace and app log were available for analysis after the run, but the
`scripts/perf/captures/` directory is absent from the current optimization
checkout. The measurements below remain recorded here; repeat the capture to
reopen raw Tracy events.

The new `usd_avian_modify_contacts` span had zero calls. Avian invokes this
hook only after it generates a touching pair whose `MODIFY_CONTACTS` flag is
enabled; this run therefore provides no per-call cost for the USD hook. It
does not justify an optimization there. Avian's `update_narrow_phase` ran 882
times at 0.442273 ms mean (0.298987 ms minimum, 4.103598 ms maximum). This
moves the measured physics lead to Avian contact generation; verify its cost
in a clean run before changing that owner.

Tracy's `run_physics_schedule` averaged 3.197 ms across 895 calls, with a
20.696 ms maximum. The 882 `PhysicsSchedule` spans averaged 3.230 ms with a
20.641 ms maximum; nested `run_substep_schedule` averaged 1.579 ms with a
14.444 ms maximum. These parent and nested spans overlap and must not be
summed.

`sync_twin_overlays` ran 11 times, averaging 1.241 ms with a 5.971 ms maximum.
This capture did not reproduce the earlier 260.85/277.76 ms app-thread stalls.
Keep those as the strongest observed UI-stall events, with the later history-gap
recovery and dependent-stage refresh attribution, but this run provides no new
evidence that either stall recurred.

A separate post-capture `PhysicsPerformance` query returned 120 recent samples
at step 6543: p50 6.620317 ms, p95 9.093827 ms, p99 10.678469 ms, and max
11.684374 ms. The world had 284 bodies, 23 dynamic bodies, 1 sleeping body,
272 colliders, and 18 joints. The post-capture Avian total-step rolling average
was about 5.76 ms; frame-time average later reached 22.12 ms (about 45 FPS).
These were still Tracy-featured and contention-affected readings, not clean
acceptance measurements.

The process log starts at 07:54:00.529Z. The window was created about 1.02 s
later and `StartupScene opened` was logged about 1.22 s after process start.
The first `/api/health` sample was taken only after about 17 s, so app
readiness against its 2 s clock was not measured. The Twin scene mounted about
0.45 s after Twin-open. Physics admission completed at about 5.05 s and scene
participants became ready at about 5.07 s after Twin-open, just beyond the
5 s target. Readiness was still pending near 20 s and a later sample was clear
by capture end around 45 s; the exact transition and a five-second stable
interval were not captured. This does not establish full Twin readiness within
5 s. The run did not measure sandbox or clean 150 FPS/physics acceptance.

Typed API `Exit` was accepted, the owned process exited, and ports 49282 and
8089 were released. At the 10:11 Europe/Belgrade host recheck, ports 4101,
4317, 49282, and 8089 were free; Griffin still owned 47123 at about 30% CPU,
and Cargo/rustc work remained active in the sibling `lunar-soil` checkout.
System load averages were 16.14/13.49/11.51. No new profile was started
against those workloads.

### 2026-09-27 — 4d8b139f0 readiness and Tracy follow-up

The Tracy production binary was built at `13137c115` with this optimization
worktree's uncommitted performance changes. `main` later advanced through
`54acee747` (SysML analysis) and `4d8b139f0` (route-test coverage), and
optimization was fast-forwarded to `4d8b139f0` after the capture. This trace
predates those commits. The owned High-quality, windowed Apollo run used
`--no-vsync --no-throttle --log-diag`, API port 49381, and Tracy port 8093.
Its 60.3 s capture is
`target/sss-apollo-tracy-current-main-20260927.tracy` (3,420 frames,
19,035,018 zones, 146.22 MiB); the process log and focused CSV exports are in
`target/` beside it. The capture's 56.7 frames/s is profiler- and
contention-affected, not clean FPS acceptance. At the 10:42 Europe/Belgrade
pre-run check, Griffin remained active on port 47123 at about 28% CPU and a
headless Griffin session was active on port 47124 at about 26% CPU. The latter
was gone by the post-capture check. Load averages were 7.68/11.26/13.22.

The process started at `08:43:55.251424Z`. Bevy logged its window-creation
system at `08:43:56.495467Z` (1.244 s), and the first `/api/health` response
arrived at 1.288 s. No compositor-level observation of a mapped window was
captured, so app readiness remains unverified. `StartupScene opened` was logged
at `08:43:56.741741Z`, 1.490 s after launch. The scene root spawned at
`08:43:57.197893Z`, 0.456 s after Twin-open. Physics admission and
scene-participant lifecycle readiness completed 4.572 s and 4.584 s after
Twin-open.

`/api/ready` was briefly all-clear at 1.288 s from process launch, before the
Twin opened; the scene-load wait appeared at 2.106 s. After scene and
participant startup, the first all-clear sample was at 6.242 s from process
launch (4.752 s after Twin-open), then readiness regressed at 6.582 s with USD
connection binding and deferred prims pending. The next all-clear sample was
at 7.063 s from process launch (5.573 s after Twin-open) and remained clear
through the five-second stability window and capture shutdown. The qualifying
stable transition therefore missed the five-second Twin target by about
0.573 s. The scene mount was under two seconds in this diagnostic, but these
profiled, contended timings are not clean acceptance.

The trace recorded 13 `sync_twin_overlays` calls; the longest was 6.889 ms.
The earlier 260.85/277.76 ms app-thread stalls did not recur. In Tracy's
self-time export, the Render and PostUpdate schedule spans averaged 3.382 ms
and 3.318 ms, with maxima of 150.001 ms and 67.352 ms. `globe_tile_mesh` ran 648 times for 4.859 s of
aggregate worker time, with a 165.784 ms maximum. It ran on worker threads
11–14; 89 calls occurred after the first 10 s, with p50/p95/p99/max durations
of 18.158/42.513/45.772/49.132 ms. This is a sustained terrain-work lead, not
evidence that worker completion blocks the UI. BigSpace's
`propagate_high_precision_channeled` span ran 3,387 times at 0.314 ms mean and
6.987 ms maximum. The scene still had 23 dynamic bodies and one sleeping body,
so this profile alone cannot distinguish required moving-state propagation
from avoidable work.

One `PhysicsPerformance` query returned 120 recent fixed-step samples at step
2488: p50 2.781 ms, p95 3.751 ms, p99 4.168 ms, and max 4.643 ms; every sample
exceeded 0.5 ms. The world had 283 bodies, 23 dynamic bodies, 271 colliders,
and 18 joints. Tracy's self-time `PhysicsSchedule` spans averaged 0.405 ms
with an 8.657 ms maximum; Avian `update_narrow_phase` averaged 0.341 ms with a
0.794 ms maximum in the same self-time export. These are attribution timings,
not the full-step samples.

The app log also recorded a physics-body escape that paused seven dynamic
bodies, followed by Bevy render errors reading unallocated slab-allocator
keys (39 repeated instances were deduplicated). These scene/runtime findings
need separate review. Typed API `Exit` was accepted, the process exited with
code 0, and API port 49381 and Tracy port 8093 were released. The run did not
measure sandbox or clean FPS/physics acceptance.

### 2026-09-27 — 4d8b139f0 unprofiled Apollo and sandbox diagnostics

After the latest local `main` fast-forward, the no-Tracy production binary was
built at `4d8b139f0` with this worktree's uncommitted performance changes via
`cargo build -p lunco-luncosim --bin luncosim -j 4`. Both High-quality,
windowed runs used `--no-vsync --no-throttle --log-diag`, X11, and distinct
owned API ports: Apollo 49382, then sandbox 49383. Griffin remained active on
external port 47123 during these runs, so treat all timing as contended
diagnostics rather than clean acceptance. Run summaries and logs are in
`target/apollo-high-unprofiled-contended-20260927.*` and
`target/sandbox-high-unprofiled-contended-20260927.*`.

For Apollo, the window-creation log was at 0.634 s and `/api/health` responded
at 0.767 s after process launch. `StartupScene opened` was at 0.779 s; the
scene root spawned at 1.111 s, mounting 0.332 s after Twin-open. The strict
full-readiness candidate began 3.966 s after Twin-open and stayed clear through
the five-second soak. This diagnostic is inside the separate 2 s app and
5 s Twin targets; compositor-level window mapping was not observed. Across 240
settled telemetry samples, FPS was 147.64 p50, 149.42 mean, and 211.46 p95;
frame time was 6.205 ms p50 and 10.777 ms p95. The p50 FPS remains below 150.
The 120-sample full Avian step was 0.825/1.264/1.492/1.790 ms p50/p95/p99/max;
all samples exceeded 0.5 ms. The scene had 283 bodies (23 dynamic), 271
colliders, and 18 joints.

For sandbox, window creation and `/api/health` were logged at 0.620 s and
0.766 s after launch. `StartupScene opened` was at 0.948 s, and the scene root
spawned at 1.236 s, mounting 0.288 s after Twin-open. Physics admission and
scene-participant readiness completed at 11.753 s and 11.757 s after Twin-open.
The first strict all-clear candidate began 10.906 s after Twin-open and stayed
clear through the five-second soak, missing the full-readiness target by
5.906 s. Compositor-level window mapping was not observed. Across 240 settled
samples, FPS was 69.78 p50, 69.20 mean, and 75.92 p95; frame time was 14.331 ms
p50 and 16.046 ms p95. The 120-sample full Avian step was
1.014/1.278/1.418/1.510 ms p50/p95/p99/max; all samples exceeded 0.5 ms. The
scene had 50 bodies (44 dynamic), 33 colliders, and 32 joints.

Both sessions accepted typed API `Exit`, exited with code 0, and released
their ports. These runs separate app/window-health, scene-mount, and full-Twin
readiness clocks; their startup and performance results remain contention-
affected diagnostics, not acceptance.

### 2026-09-27 — 4d8b139f0 sandbox Tracy shadow-view attribution

The follow-up Tracy-enabled production run used the High-quality sandbox on
owned API port 49384 and Tracy port 8094. Its 60.4 s capture is
`target/sandbox-tracy-current-main-20260927.tracy` (1,885 frames,
11,112,813 zones); the app log and CPU/GPU exports are beside it in `target/`.
The Griffin app on port 47123 remained active, and host GPU utilization was
already elevated. Treat this run as profiler- and contention-affected
diagnostics. The Tracy binary was built from `4d8b139f0` plus this worktree's
uncommitted changes.

The sandbox composes six rover assemblies. Each references the shared headlight
component twice; each headlight authors a 90 m, 20-degree-cone `SphereLight`
with shadows enabled. Tracy observed 12 spotlight shadow roots on nearly every
frame. The render-world relevance filter retained them because their
conservative influence bounds intersected the output view. High quality sets
`max_spot_shadow_casters` to 8, while the authored scene supplies 12; the
existing Rhai policy reports an unmet limit and deliberately leaves every
authored shadow enabled.

CPU attribution makes those repeated light views the strongest sandbox render
lead: `camera_driver` averaged 12.27 ms per frame, the Render schedule averaged
23.13 ms, and Core3d ran 12.56 camera roots per output frame. The twelve
auxiliary spotlight camera schedules each averaged about 0.71–0.93 ms. Bevy's
`check_point_light_mesh_visibility` averaged 0.946 ms per frame; in Bevy 0.19.1
it scans eligible shadow-caster meshes once for each visible local light. The
current [Bevy upstream implementation](https://github.com/bevyengine/bevy/blob/main/crates/bevy_light/src/lib.rs)
still has that per-light scan. GPU export after 30 s showed the main opaque
pass at 2.17/2.41/3.21/5.51 ms p50/p95/p99/max and no named spotlight shadow
pass events, so the trace does not quantify GPU shadow-map time. The CPU spans
overlap and include profiler overhead; they are not a serial frame budget.

`StartupScene opened` was logged 3.39 s after process start and the scene root
spawned 1.76 s after Twin-open. The first `/api/health` response was about
3.05 s after process launch. Readiness first appeared clear at that early
sample, later regressed, and remained pending around 20 s; the later clear
sample was near capture end, so the exact stable transition was not captured.
Four `modelica_solve_preparation_lower_for_live` worker jobs took 14.3–16.3 s
each, with only 39–157 microseconds of queue wait. This is a startup-readiness
lead to investigate separately from the steady render cost.

The owned app accepted typed API `Exit` and exited with code 0; ports 49384 and
8094 were released. This capture is not clean FPS, physics, app-readiness, or
full-Twin-readiness acceptance.

### 2026-09-27 — 8ad070fa0 unprofiled sandbox diagnostic

After optimization fast-forwarded to local `main` at `8ad070fa0`, the no-Tracy
production binary was rebuilt with
`cargo build -p lunco-luncosim --bin luncosim -j 4` (passed in about 4 minutes).
The High-quality X11 sandbox run used `--no-vsync --no-throttle --log-diag`,
the authored `assets/scenes/luncosim/sandbox_scene.usda`, and owned API port
49385. Its process log and API/telemetry snapshots are in
`target/sandbox-high-current-8ad070fa-contended-20260927.*`.

The process started at `09:40:35.518Z`. Window creation was logged at 3.99 s,
`StartupScene opened` at 4.57 s, and the scene root spawned at 5.51 s, or
0.94 s after Twin-open. The first successful `/api/health` sample arrived at
32.97 s after process launch, so the 2 s app-readiness target was not
demonstrated. Physics admission and scene-participant readiness completed at
7.66 s and 7.70 s after Twin-open, already beyond the 5 s full-readiness
target. The readiness poll initially misread the API's `data` wrapper; a
direct GET later showed `ready: true` at about 87.5 s after process start. A
corrected poll then observed continuous all-clear samples from 127.875 s to
133.121 s after process start, a five-second soak. The transition and any
earlier stable interval were not captured, so this run provides no evidence
that full readiness arrived within 5 s of Twin-open.

Across 240 telemetry samples, FPS was 41.756 p50, 68.733 p95, 77.691 p99,
and 84.705 maximum; frame time was 23.997 ms p50 and 58.113 ms p95. The 120
recent full Avian step samples were 3.459/8.054/9.907/14.874 ms p50/p95/p99/max;
all 120 exceeded 0.5 ms. The world had 50 bodies (44 dynamic), 33 colliders,
and 32 joints. Runtime diagnostics also recorded 1,406 realtime-budget
exceedances. Around the run, system load averages reached 23.16/19.25/13.73,
GPU utilization reached 91%, and the app used about 248% CPU. Griffin was
present before the build but absent at the post-run check; overlap during the
capture was not established. Treat all figures as heavily contended
diagnostics, not acceptance results or a comparable regression measurement.

The owned app accepted typed API `Exit` and exited; API port 49385 was
released. Port 47123 was also closed at the post-run check. The run did not
use Tracy and did not provide clean FPS or physics acceptance.

### 2026-09-27 — sandbox Tracy CPU follow-up and targeted reductions

Before the current follow-up edits, the High-quality sandbox ran with the
Tracy-enabled production binary on API 49386 / Tracy 8086. The 65-second
capture is `target/sandbox-tracy-8ad070fa0-20260927.tracy`, with the app log
and CSV exports beside it. It contained 315 Tracy frames and about 2.52 million
zones. The process started at `09:57:07.207Z`; the window was created at
`09:57:15.155Z` (7.95 s after process launch), Twin-open was logged at
`09:57:15.922Z`, and scene participants became ready at
`09:57:27.322Z` (11.40 s after Twin-open). Full readiness remained pending
near 20 s and was clear in a later sample near 45 s; the exact transition and
a five-second stable interval were not captured.

The trace's `process_queued_usd_visuals` system ran 15 times, taking
104.78 ms total, 6.99 ms mean, and 21.35 ms maximum. The pass rebuilt a
whole-world child-identity set for each queued batch, making small incremental
batches pay for every live prim. Its duplicate check now walks only the direct
children of queued parents. `register_ready_schema_assets` ran 401 times, taking
206.45 ms total with a 195.79 ms maximum; it cloned its full entry list on each
pass and kept checking after all sources settled. It now iterates the owned
entries without cloning, caches the core-source IDs at request time, and closes
the system after completion. `celestial_visuals_system` also ran 351 times
(0.170 ms mean, 6.64 ms maximum); it rebuilt its per-body transition map each
frame and cloned it on dirty updates. It now reuses current/previous map
capacity and swaps the maps after a write pass. All three owners passed the
focused `cargo check`; none of these edits has yet been reprofiled.

The capture also hit GPU out-of-memory and shadow-map shader errors. Presentation
recovery disabled cameras after thousands of uncaptured render errors, and the
app exited at capture end. The profiler run reached load averages of
26.63/17.30/14.13 with the app around 275% CPU. The current follow-up windowed
profile was deferred while sibling applications occupied 98% GPU utilization
and 5.4/8.1 GiB. After the focused check, three sibling apps were still active
on ports 4147, 3864, and 3865; system load was 40.82/32.35/22.43. No sibling
session was controlled or stopped, and no fresh runtime profile was launched.
The Tracy production build passed before the celestial map-reuse edit; the final
source passed the focused check. This trace is a startup and UI-work diagnostic,
not clean FPS, physics, renderer, or readiness acceptance.

### 2026-09-27 — bounded domain synthesis off the frame cycle

Domain network synthesis now submits through `AsyncWorkAdmission` with a
stable work key and a bounded, non-blocking admission path. Immutable prepared
USD plans run network synthesis fully on workers. Live canonical OpenUSD
readers remain thread-bound, so that path captures the typed network facts on
the owning thread and moves Rhai policy execution, matrix construction, source
parsing, and validation to a worker. Both hook-backed and actuator-wrench
policies use this split. Completed results are fenced by Twin/stage/instance
generation, published in request order, and limited to one network commit per
Update; live-reader facts are snapshotted for at most one network per Update.
Authored telemetry ownership indexes persist by canonical generation
or immutable plan identity rather than rebuilding a full-stage scan for each
completion frame; prepared-plan indexes are populated once on a worker. Worker
requests copy only resolved class facts referenced by their network, not the
asset handles and pending-resolution maps from the full class resource.

This is an unprofiled implementation change. The focused
`cargo check -p lunco-usd-sim-domain -p lunco-usd-sim-cosim -j 4` passed on
2026-09-27; no behavior tests, fresh Tracy capture, or clean performance
acceptance run was performed. Live USD fact extraction and the first
telemetry-index build for a changed canonical stage remain main-thread work;
profile those spans before claiming they fit the frame budget. The current
150 FPS, sub-0.5 ms full physics-step, 2 s app/scene-mount, and 5 s full Twin
readiness targets remain unverified.

### 2026-09-27 — entity-tree derivation off the UI cycle

The 84.9 s Apollo trace
`target/sss-apollo-boundary-index-followup-20260927.tracy` measured
`populate_entity_tree_view` at 390.766 ms maximum. The producer now snapshots
the required ECS facts on the app thread and derives hierarchy, visibility,
labels, and ordering on one bounded `AsyncComputeTaskPool` task. One in-flight
task coalesces topology invalidations; a revision fence drops stale results,
the last completed tree remains readable while work is pending, and Twin close
clears the view and invalidates the result.

The follow-up High-quality Apollo trace is
`target/sss-apollo-entity-tree-async-authored-20260927.tracy` (45.34 s,
1,708 frames, 9,183,023 zones, 82.28 MB). In it, the producer ran 25 times at
2.846 ms mean and 13.766 ms maximum; `entity_tree_view_snapshot` peaked at
13.717 ms. The worker derivation ran 25 times at 1.774 ms mean and 4.365 ms
maximum. This is diagnostic evidence from a different capture and workload,
not a clean before/after acceptance result. The Tracy-enabled production build
and focused `cargo check -p lunco-luncosim-edit-ui -j 4` passed; no tests were
run.

This trace also reproduced a long exclusive app-thread operation:
`sync_twin_overlays` ran 10 times and reached 343.400 ms. Its
`usd_twin_projection_refresh_dependent_stage_assets` child reached 340.479 ms;
the long event refreshed one dependent stage after the `lunokhod2.usda` layer
changed. That capture had no finer spans, so the time inside stage-plan
construction, canonical-stage rebuild, and visual requeue was not attributed.
Other revision-triggered maxima were 99.474 ms in
`mark_usd_telemetry_projection_index_dirty`, 81.867 ms in
`project_usd_policies`, 58.670 ms in `process_queued_usd_visuals`, 48.338 ms
in `process_usd_sim_prims`, and 45.674 ms in
`project_authored_runtime_components`. These remain startup/edit-cycle leads;
their preparation and owning commit boundaries need separate profiling before
moving them.

The app window-creation system ran 0.754 s after process start, and the
authored Twin scene opened 0.936 s after start. Physics admission completed
17.213 s after Twin-open and scene participants became lifecycle-ready at
17.231 s. An early `/api/ready` sample had 12 pending items. The first sampled
all-clear state was at 13:09:48Z and remained clear at 13:09:53Z; the transition
time was not captured, and this does not establish the 5 s full-readiness
target. Typed API `Exit` was accepted; the owned process exited with code 0 and
released ports 4101 and 8089. Tracy overhead and concurrent system load remain,
so none of these timings are clean FPS, physics, or readiness acceptance.

### 2026-09-27 — dependent-stage refresh and startup follow-up

The dependent-stage owner now shares immutable `StageRecipe` values through
`Arc`, coalesces changed-layer overlays per target stage, and submits the recipe
copy and immutable projection-plan composition through bounded USD admission.
The thread-affine `CanonicalStage` still opens on the app thread after the owner
checks the operation/revision and target-plan identity; an active stage retains
its exact simulation-progress key through that commit. Tracy spans separate
candidate scanning, worker plan preparation, live-stage build, reset
preparation, and visual refresh. The focused application `cargo check` and
Tracy production build passed; no tests were run.

The earlier 30.35 s follow-up,
`target/sss-apollo-dependent-refresh-single-stage-20260927.tracy` (601 frames,
3,578,410 zones, 36.75 MB), did not execute the changed-layer rebuild branch:
the dependent Traverse recipe already contained the current layer. Across the
seven refresh calls, the candidate scan peaked at 10.569 microseconds and the
whole refresh at 0.907 ms; `sync_twin_overlays` peaked at 16.816 ms. This does
not measure the earlier 340.479 ms changed-layer refresh.

The 35.31 s Tracy follow-up,
`target/sss-dependent-stage-async-20260927.tracy` (751 frames, 4,531,510
zones, 42.83 MB), also did not execute the changed-byte rebuild: the Apollo
scene document reported one dependent candidate, whose recipe already held the
current layer. The 15 `sync_twin_overlays` calls peaked at 24.149 ms (2.408 ms
mean), and the seven candidate scans peaked at 13.394 microseconds. No worker
plan-preparation or live-stage-build span was recorded, so this run does not
measure the moved work or establish a before/after reduction. A controlled
changed-layer capture is still needed. The user-owned session on 4101/8089
remained active during the profiler run; timings are contention-affected.

In this run, physics admission first reported `/Traverse/Rover` as kinematic,
state-pending, and initialization-pending at 1.925 s after the logged
Twin-open event. Physics admission completed 8.100 s after Twin-open, and scene
participants became ready at 8.117 s. Terrain admission separately logged one
DEM request and two missing collider tiles; those individual holds cleared in
about 0.48 s total. `/api/ready` was first queried much later and was already
clear, so the readiness transition and five-second soak were not captured. The
run does not establish the 5 s full-Twin target. Typed API `Exit` was accepted;
the owned app exited and released ports 4102 and 8086.

Other one-off maxima in this capture were 105.814 ms for
`project_usd_policies`, 61.900 ms for `process_usd_sim_prims`, 47.935 ms in the
queued command phase of telemetry-index invalidation, 38.020 ms for the entity
tree ECS snapshot, and 35.947 ms in the queued visual command phase. The worker
tree derivation peaked at 4.480 ms. `PhysicsSchedule` averaged 5.057 ms and
peaked at 18.331 ms across 546 calls. These profiled spans overlap and are
contention-affected; they identify the next preparation/commit boundaries to
review and are not acceptance figures.

The window-creation system ran 2.176 s after process start, and the authored
Twin scene opened at 2.609 s. Physics admission completed 8.072 s after
Twin-open, with scene participants ready at 8.088 s. The readiness transition
was not sampled continuously, so the 5 s target is not established. Sibling
Apollo and Griffin sessions were active during this diagnostic. Typed API
`Exit` was accepted and the owned process exited with code 0, releasing ports
4101 and 8089.

## Remaining blocker

The 150 FPS and sub-0.5 ms full physics-step targets remain unmet. The latest
contended no-Tracy Apollo diagnostic recorded 147.64 FPS p50 and 0.825 ms
physics-step p50; the 4d8 sandbox diagnostic recorded 69.78 FPS p50 and
1.014 ms physics-step p50. Every sampled physics step in both runs exceeded
0.5 ms. Apollo's strict full-readiness candidate began 3.966 s after Twin-open
and survived the five-second soak; sandbox's began at 10.906 s and also
survived the soak, missing the 5 s target. Scene mount was 0.332 s for Apollo
and 0.288 s for sandbox. Window-creation and health events were within 2 s in
both 4d8 runs, but a compositor-level mapped-window observation was not
captured. In the newer 8ad sandbox run, window creation was logged at 3.99 s
and the first health sample arrived at 32.97 s; scene-participant readiness
was 7.70 s after Twin-open. Its 41.76 FPS p50 and 3.459 ms full-step p50 are
also below target. The system was heavily loaded and concurrency was not fully
established, so clean acceptance of app, scene-mount, full-Twin-readiness,
FPS, and physics targets remains unverified.

The prior strict-readiness capture reached stable full readiness at 7.532 s
after Twin-open, after the 5.679 s scene-participant milestone and a later
readiness regression. It confirms that app readiness and Twin readiness need
separate clocks: the window and health response were inside 2 s, while full
Twin readiness exceeded 5 s. The newer contact-hook run did not capture the
exact readiness transition; its first health sample was late, and readiness
was pending near 20 s before a clear sample near 45 s. The subsequent
4d8b139f0 Tracy run captured the regression: an early clear state at 4.752 s
after Twin-open later regressed, and the stable candidate began at 5.573 s,
missing the target by 0.573 s. The 4d8 unprofiled, contended run reached stable
readiness at 3.966 s for Apollo but 10.906 s for sandbox. Health returned
within 2 s in both, but mapped windows were not independently observed. In
the 8ad sandbox run, readiness was directly observed later and the five-second
stable interval was confirmed only after 127.875 s from process start; the
exact transition was not captured. These diagnostics do not establish clean
acceptance.

The strongest observed UI stall remains the historical 343.400 ms
`sync_twin_overlays` call whose `usd_twin_projection_refresh_dependent_stage_assets`
child reached 340.479 ms while a component layer changed; earlier overlay calls
reached 260.85/277.76 ms. The current source moves recipe copying and plan
composition to bounded worker admission, but the latest capture again found no
changed bytes and did not exercise that path. The stall reduction is therefore
unmeasured. These captures do not establish steady 150 FPS. An earlier
child-span trace also recorded a 23.529 ms first-use `/Traverse`
`CollisionGroupTable` lookup; a clean, settled profile is still needed before
changing that path.

The newest post-capture `PhysicsPerformance` query returned p50 2.781 ms,
p95 3.751 ms, p99 4.168 ms, and max 4.643 ms across 120 recent steps, all
above 0.5 ms. Its concurrent Griffin workload and Tracy overhead prevent an
acceptance claim. The USD contact-modification span had no calls in the
contact-hook capture; Avian `update_narrow_phase` averaged 0.341 ms with a
0.794 ms maximum in the 4d8b139f0 trace. Clean settled evidence is still
needed to determine the owner-level contributions to full-step time.

The workspace resolves Avian 0.7.0, still the [latest stable upstream
release](https://github.com/avianphysics/avian/releases/tag/v0.7.0). The
upstream `main` head checked on 2026-09-27 is
`af7e99c5c1da386aaa2b4c330e2adc96fa7ca6eb` (`0.8.0-dev`); its [contact
recycling implementation](https://github.com/avianphysics/avian/blob/af7e99c5c1da386aaa2b4c330e2adc96fa7ca6eb/src/collision/narrow_phase/system_param.rs#L541-L648)
reuses contacts for pairs whose relative motion stays within configured
thresholds, skipping manifold regeneration and recomputing when motion is
larger. This is a promising narrow-phase A/B candidate, not a measured win on
Apollo. The candidate's recycling path converts relative positions through
`f32`; audit that precision boundary and the crate API changes against this
workspace's f64 physics contract before pinning or adopting it. No dependency
change has been made.

The maintained BigSpace dependency is pinned to the reviewed `bevy-0.19`
revision available here (`5f255228e9b4…`). Application admission skips stable
propagation frames, and the 4d8b139f0 trace measured the active
high-precision pass at 0.314 ms mean (6.987 ms max self time) across 3,387
calls. Because the Apollo scene still had 23 dynamic bodies, this does not
establish a stable no-change gate regression. The remaining render/update
budget needs a clean, settled measurement against the 6.67 ms frame budget for
150 FPS. Further per-entity fan-out reduction belongs in the maintained
BigSpace owner, not an application duplicate or degraded path.

At 12:41 CEST on 2026-09-27, a fresh pre-profile check found other simulator
workloads listening on API ports 4147, 3864, and 3865; another simulator
process in the `lunar-soil` checkout was also using about 237% CPU. Host load
was 13.19/24.27/21.53, GPU utilization was 98% with 5.8/8.2 GiB occupied, and
the checkout had 3.4 GiB free on `/home` with a 17 GiB `target/`. No new build
or capture was started under this contention and disk pressure. These sessions
were left untouched; recheck before the next profile.

### 2026-09-27 — prepared policy lookup and startup follow-up

`UsdStageProjectionPlan` now builds a composed-type index during asynchronous
USD preparation. The policy projector reads only indexed `LunCoPolicy` prims
from a prepared plan and caches the resulting facts by stage asset, prepared
plan identity, and canonical generation. Live canonical reads remain on the
main thread. The focused runtime check and Tracy production build passed; no
tests were run.

The follow-up `target/sss-apollo-policy-stage-kind-20260927.tracy` capture
(15.34 s, 558 frames, 2,459,379 zones) still found one live-stage traversal in
`project_usd_policies`: 70.029 ms. There were two authored-fact cache hits and
one miss; `project_usd_policies` peaked at 70.613 ms. The prepared-plan index
does not remove this first live-stage scan, so the capture does not demonstrate
that the long UI stall is solved. The earlier 20.30 s indexed-policy capture
reported 9.4 s from process start to `scene participants ready`; its first
`/api/ready` sample arrived 12.2 s after process start and was already green,
so the exact readiness transition remains uncaptured.

In the 15.34 s run, the window appeared 1.410 s after process start, the Twin
scan was queued at 1.527 s, and the Twin opened at 1.598 s. Physics admission
and scene participants became ready at about 17.38 s, after the Tracy capture
ended. Tracy overhead and concurrent workloads make this diagnostic only; the
5 s Twin-readiness target remains unproven and was not met in this run.

### 2026-09-27 — bounded parallel USD layer reads

The USD stage loader now fetches independent sibling layers in bounded batches
of up to 16 reads. It processes the completed reads in authored dependency
order, so recipe contents and missing-layer diagnostics do not depend on I/O
completion order. Labeled read receipts retain Bevy's source-change reload
dependency graph. Native USD composition and projection-plan extraction then
run on Bevy's existing `AsyncComputeTaskPool`, keeping CPU preparation off the
asset I/O worker. The browser path still composes synchronously on its main
thread pending Web Worker transport. No renderer or physics-cycle code changed.

`cargo check -p lunco-usd-bevy-stage -p lunco-usd-bevy -j 4` passed without
warnings; no tests were added or run. A production startup run was not started:
ports 4101/8089, 4147, and 3868 are occupied by active simulator sessions,
including the checkout's `target/debug/luncosim`, and `/home` has 1.2 GiB free.
Both changes have compile evidence only; their Apollo startup effect and the
5 s Twin-readiness target remain unmeasured.

### 2026-09-27 — skip unrelated USD policy rebuilds

The policy projector now retains the set of composed `LunCoPolicy` prim paths
with its stage facts. When `UsdSceneChangeBatch` messages cover every canonical
generation since the cache, it promotes the cache without extracting policy
facts if changed subtrees and info paths do not intersect policy prims. That
also skips source-path resolution and policy registry installation for an
unrelated stage edit. Plan replacement, generation gaps, missing batches, or a
policy-affecting edit retain the full extraction path. Live-stage reads stay on
the owning thread.

`cargo check -p lunco-luncosim-runtime -j 4` passed without warnings; no tests
were added or run. This path was not reprofiled. The checkout app still holds
API port 4101 and Tracy port 8089; other active sessions hold 3743, 4147, and
3868, so they were left untouched. `/home` reached 0 bytes free during the
check; only this checkout's `lunco-luncosim-runtime`, `lunco-modelica-core`,
`lunco-modelica-execution`, and `lunco-luncosim-core` build outputs were
cleaned. A later state check found the checkout's `target/` is a broken
self-link and PID 1849321's executable points to a deleted file under Trash.
No application process was stopped. Restore a valid checkout-local target
before the next build or capture, then recheck ports and disk.

### 2026-09-27 — dependent-stage refresh preparation off the app thread

After fast-forwarding `optimization` to local `main` at `59fb013a3`, commit
`6766ccadd` moved persistent component-source serialization, recipe overlay
copying, and projection-plan composition for dependent-stage refresh into
bounded `UsdPreparation` work. Source bytes are serialized once per document
revision and shared across dependent stages; identical layer bytes skip the
stage rebuild. The app-thread owner validates source revisions and the target
plan before the live-stage commit. `cargo check -j 4 -p lunco-luncosim --bin
luncosim` passed after integration. No new runtime or changed-byte profile was
captured, so the historical stall reduction remains unmeasured.

### 2026-09-27 — parallelize each USD dependency frontier

Commit `e74acf00a` changes the USD closure loader to discover every layer in a
breadth-first frontier before issuing bounded read batches, filling the existing
16-read budget across independent dependency branches. Results, labels, and
missing-layer diagnostics are applied in stable authored order; source-change
receipts remain attached to their child load contexts.
`cargo check -j 4 -p lunco-usd-bevy-stage -p lunco-usd-bevy` passed, and after a
diagnostic-order review `cargo check -j 4 -p lunco-usd-bevy-stage` passed. No
tests or startup capture were run, so the latency effect remains unmeasured. No
renderer or physics-cycle code changed.

### 2026-09-27 — seed cold policy facts from the prepared plan

When the policy cache is cold, `project_usd_policies` can seed policy facts
from the worker-prepared projection plan and promote them to the current live
generation only if every intervening `UsdSceneChangeBatch` is present and
proves that no policy prim changed. A missing batch, generation gap, or
policy-affecting edit retains full live extraction. The change is compile
checked with `cargo check -j 4 -p lunco-luncosim-runtime` and was not
reprofiled; startup and edit-time latency effects remain unmeasured. Renderer
and physics-cycle code are untouched. The handover remains uncommitted.

### 2026-09-27 — skip topology rebuilds for transform-only edits

The scene-change batch now marks info-only paths whose changed properties are
exclusively transform opinions. The joint-topology cache advances its observed
generation for those changes without re-reading the whole composed stage;
structural resyncs and mixed property edits still trigger the normal refresh.
`cargo check -j 4 -p lunco-usd-bevy-runtime-core -p lunco-usd-sim` passed.
No tests or runtime profile were run, so the expected spike reduction remains
unmeasured. The handover remains uncommitted.

### 2026-09-27 — skip domain synthesis invalidation on transform edits

The Modelica domain-root reverse index now ignores transform-only info changes;
resynced paths and other changed member/root properties continue to invalidate
affected networks. `cargo check -j 4 -p lunco-usd-sim-domain` passed. No
runtime capture was run, so the startup and edit-spike effects remain
unmeasured. The handover remains uncommitted.

### 2026-09-27 — reuse prepared authored-telemetry indexes at commit

Source review found a cache-key mismatch in the generation-zero commit path: the
worker builds the authored telemetry index under prepared-plan identity, while
ordinary result publication previously requested a canonical-generation key
even though `CanonicalStages` selected the prepared plan. Commit now uses the
prepared plan and identity-keyed entry whenever the committed read generation
is zero, including runtime-instance plans; later canonical generations retain
their generation-keyed live reader and cache. This is a source-verified
correction, but the 21.426 ms `domain_telemetry_index` event in the preceding
capture did not record its read scope, so it cannot be attributed to this
mismatch.

This trace is diagnostic and affected by Tracy/profiler overhead and concurrent
workloads. The code change has not yet been reprofiled; no performance
acceptance is claimed. `cargo +nightly-2026-02-27 check -j 4 -p
lunco-usd-sim-domain` passed. No tests were run. This handover remains
uncommitted.

### 2026-09-27 — bound domain membership discovery per update

The same Tracy capture recorded three app-thread
`resolve_member_classes` calls at 23.370, 22.503, and 20.871 ms. The system has
no nested spans for separating entity arrival handling from composed membership
reads, so these are system-level diagnostics rather than precise sub-operation
attribution. Bootstrap discovery now snapshots entity IDs once and resolves
them in sorted batches of at most 64 per app update, leaving remaining
candidates queued. Entity ordering within each batch is stable; network facts
and source-loading semantics are unchanged. This aims to cap a single UI-frame
stall. The post-change diagnostic below still recorded a 42.903 ms maximum, so
it does not establish a lower worst-case duration or a readiness improvement.
`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-sim-domain` passed. No tests
were run. No physics or renderer code changed.

### 2026-09-27 — domain-cache and bounded-discovery diagnostic

The post-change Tracy capture `target/sss-domain-cache-batch-post-20260927.tracy`
ran for 50.07 s (689 frames, 5,006,038 zones) on the authored Apollo scene.
It overlapped active Apollo/Griffin simulator sessions and Cargo builds in
other checkouts; readings are contention- and profiler-affected diagnostics.
The owned app used API port 4318 and Tracy port 8086, then exited via typed
`Exit`; both ports were released. Other sessions were left untouched.

The window-created event was 0.885 s from process launch. `StartupScene opened`
was logged at 19:47:39.694Z. Scene participants became ready at 19:48:01.429Z,
about 21.734 s after Twin-open, missing the 5 s target. The first `/api/ready`
poll began at 19:48:11.478Z and showed zero pending items; all 50 samples
remained clear, but the transition was not captured. The health response was
available during startup, but its response time was not timestamped. Scene
mount and clean FPS/physics acceptance were not measured.

`resolve_member_classes` ran 1,020 times; its 95th percentile was 0.193 ms and
its maximum was 42.903 ms. The 64-candidate batch change does not yet establish
a lower maximum because this was one contended run and the system has no nested
spans for its internal work. `domain_telemetry_index` still had one 21.072 ms
app-thread event. Its scope was not recorded, so this capture does not show
whether the generation-zero cache correction was exercised; live canonical
generations still require their generation-keyed read surface.

The clearest remaining app-thread stall was
`usd_sim_prepared_topology_reconcile` at generation 1 with no captured change
history: 24.896 ms, including a 20.851 ms `usd_sim_joint_topology_scan`. This
is the full live-stage recovery path. The topology owner now checks whether its
existing index already proves the current canonical generation; when that proof
exists, it discards the late prepared result without another scan. A dirty or
behind index still takes the current live refresh for correctness. The
`usd_sim_prepared_topology_cache` Tracy span records whether the guard fired;
the next capture should compare it with `usd_sim_joint_topology_scan`. This
change has not yet been compile-checked or reprofiled.

### 2026-09-27 — reuse domain-root synthesizer selection

CSV export of the same capture shows `project_domain_islands` at 14 calls,
393.540 ms total self time, and 122.158 ms maximum self time (135.004 ms
inclusive maximum). Its nested live `domain_network_read` reached 12.618 ms
inclusive and `domain_synthesis_input_snapshot` reached 12.846 ms, so the
system's largest app-thread interval is not explained by those network reads.
The worker-side `domain_synthesis_decode_validate` reached 240.176 ms and
`domain_rhai_synthesis` 86.803 ms; these are asynchronous synthesis work, not
the app-thread stall.

Source review found that `project_domain_islands` selected the owner by walking
the full component collection and querying both role schemas after
`resolve_member_classes` had already visited those members. Discovery now
derives the selection while it reads the member roles and stores it with the
root's indexed facts. Projection reuses that selection until root/member
invalidation or instance-plan rediscovery. The capture did not include a
selection-specific span, so this source-attributed duplicate is a targeted
optimization rather than a proven explanation of the full 122 ms outlier. The
change adds `domain_member_role_discovery` and
`domain_synthesizer_selection_cache` spans; compare them with the enclosing
system after a focused compile check and post-change Tracy capture. No runtime
improvement is claimed.

### 2026-09-27 — budget queued visual child-key preparation

The earlier `process_queued_usd_visuals` trace recorded a 58.670 ms maximum
during a revision-triggered projection, without nested spans that attribute the
time to child-key preparation. Source review found that duplicate-child keys
were collected for every queued parent before the system checked its per-frame
projection budget. The key space is scoped to one parent, so visual projection
now clears and populates a system-local scratch set only for the parent admitted
after the budget check. Authored-path queue ordering is unchanged. The new
`usd_visual_existing_child_keys` span will separate this work in a later Tracy
capture. A single admitted parent with very high fanout still performs its
direct-child scan synchronously; this change does not claim to bound that case.

The focused `lunco-usd-bevy` compile check passed after the system-local scratch
set change. No tests or post-change Tracy capture were run, so this source-level
reduction in pre-budget work has no measured frame-time or readiness result.
No renderer or physics code changed.

### 2026-09-27 — reduce live policy type lookup work

The dependent-stage Tracy capture contains two app-thread
`usd_policy_live_stage_traversal` events at 24.051 ms and 24.245 ms. The live
reader previously materialized every composed prim path and then issued a
separate composed type lookup for each path. Policy extraction now uses the
existing typed `prim_paths_matching` reader seam, which finds `LunCoPolicy`
prim paths during the single live traversal and avoids the second per-prim
lookup pass. The live stage remains authoritative; this does not make the live
traversal incremental or asynchronous. The existing span will quantify the
change in a later capture. No post-change profile or runtime improvement is
claimed.

### 2026-09-27 — defer fixed-step telemetry metadata allocation

The saved domain-cache/bounded-discovery trace has a
`retain_physics_telemetry` event at 453.531 ms on the fixed post-step cycle.
That event has no nested attribution, and the capture was contention- and
profiler-affected. Source review found that each sampled body constructed owned
presentation strings for every channel even when the existing channel metadata
was unchanged and immediately discarded. Telemetry samples now keep static
presentation labels borrowed and build the owned `SignalPresentation` only
when the registry metadata is absent or dirty. Signal names, values, metadata,
history rates, and fixed-step sampling cadence are unchanged. No post-change
profile is available, so no latency reduction is claimed.

### 2026-09-27 — use API-schema matching for celestial source detection

The saved domain-cache/bounded-discovery trace recorded a
`project_celestial_comms_prims` app-thread event at 148.063 ms. Its internal
work was not split into spans. Source review found the scene-root
classification traversed every prim path and then queried the celestial API
schema separately for each prim. It now uses the composed reader's existing
API-schema matching query, which checks the schema during traversal and returns
only matches. This remains an unprofiled source-level reduction in repeated
lookups; no improvement to the full system duration or startup readiness is
claimed.

### 2026-09-27 — prepare Rhai source ASTs off the I/O worker

`RhaiSourceLoader` read the source and registered its literal import handles,
then compiled the source AST before returning on Bevy's asset I/O worker. Native
loads now await that CPU-heavy AST preparation on `AsyncComputeTaskPool` after
registering the dependency handles, allowing independent imports to load in
parallel with compilation. The wasm path remains synchronous. Asset identity,
compile limits, errors, and the dependency-ready runtime activation boundary
are unchanged. `cargo +nightly-2026-02-27 check -j 4 -p
lunco-scripting-rhai-world` passed. No tests or runtime/profile acceptance were
run, so no startup-time improvement is claimed.

### 2026-09-27 — use the prepared type index for collision groups

The saved `sss-domain-cache-batch-post-20260927.tracy` capture contains 800
`usd_avian_collision_groups` calls for `/Traverse`, totalling 22.512 ms with a
21.886 ms maximum. Two earlier captures show maxima of 25.001 ms and 26.906 ms.
The cached table was also deeply cloned on every projected prim, although the
extractor only borrows it. The caller now holds a reference instead. On the
first read, `CollisionGroupTable::read` had traversed every prepared prim and
queried its type individually before selecting `PhysicsCollisionGroup` prims;
it now requests that type through `UsdReadObject::prim_paths_matching`, which
uses the prepared projection plan's type index and preserves the existing
deterministic path sort and table semantics. Live-stage readers still use the
shared reader's composed traversal. Per-prim collection membership now uses
`str::strip_prefix` and a slash-boundary check instead of allocating a formatted
prefix string for every include and exclude. The focused
`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-avian -p
lunco-usd-avian-filters` passed. No tests or post-change profile were run, so
the saved captures remain diagnostic and no latency reduction is claimed.

### 2026-09-28 — bound USD simulation-prim command batches

The same capture recorded 64 `process_usd_sim_prims` calls. The longest
inclusive system event was 46.959 ms, with a 31.699 ms deferred-command flush;
its `usd_sim_prim_projection_batch` covered as many as 302 prims, while the
attributed projection span peaked at 1.989 ms. Source review confirmed that the
owner drained the entire queued scene population in one Update. It now admits
up to 32 prims in stable USD-path order per Update and requeues the remainder,
so command application cannot accumulate an unbounded scene-wide batch. The
existing topology/readiness owners still gate physics admission. Authored
diagnostics accumulate across the queued projection batches and publish when
that queue drains; scene teardown clears the pending diagnostic snapshot. The
same capture's `usd_sim_topology_refresh` still reached 46.623 ms, including a
20.851 ms `usd_sim_joint_topology_scan`; this entity batch limit does not bound
the separate stage-level topology fallback. The saved capture is pre-change
and contention/profiler affected. The focused
`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-sim` passed. No tests or
post-change timing/runtime profile were run, so the saved capture remains
diagnostic and the effect on frame time/readiness is not measured.

### 2026-09-28 — reuse canonical prepared snapshots after dependent refresh

Source review found that dependent-stage refresh constructs a live
`CanonicalStage` and a worker-prepared plan from the same recipe, then publishes
the plan through `Assets::get_mut`. Bevy emits `AssetEvent::Modified` when that
mutable asset borrow ends; `sync_canonical_stages` previously reopened the same
recipe during the next Update. The topology projector also treated the rebuilt
stage's nonzero generation as unrelated to its prepared plan and could take the
full live-stage scan when no edit batch intervened.

`CanonicalStages` now records the exact prepared-plan identity and generation
for a live stage built from that snapshot. The asset sync reuses the stage when
that identity is still current; topology commit may use the worker facts at the
recorded nonzero generation. Any later live edit advances the generation and
invalidates the match, preserving complete-history validation and the live
refresh when needed. The focused stage/runtime/simulation `cargo check` passed;
no tests or runtime profile were run. This source-verified reduction has no
post-change timing measurement yet. Tracy spans
`usd_canonical_stage_asset_sync`, `usd_sim_prepared_topology_cache`, and
`usd_sim_joint_topology_scan` now expose both reuse decisions and any remaining
live extraction. A fresh profile was not started: `/home` was 98% full with
5.9 GB free, and four other LunCoSim sessions held ports 4157, 47125, 47126,
and 48473. Recheck disk and ports before profiling.

### 2026-09-28 — startup trace and telemetry projection follow-up

The app-ready objective is 5 s from process launch; full Twin readiness is
5 s from Twin-open; scene mount remains its separate 2 s checkpoint. Earlier
2 s app-readiness thresholds below describe their dated measurements, not the
current acceptance threshold.

The Tracy-featured production build from the local `main` checkout passed.
Two Apollo High-quality captures were recorded with the profiler active and
other simulator work on the machine, so they are diagnostic only. The first,
`../main/target/sss-startup-20260928.tracy`, ran for 65.4 s (4,389 frames,
21.03 million zones). Full readiness still had 12 pending items at about 20 s
and was clear by roughly 45 s; the exact transition and five-second soak were
not sampled. The warm follow-up,
`../main/target/sss-warm-readiness-20260928.tracy`, ran for 45.31 s (1,652
frames, 9.37 million zones); readiness was already clear at the first poll,
about 14 s after Twin-open, so its transition was also missed. These runs do
not establish the 5 s app or Twin readiness targets; the observed full-Twin
readiness was later than 5 s. Sandbox was not measured.

The warm run confirmed persistent Modelica solve-cache reuse: eight disk
lookups averaged 2.81 ms (5.29 ms max), with no `lower_for_live` spans. Cold
lowering remains part of first-open work. `sync_twin_overlays` did not reproduce
the earlier long calls in this sample (nine calls, 29 microseconds maximum).
The first run's Tracy-only physics diagnostics were 2.47 ms p50, 3.24 ms p95,
3.75 ms p99, and 3.87 ms max across 120 samples with 281 bodies and 18 joints;
these do not meet the 0.5 ms target and are not clean acceptance values.

Other app-thread spans worth follow-up were `project_celestial_comms_prims`
(49.32 ms maximum), `project_usd_telemetry` (4.71 ms mean, 9.31 ms body maximum,
with deferred-command flushes up to 143 ms), and
`process_queued_usd_visuals` (8.49 ms body maximum, with deferred-command
flushes up to 82.5 ms). The telemetry invalidation marker/channel removal
flush reached 102 ms. `project_authored_runtime_components` averaged 3.86 ms
and reached 36.3 ms max over 47 calls; its initial owner path separately
acquired a canonical reader and enumerated the same owner's children for
controls and generic programs. `domain_synthesis_decode_validate` reached
82.8 ms on the synthesis worker. These are contention- and profiler-affected
outliers; attribute each at its owner before changing behavior. Renderer spans
were excluded from this work.

The current source change coalesces USD telemetry invalidation during staged
scene admission: clear derived markers/channels once per dirty interval and
defer the full index pass until no prim is awaiting its stage or queued for
structural projection. It retains the processed marker on prims without a
telemetry declaration, so later index rebuilds do not reread every non-
declaration prim. The focused compile passed; a matching post-change profile
is still required. The captures used owned API ports
43451 and 43452 and those sessions were closed cleanly; external sessions on
4157, 47125, and 47126 were left untouched.

The follow-up source change reuses one canonical owner reader and one child-
path snapshot to resolve both authored controls and generic programs during
initial projection. The live program-refresh path uses the same typed resolver
and commit helper. The focused
`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-bevy-authored-runtime`
passed on 2026-09-28. No tests or post-change Tracy capture were run, so its
timing effect remains unmeasured.

`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-sim-cosim` passed on
2026-09-28. No tests or post-change runtime profile were run; acceptance remains
unverified. Telemetry admission coalescing is committed as `bb4eab1b5`; authored
runtime owner-read reuse is `b68da02ba`. The latter was merged with local main
commit `382db427a` as `52a5922c3`, then `optimization` was fast-forwarded to
that merge. No push was made. This handover remains uncommitted.

### 2026-09-28 — engine dataset startup scan off the app thread

The pre-change Apollo captures measured the synchronous
`scan_engine_manifests` startup system at 25.05 ms and 78.08 ms. It enumerated
manifest files, read and parsed TOML, and checked delivered-artifact presence
on the app thread. Commit `a82dca70d` moves that complete scan to Bevy's
async-compute pool; the app schedule merges its typed registry snapshot and
publishes `DatasetScopeReady` after completion, with cross-registry duplicate
and processing-output conflict checks still enforced at commit.

The post-change Tracy-enabled production build and 25.27 s Apollo High-quality
capture used `target/datasets-scan-offthread-20260928.tracy` (979 frames,
5,263,276 zones; API port 43457). The trace recorded one
`dataset_engine_manifest_scan` span at 90.60 ms on worker thread 12 and no
`scan_engine_manifests` app-thread system span. The worker span includes
filesystem discovery, manifest reads/parsing, and artifact checks; its longer
duration than the pre-change app-thread samples does not imply a CPU-time
reduction. It confirms the work moved off the app thread. The run was
profiler-affected with another simulator on port 47127, so this is diagnostic,
not clean responsiveness or acceptance evidence. `/api/ready` was green at the
first poll only; the transition and five-second soak were not sampled. Sandbox,
FPS, and physics targets were not measured. Typed API `Exit` was accepted and
owned ports 43457/8086 were verified closed; port 47127 was left untouched.

The local `main` and `optimization` branches now both point to `a82dca70d`.
The update was fast-forwarded locally; no push was made. This handover remains
uncommitted.

### 2026-09-28 — ordered visual admission and environment fact reuse

Commit `6728b79f5` replaces repeated collection and sorting of pending USD
visuals with a path-ordered queue plus entity membership tracking. A diagnostic
30.3 s Apollo High capture (`target/usd-ordered-visual-admission-20260928.tracy`,
1,751 frames, 9.17 million zones) measured 90
`process_queued_usd_visuals` calls: 1.68 ms p50, 8.49 ms p95, 9.59 ms p99,
20.04 ms max. The preceding sort-path capture had 76 calls over 30.31 s: 3.24
ms p50, 8.44 ms p95, 31.94 ms p99, 33.16 ms max. Call counts and workload
differed, so this is directional diagnostic evidence, not a controlled A/B or
FPS result. The captures were Tracy-instrumented and contention-affected.
In the post-change log, the window was created at `01:04:38.374538Z`, Twin
open began at `01:04:38.787186Z`, physics admission completed at
`01:04:43.107083Z`, and scene participants were ready at
`01:04:43.118124Z`. These are 0.41 s from window to Twin-open, 4.73 s from
process start to physics admission, and 4.74 s to scene-participant readiness.
The first `/api/ready` poll was green at 14.38 s from process start and stayed
clear for a five-second soak. Polling began too late to capture the transition;
these observations do not establish the 5 s API or full-Twin readiness targets.

The same post-change trace measured `usd_environment_type_lookup` three times
(47.47 ms mean, 72.48 ms max) and `project_celestial_comms_prims` at 49.32 ms
max. Commit `1c53441af` caches authored environment exposure/bloom facts and
matching prim paths against the exact stage-plan `Arc`. It reuses them across
transform-only batches and info edits outside those prim paths; relevant edits,
resyncs, generation gaps, and plan replacement trigger a fresh composed read.
The system compares its stage signal in place to avoid allocating a roots
vector on steady updates. The focused
`cargo +nightly-2026-02-27 check -j 4 -p lunco-luncosim-presentation`
passed. No post-change Tracy capture is available, so the timing effect is
unmeasured.

The Tracy production build attempted from `optimization` failed when the
filesystem ran out of space before producing a new binary or capture. `cargo
clean` then removed that checkout's 6.2 GiB of build outputs; the `main`
checkout's target and running process were left untouched. The separate owned
profile session was not started. Port 47131 remained an external simulator and
was not controlled. At the end of this work, `main` and `optimization` both
point to `1c53441af`; the local fast-forward was not pushed. This handover
remains uncommitted.

### 2026-09-28 — skip irrelevant celestial prim decoding

The same trace's `project_celestial_comms_prims` event reached 49.32 ms, but
the system had no internal attribution. Source inspection found that it read
celestial and connectivity fields for every newly admitted scene prim,
including ordinary prims with only standard USD properties. Commit
`eb3352b07` keeps the completion marker on every prim while skipping those
field reads unless the prim is the scene root, has a `lunco:` property, is a
`DistantLight` (parent-body fill), or carries `LunCoEpochAPI` (misplaced-root
diagnostic). The focused
`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-sim-celestial` passed. No
post-change profile is available, so the system timing effect is unmeasured.
`main` and `optimization` now both point to `eb3352b07`; the change was
fast-forwarded locally and not pushed. This handover remains uncommitted.

### 2026-09-28 — move Twin policy source reads off the app thread

An earlier Tracy export recorded `sync_policies_on_twin_added` at 25.513 ms
max. That is an outer-system duration without child attribution, so policy file
I/O was a candidate source, not a proven 25 ms attribution. Commit `62bd98a42`
snapshots only indexed policy-manifest paths, reads and parses the manifest and
declared Rhai sources through shared bounded native worker admission, then
validates Twin id/root/operation and activates the typed bundle in the
`Lifecycle` lane. The active Twin's `assets_mounted` plan waits for activation;
the root-mounted event remains immediate for other consumers. A
`TwinPolicyPreparation` progress hold keeps the first fixed tick and full
readiness behind that commit while the app/UI schedule continues. Close or
switch cancels queued work and fences running results. Browser builds retain
their existing synchronous WebStorage path until worker transport is available.

The native focused check passed for `lunco-assets-runtime`, `lunco-core-runtime`,
`lunco-scripting`, `lunco-scripting-rhai-world`,
`lunco-scripting-rhai-runtime`, and `lunco-luncosim-exposures`; the
`native-plugins` feature check also passed. The wasm-target check stopped in
`getrandom 0.3.4` before the modified crates compiled, because that dependency
requires its `wasm_js` configuration. The Tracy production build ran out of
filesystem space before creating a binary or capture; its partial `target/`
outputs were removed and the focused native check was rebuilt afterward. No
post-change Tracy timing or clean FPS sample is available, so the effect on
readiness and UI stalls remains unmeasured. No owned runtime session was
started. Port 47240 remained active in the separate `terrain` checkout and was
not controlled; ports 4101, 4317, 43463, 8086, and 47131 were free at the last
check.

`main` and `optimization` now point to `62bd98a42`; this fast-forward is local
and was not pushed. The handover remains uncommitted.

### 2026-09-28 — follow-up owner audit

Source review confirms the paused Rhai scenario pass is exclusive on the app
thread and can deliver queued events or run lifecycle startup while simulation
time is paused. Its historical 48.4 ms outer event has no child attribution, so
it does not identify a safe optimization target. The earlier primary Twin
prepared-plan binding for topology reuse was reverted; reintroducing that
shortcut without its correctness rationale would risk reading stale facts.

No current Tracy profile was captured. The existing
`main/target/debug/luncosim` reports build hash `6728b79f`, not the integrated
`62bd98a42`, so it is stale for post-change measurements. Rebuilding Tracy in
the optimization checkout previously exhausted `/home`; the latest check left
3.9 GB free. The active process on port 47240 belongs to the separate `terrain`
checkout and was left untouched. Capture again after a current Tracy binary is
available and the necessary disk space and ports have been rechecked.

### 2026-09-28 — bound projection batches and idle joint retirement

Commits `998ebb967`, `09013b50e`, `03f377866`, `a3074ebd4`, `4237f2ec0`, and
`543c7b3f6` are integrated in local `main` and `optimization`; none was
pushed. USD visual
projection now limits prim binding to 4 ms and 64 queued prim work items per
update, while direct-child admission remains capped at 1 ms and 128 children.
The 64-item diagnostic capture reached full Summer Space School readiness 3.40
seconds after `OpenTwin`; its prim command flush had p95 8.73 ms and max 21.57
ms. A 32-item trial reached readiness in 4.03 seconds but did not improve the
flush tail (p95 11.89 ms, max 27.87 ms), so the 64-item setting was restored.
These separate Tracy runs had varying CPU contention and are not a clean A/B.

The `JointAttachPlugin` now gates its exclusive retirement transaction on the
existing `PhysicsJointDetachRequested` marker. The prior capture contained
3,241 idle `retire_requested_joints` calls; the 45.37-second capture from
`543c7b3f6` contained zero. The same capture recorded `sync_twin_overlays` at
4.07 ms max, app API readiness at 1.88 seconds, and accepted the Summer Space
School Twin open request in 0.16 seconds. Full Twin readiness took 14.07
seconds, and `PhysicsPerformance` reported 4.31 ms current and 6.05 ms sample
maximum, so those readiness and physics targets were not met in this run.

The run was strongly contended: the separate Griffin session on port 47240 was
using about 177% CPU, with two persistent `luncosim --version` processes also
using CPU. Those sessions were left untouched. The capture had 2,037 frames and
10,443,278 zones. No uncontended non-Tracy FPS/physics acceptance run is
available; do not treat these timings as clean acceptance results. The owned
session exited successfully through API `Exit`, and ports 43501 and 8086 were
free afterward. This handover remains uncommitted.

### 2026-09-28 — gate idle joint-admission readiness

Commit `3f5617874` is fast-forwarded into local `main` and `optimization`; it
was not pushed. `synchronize_joint_admission_batch` now runs only while the
existing `PendingUsdJoint` or shared `PendingJointAdmission` marker is present.
That leaves one combined marker query in the idle Update path and skips the
scene-projection and solver-endpoint scans when there is no joint candidate.
The readiness reconciliation still runs before the admission commit whenever
an authored or native pending joint exists, preserving the existing batch
boundary.

`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-avian-joints` and
`git diff --check` passed. No tests or runtime profile were run, so this is a
source-level reduction with no measured timing claim. At the latest host
recheck the separate Griffin app on port 47240 was using about 175% CPU, and
disk space was 2.9 GiB; its session was left untouched. Clean 150 FPS, physics,
and readiness acceptance remain open. This handover remains uncommitted.

### 2026-09-28 — batch Modelica telemetry metadata lookup

The fresh 180-second Tracy capture
`main/target/sss-sandbox-startup-180s-3f5617874-20260928.tracy` covered
startup, Summer Space School, and Sandbox. Summer Space School first stayed
fully ready about 11.99 seconds after its `OpenTwin` handling began; Sandbox
first stayed fully ready about 17.17 seconds after its open handling began.
Both exceed the 5-second full-readiness target. Their scene roots were mounted
within 2 seconds. The Tracy/contention-affected sandbox physics samples were
4.10 ms current, with 3.47/4.64/5.25/5.32 ms p50/p95/p99/max; its 240-frame
sample was 17.84/28.21/32.78/42.31 ms. These are diagnostics, not clean
acceptance measurements, and miss the 0.5 ms physics and 150 FPS targets.

Tracy recorded `lunco_modelica_telemetry::retain_modelica_runtime_state` at
248.80 ms during Sandbox readiness and 87.57 ms during Summer Space School
readiness. Source attribution found a repeated linear
`ModelicaIndex::find_component_by_leaf` scan for each solver variable while
document metadata was refreshed, making the dirty batch proportional to
variables times document components. The index now exposes a borrowed
`ComponentNameLookup` for bulk exact/qualified-leaf resolution, and telemetry
builds it once for each metadata refresh batch. Settled samples keep using the
session metadata cache. This preserves first-match and exact-before-leaf
resolution semantics. The focused telemetry crate check passed. A post-change
source profile was captured at the Tracy listener on port 8086:
`main/target/sss-sandbox-modelica-lookup-2d386e156-20260928.tracy` (60.3 s,
4,595 frames, 20,006,355 zones, 125.19 MB). Its telemetry system had 4,317
calls at 0.0067/0.0203/1.8126/109.1282 ms p50/p95/p99/max; the paired
`system_commands` flush max was 0.3311 ms. The 109.13 ms and 68.40 ms system
outliers occurred during Sandbox readiness. This is not a controlled before/
after or clean acceptance comparison; the earlier outlier was 248.80 ms.

The same run's window-created event was 1.49 s after process startup; API
health was not sampled until 13.45 s, so app-ready timing is unverified.
Summer Space School full readiness first stayed clear at 2.826 s after
`OpenTwin` and remained clear for the five-second soak. Sandbox readiness
first stayed clear at 16.469 s and also passed the soak, missing the 5-second
target. Sandbox's Tracy-affected `PhysicsPerformance` sample at step 9716 had
50 bodies, 44 dynamic bodies, 33 colliders, and 32 joints; step-time samples
were 6.5451/8.8923/9.7685/9.9316 ms p50/p95/p99/max. The settled capture
frames averaged 76.2/s; this instrumented, contended rate is not acceptance.
The API reported 4,782 telemetry channels, including 2,113 Modelica channels
across 15 owners.

Source review found a second startup allocation lead: `ScalarHistory::new`
eagerly reserved all 1,500 default samples for every new channel before its
first sample. At 2,113 Modelica channels, those empty ring buffers alone reserve
about 48 MiB on the app thread during initial publication. The shared
`lunco-signal` owner now keeps the 1,500-sample logical bound while starting
each deque empty and letting it grow with recorded samples. This preserves
retention semantics while avoiding the full reservation for newly admitted
channels. `cargo +nightly-2026-02-27 check -j 4 -p lunco-signal` passed. A
post-change runtime/Tracy build was not run: only 23 MiB remained on `/home`
after the focused check. No timing effect is claimed until the owned runtime
can be rebuilt and profiled. This handover remains uncommitted.

### 2026-09-28 — reduce entity-tree snapshot scans

The 60-second Tracy capture attributed up to 16.17 ms to the synchronous
`entity_tree_view_snapshot` on the app thread (4.214/10.686/16.172 ms
p95/p99/max). The named-candidate query now reads `SystemManaged`,
`SelectableRoot`, and `Mesh3d` membership alongside each named entity. This
removes three separate population-query passes whose marker results were only
used for named tree nodes; hierarchy and grid snapshots remain separate. The
worker still performs hierarchy derivation and sorting. A focused UI-crate
`cargo +nightly-2026-02-27 check -j 4 -p lunco-luncosim-edit-ui` passed; no
post-change capture or timing improvement is claimed. Commit `54ad69a32` is
fast-forwarded into local `main` and `optimization`; it was not pushed. This
handover remains uncommitted.

### 2026-09-28 — remove telemetry marker command churn

The Sandbox Tracy capture for `54ad69a32` measured the
`project_usd_telemetry` system body at 2.58–4.31 ms around Twin admission, while
the paired app-thread `system_commands` flushes reached 40.42 ms and 59.68 ms.
An earlier invalidation flush reached 102 ms. The projector inserted a
completion marker for every inspected USD prim, including non-declarations,
and removed all those markers on invalidation. Those per-prim deferred commands
were avoidable on top of the required telemetry-channel publication.

Projection progress now lives in the telemetry index's `HashSet<Entity>`. A
dirty interval clears the set with stale channels, while unresolved wrapper
ports remain retryable. This removes the per-prim marker insertions and marker
removal batch without changing authored telemetry or channel ownership. The
focused `cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-sim-cosim` passed;
tests were not run. A post-change Tracy capture could not be built because the
focused check left about 856 MB free on `/home`, below the app link's observed
size. The existing capture is pre-change, profiler- and contention-affected;
no timing improvement is claimed. The active Griffin app on port 49732 belongs
to the `terrain` checkout and was left untouched. This source change is being
committed as `e0023246e` and fast-forwarded into local `main` and `optimization`;
it was not pushed. This handover remains uncommitted.

### 2026-09-28 — coalesce celestial prim admission

The Sandbox capture `sandbox-startup-54ad69a32-20260928.tracy` recorded
`project_celestial_comms_prims` at 4.87 ms, followed by a deferred
`system_commands` flush of 163.42 ms at trace time 60.90 s. The projector was
adding `CelestialProjected` to every arriving prim, while the only current
consumer checks that marker on the active scene root before seeding an authored
static sun.

The projector now uses the existing `PendingEntityWork` owner for its one-time
bootstrap and lifecycle arrivals, and it sorts queued entity IDs before
projection. Non-root prims no longer enqueue a completion-marker command; the
root marker remains after celestial-source classification, and unresolved stage
or root-path work remains queued for retry. The idle run condition now reads
the queue instead of scanning the USD-prim population. The focused
`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-sim-celestial` passed; tests
were not run. A matching post-change Tracy build was not attempted with 913 MB
free on `/home`, less than the existing 1.1 GB production executable before
link intermediates. No timing improvement is claimed. Commit `4f0975df9` is
fast-forwarded into local `main` and `optimization`; it was not pushed. This
handover remains uncommitted.

### 2026-09-28 — limit entity-tree hierarchy harvesting to relevant ancestors

In the `sandbox-startup-54ad69a32-20260928.tracy` capture,
`entity_tree_view_snapshot` still reached 55.34 ms after marker reads had been
combined into the named-candidate query. The remaining snapshot copied a
`ChildOf` entry for every scene entity and separately collected every `Grid`,
although tree derivation only reads hierarchy and grid membership along named
entities' ancestor paths.

The snapshot now reads each named candidate's parent chain through indexed
`Query::get` calls, deduplicates shared ancestors, and collects grid membership
on the same path. It checks the selected active grid directly. The existing
worker still derives visibility, ordering, and labels from the immutable
snapshot. `cargo +nightly-2026-02-27 check -j 4 -p lunco-luncosim-edit-ui`
passed. The first attempt hit `ENOSPC` while writing a dependency fingerprint
with 5 MB free; this checkout's target was cleaned, freeing 1.8 GiB, and the
focused check then passed on a fresh build. No post-change Tracy run is
available, so the timing effect is unmeasured. Commit `10efe5edc` is
fast-forwarded into local `main` and `optimization`; it was not pushed. This
handover remains uncommitted.

### 2026-09-28 — use prepared facts for terrain admission

The pre-change `sandbox-startup-54ad69a32-20260928.tracy` capture recorded
`lunco_usd_terrain::bridge_usd_dem_terrain` at 88.75 ms on the app thread
(trace time 60.77 s), followed by a 27.57 ms `system_commands` flush for the
same system. These are profiler- and contention-affected diagnostics. Source
review found that initial terrain projection could synchronously call
`CanonicalStages::get_or_build`, then read every newly mounted USD prim and
enqueue a `DemBridged` marker even when it carried no terrain data.

The bridge now uses `reader_for_entity`: generation-zero facts come from the
worker-prepared projection plan, while later authored generations use the live
canonical reader. A per-plan candidate index finds prims with
`LunCoTerrainAPI` or authored `lunco:` properties; the bridge inspects changed
prim paths and explicit dataset retries, and marks only resolved terrain
candidates. On live generations it classifies only the changed prim using the
terrain API and the two authored fields the bridge consumes, without rebuilding
a whole-stage index. Terrain layer parsing uses the shared `UsdRead` contract
for either reader. This removes the bridge's startup stage-open path and avoids
deferred completion commands for unrelated prims. No renderer code changed.

`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-terrain` passed. No tests or
post-change runtime capture were run, so no timing improvement is claimed. The
performance targets and clean runtime acceptance remain open. This handover
remains uncommitted.

### 2026-09-28 — event-drive render-free camera-path discovery

The Sandbox Tracy capture recorded 12,639 calls to
`lunco_usd_bevy_camera::camera_path::resolve_camera_paths` over about 90.25 s,
with 50.79 ms total, 4.0 μs mean, and one 17.14 ms maximum at trace time
61.321 s. The capture is diagnostic and contention-affected. The system-level
span does not attribute the outlier to a specific operation. Source review
found that each Update still read every unresolved `BasisCurves`, including
curves without a camera-path relationship.

Discovery now runs a one-time bootstrap, then revisits inserted prim identities,
canonical stage generations, and stage-asset changes. Only candidates waiting
for a runtime prerequisite retry between those signals. Spatial prerequisites
are checked before decoding authored track and curve data. The render-free
camera owner and cinematic-camera contract document this lifecycle; no renderer
code changed. The focused camera package check passed; tests and a post-change
capture were not run. Commit `9b86bf8ff` is fast-forwarded into local `main` and
`optimization`; it was not pushed. No timing improvement is claimed. This
handover remains uncommitted.

### 2026-09-28 — select the bounded USD simulation projection prefix

The Sandbox capture `sandbox-startup-54ad69a32-20260928.tracy` recorded
`usd_sim_pending_collect_sort` 74 times, with 57.74 ms total and a 26.66 ms
maximum. The diagnostic, contention-affected span covered draining, filtering,
and sorting the entire pending population before applying the existing
32-prim-per-Update cap. The topology refresh child span peaked at 0.035 ms, so
the measured spike came from queue preparation rather than stage topology
extraction.

The projector now uses selection to find the lowest 32 authored paths (entity
identity breaks path ties), sorts only that admitted prefix, and retains the
remainder in the existing pending queue. This preserves deterministic path
order and the per-Update command bound while replacing a full batch sort with
linear selection plus a small prefix sort. No renderer code changed. The
focused `cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-sim` passed without
warnings; a matching post-change Tracy capture remains to be run, so no timing
improvement is claimed. Commit `90fa387fe` is fast-forwarded into local `main`
and `optimization`; it was not pushed. This handover remains uncommitted.

### 2026-09-28 — prepare exact live USD topology snapshots off Update

The post-`90fa387fe` Apollo Tracy capture recorded
`usd_sim_prepared_topology_reconcile` at canonical generation 1 with no change
history and no exact prepared-plan match: 34.024 ms, including a 34.098 ms
`usd_sim_joint_topology_scan`. The enclosing whole-scene history-gap rebuild
started near trace time 3.401 s; its live-stage build was 0.465 ms and visual
refresh 1.490 ms. This attributes the observed main-thread stall to topology
reconciliation after the rebuild, not to rebuilding the canonical stage. The
capture was profiler- and contention-affected and is not an acceptance result.

The USD simulation owner now prepares topology from an immutable recipe
snapshot of the exact live canonical generation on the worker pool when no
matching prepared plan exists. It admits at most four stage tasks and commits
facts only when both the asset-plan identity and canonical generation remain
current. Stale results are discarded; simulation-prim projection stays queued
until the index is ready. The mounted root holds startup progress only until the
first topology index commits. Later generations retain the last committed
index, so existing simulation continues while new or reprojected prim admission
waits for replacement facts. A current-generation preparation error faults
the scene and applies its safety hold. This removes the synchronous whole-stage
topology fallback from Update without changing renderer code or physics
equations.

The focused
`cargo +nightly-2026-02-27 check -j 4 -p lunco-usd-sim -p lunco-luncosim-exposures -p lunco-scripting`
passed after the last live-refresh adjustment. A Tracy-enabled production build
and capture were made just before that adjustment; the initial topology path
measured below is unchanged, but live-generation retention has not had a
post-adjustment runtime pass. The Apollo capture,
`luncosim-usd-sim-topology-post-106031a97-20260928.tracy`, contains 60.3 s,
2,103 frames, and 9,088,070 zones. At generation 1, the owner-thread
canonical snapshot peaked at 0.437 ms; worker projection-plan and topology
preparation peaked at 488.9 ms; the worker joint scan peaked at 0.411 ms. The
owner `usd_sim_topology_refresh` span peaked at 0.474 ms over 126 calls. These
are profiler- and contention-affected diagnostics. Other simulator/Griffin
workloads were active; this is not clean FPS or physics acceptance. Twin open
was accepted in 22 ms. The first readiness sample showed 12 pending items;
readiness was later stable-clear for five seconds, but the exact transition was
not captured, so the 5 s Twin-readiness target is unverified. Sandbox remains
unmeasured.

Tracy also showed an 84.95 ms `system_commands` flush associated with
`process_usd_sim_prims`. In that capture it followed a four-prim batch whose
projection body took 8 microseconds and emitted no per-prim simulation spans;
the other available captures peaked at 23.7–28.7 ms. This isolated outlier is
not enough evidence to change the existing bounded batch.

The one-time `load_application_policies_on_startup` span reached 108 ms. Native
startup now reads the manifest and policy sources, compiles policy hooks, and
evaluates the authored hook-order selector on one async-compute worker during
runtime plugin construction. `PreStartup` validates that typed order and
commits prepared hooks before Startup consumers. Three Tracy startup captures
show the progression: the initial worker split had 37.8 ms source preparation,
61.4 ms policy compilation, 27 microseconds waiting in `PreStartup`, and 107.3
ms activation; passing only hook identities through Rhai reduced activation to
33.5 ms; moving authored order selection to the worker reduced activation to
12.5 ms and the wait to 7 microseconds. In that final capture, worker source
preparation took 39.3 ms and hook compilation plus order selection took 166.3
ms, overlapping runtime plugin construction. The app logged 24 installed
policies and `/api/ready` returned ready without a fault, but the exact app
readiness transition was not polled from process launch. Captures were
profiler- and contention-affected with other simulator sessions active; they
are diagnostic, not clean acceptance. Captures
`luncosim-policy-startup-20260928.tracy`,
`luncosim-policy-startup-compact-20260928.tracy`, and
`luncosim-policy-plan-20260928.tracy` are in `/var/tmp` and are not committed.
No renderer code changed. Clean FPS, physics, and readiness
acceptance remain open. This handover remains uncommitted.

### 2026-09-28 — reuse Modelica telemetry identities on due samples

The `2d386e156` Sandbox capture measured
`retain_modelica_runtime_state` at 109.13 ms maximum (with a separate 68.40 ms
outlier during readiness) after batching component-name lookup. That profile did
not identify the remaining body cost. Source review found that each due
Modelica sample rebuilt an owned `SignalRef` and cloned its path before the
registry looked up an existing channel.

Modelica runtime telemetry now caches each eligible variable's `SignalRef` and
metadata in the solver-session record. `SignalRegistry::record_scalar_at_rate` borrows
the identity for existing channels, avoiding per-sample path allocation; new
channels still enter through the same registry insertion and history policy.
Session changes clear histories only for variables that were actually
retained. Rate, retention, time-reversal, and metadata semantics are unchanged.
The compile-only check
`cargo +nightly-2026-02-27 check --locked --tests -j 4 -p lunco-signal -p
lunco-modelica-telemetry -p lunco-usd-sim-telemetry` passed; tests were not
executed. No post-change Tracy capture was made, so no timing improvement is
claimed. Renderer code was not changed. This handover remains uncommitted.

## 2026-09-28 — sandbox readiness and fixed-step telemetry capture

An owned Tracy-enabled production session on API port `4101` started at
`12:23:49.224Z`; the first `/api/health` response arrived 1.85 s later. Window
creation was logged at `12:23:51.462Z`. Summer Space School `OpenTwin` was
issued at `12:23:51.893Z`; physics admission and scene-participant readiness
completed at `12:23:56.324Z` and `12:23:56.342Z`, respectively (4.43–4.45 s
after the request). Its stage spawned at `12:23:53.536Z`, about 1.64 s after
Twin-open. The API readiness gate stayed clear through a five-second soak
before Sandbox was opened; its exact first-clear transition was not captured.

Sandbox stage creation followed Twin-open by about 0.17 s. Physics admission
completed at `12:24:15.629Z`; the first sampled clear readiness state was about
12.86 s after Twin-open and remained clear for 5.06 s. This confirms fast scene
mount but misses the five-second full-readiness target. A post-settle
`PhysicsPerformance` query returned 120 samples at 3.971/6.312/7.910/8.766 ms
p50/p95/p99/max, with 50 bodies, 44 dynamic, 33 colliders, and 32 joints. The
capture ran with Tracy instrumentation and other simulator/Griffin workloads
active; these are diagnostic timings, not clean physics or FPS acceptance.

The capture `target/sss-twin-open-20260928.tracy` contains 31.62 s and about
5.50 million zones. `sync_twin_overlays` ran 81 times and peaked at 0.057 ms,
so the previous 261/278 ms exclusive-call lead did not recur. The clearest
fixed-schedule app-side leaf was `retain_physics_telemetry`: 312 calls, 0.302 ms
mean and 8.085 ms maximum; its deferred-command flush peaked at 2.656 ms. The
system runs after Avian's step but within `FixedPostUpdate`, so expensive
sampling work can still delay the next app update. Its per-sample path rebuilt
owned signal names and `SignalRef`s despite stable channel identity.

The telemetry owner now describes channel names with borrowed static paths or
component pairs and retains each `SignalRef` alongside its metadata in the
existing per-source cache. The cache retires with the source lifecycle; sample
names, metadata, rate, retention, mission-time stamps, and Avian substeps are
unchanged. `cargo +nightly-2026-02-27 check --locked --tests -j 4 -p
lunco-usd-sim-telemetry` passed as a compile-only check; no tests were executed.
This edit has not yet had a post-change Tracy comparison, so no timing gain is
claimed.

### 2026-09-28 — batch stable identity admission

The same capture measured a 78.8 ms deferred-command flush after
`assign_global_entity_ids`; its system body peaked at 0.581 ms across 1,061
calls. The owner now collects IDs in its existing query order and submits the
shared `GlobalEntityId` component through Bevy's `Commands::try_insert_batch`,
which reuses the archetype/bundle insertion path. The fallible batch retains
the despawn-tolerant boundary and reports invalid entities through Bevy's
warning handler. No identity allocation or lifecycle ordering changed.

`cargo +nightly-2026-02-27 check --locked --tests -j 4 -p lunco-core-session`
passed as a compile-only check; no tests were executed. This path has not yet
been reprofiled, so no timing gain is claimed. The same capture's remaining
startup lead is worker-side `modelica_solve_preparation_lower_for_live` (six
calls, 5.09 s mean, 6.82 s maximum).

### 2026-09-28 — batch procedural terrain-rock components

In the same capture, `scatter_terrain_layers` had a 5.746 ms system-body
maximum and a 67.033 ms deferred-command flush. The procedural rock loop queued
separate common-component, optional visual-component, and parent writes for each
rock. It now collects each into a fallible `Commands::try_insert_batch` call.
Bevy applies `ChildOf` relationship hooks in placement order and continues past
stale entities. Spawn allocation, collision properties, visual assets, scatter
order, and the quality cap are unchanged. This reduces deferred command fan-out
but does not yet move terrain preparation off the app thread.

`cargo +nightly-2026-02-27 check --locked --tests -j 4 -p
lunco-terrain-surface` passed as a compile-only check before parent updates were
batched; no tests were executed. After parent batching,
`cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-terrain-surface` passed.
No post-change Tracy comparison exists, so no timing gain is claimed. The
production run exited through typed `Exit`; port `4101` and the owned process
were released. An unrelated simulator remains active on port `4193` and uses
the existing executable; no replacement or post-change profile has been made.
The unprofiled 150 FPS, <0.5 ms physics, <2 s app startup, <2 s scene mount,
and <5 s full readiness targets remain open.

### 2026-09-28 — coalesce identical Modelica solve preparation

The existing Sandbox capture shows separate `lower_for_live` jobs for
`Skid_Raycast_1_System` (6.225 s) and `Skid_Raycast_2_System` (6.301 s). Both
reference the same skid-rover asset, and the existing structural solve key
normalizes generated instance names; the Sandbox scene authors no per-instance
Modelica overrides. The worker now shares an in-flight result for equal
prepared-solve keys while retaining one ordered commit ID per participant.
Result sharing covers both success and failure; each live stepper remains
entity-local. `cargo +nightly-2026-02-27 check --locked -j 4 -p
lunco-modelica-worker` passed. This change has not been profiled, and no timing
gain is claimed. The other three long Sandbox model lowerings remain distinct.

### 2026-09-28 — batch accelerometer sample initialization

The same Tracy capture shows `ensure_acceleration_samples` with a 46.135 ms
maximum deferred-command flush over 1,061 calls, while its system body peaked at
46.7 microseconds. The system only initializes `SolvedLinearAcceleration` for
newly physics-ready bodies. It now collects these identical initial states and
inserts them through one fallible typed batch in query order; steady fixed-step
sampling is unchanged. `cargo +nightly-2026-02-27 check --locked -j 4 -p
lunco-cosim` passed. There is no post-change Tracy capture, so no timing gain is
claimed.

### 2026-09-28 — batch readiness freeze inserts

The Tracy capture measured `reconcile_frozen_subtrees` at 0.305 ms maximum in
the system body and 8.564 ms in its deferred-command flush. The owner froze
newly admitted subtree entities with individual commands for joint, rigid-body,
collider, and ownership-marker components. It now collects those inserts in
held-root traversal order and queues fallible Bevy batches, with joint disables
before endpoint body disables. Release commands and the chained joint-release
boundary are unchanged. `cargo +nightly-2026-02-27 check --locked -j 4 -p
lunco-physics` passed. There is no post-change Tracy capture, so no timing gain
is claimed.

### 2026-09-28 — batch local-gravity projection inserts

The zone export `target/sss-twin-open-20260928.zones.csv` from
`target/sss-twin-open-20260928.tracy` shows
`sync_local_gravity_to_avian` at 79.328 microseconds maximum in the system body
and 7.968 ms maximum in its deferred-command flush across 312 calls. The owner
compares cached `LocalGravity` against Avian's existing
`ConstantLinearAcceleration`. Before this change it submitted one command per
changed body. It now collects changed components in query order and submits one
fallible batch; the comparison and `RemovedComponents` cleanup remain intact.
`cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-environment` passed.
There is no post-change Tracy capture, so no timing gain is claimed.

### 2026-09-28 — avoid repeated physics-pose seed writes

The Tracy zone export `target/sss-twin-open-20260928.zones.csv` records
`pose_to_position` at 1.055 ms maximum in its body and 4.527 ms maximum in its
deferred-command flush across 602 calls. `PhysicsPoseSeeded` is one-time
readiness metadata, but the bridge previously queued its insertion after every
valid pose refresh. It now inserts the marker only when absent; position and
rotation writes, bridge-shadow updates, and Avian sleeping-body wake removals
are unchanged. `cargo +nightly-2026-02-27 check --locked -j 4 -p
lunco-usd-avian-core` passed. There is no post-change Tracy capture, so no timing
gain is claimed.

### 2026-09-28 — avoid writes to empty force accumulators

The same Tracy zone export records `apply_pending_forces` at 0.807 ms maximum
in its body across 312 calls. The fixed-step system cleared all `PendingForces`
fields unconditionally, including when each was already zero, marking idle
components changed on every pass. A repository-wide caller search found no
`Changed<PendingForces>` consumer. The system now skips the clear for an
already-zero accumulator while still applying and clearing nonzero values,
clearing commands when physics is held, and faulting on non-finite values.
`cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-cosim` passed. There is
no post-change Tracy capture, so no timing gain is claimed.

### 2026-09-28 — bound procedural-rock admission batches

The current Tracy-enabled production binary was rebuilt at the standard
`optimization/target/debug/luncosim` path. An owned Apollo run used High
quality, X11, `--no-vsync --no-throttle`, API port `4111`, and a 45.36 s Tracy
capture (`target/optimization-apollo-current-20260928.tracy`, 1,997 frames,
8,556,927 zones). A separate `bevy_pbr` Rust compilation was active in the
`sysml-integration` checkout during this run, so its timings are diagnostic and
not clean acceptance results.

`sync_twin_overlays` ran nine times and peaked at 0.028 ms. The current
`scatter_terrain_layers` system body peaked at 0.547 ms, while its deferred
command flush peaked at 55.702 ms at trace time 4.463 s. Procedural rock
placements now enter deterministic queues and cross into ECS in bounded batches
of at most eight physics bodies and 64 visual component insertions per `Update`.
Placement order is preserved for both recycled and newly spawned rocks. The
terrain's pending marker remains until both queues finish. Body batches commit
while physics continues; this owner adds no physics hold.

The post-bundle Tracy capture (`target/optimization-apollo-post-bundle-20260928.tracy`,
45.3 s, 2,764 frames) measured a 47.117 ms maximum command flush. The follow-up
spawn-batch capture (`target/optimization-apollo-spawn-batch-20260928.tracy`,
45.36 s, 3,130 frames) measured the `scatter_terrain_layers` body at
0.0026/0.0066/0.0095/0.3027 ms p50/p95/p99/max, and its command flush at
0.0002/0.0005/0.0008/24.771 ms p50/p95/p99/max. The flush maximum occurred at
5.187 s. These are separate Tracy captures, not a controlled A/B or clean
acceptance run, so they do not establish a causal timing gain. The latter run
had no concurrent Cargo build or other simulator at launch; profiler and
desktop overhead still make its timings diagnostic.

The full `PhysicsSchedule` zones measured 0.458/0.833/1.077/13.556 ms
p50/p95/p99/max; they are diagnostic schedule durations, not the
`PhysicsTotalDiagnostics.step_time` acceptance measure. The worker-side
`Traverse/Rover/Electrical_System` solve lowering took 7.812 s with a 22
microsecond queue wait; it does not block the app thread.

In the follow-up run, `/api/health` first responded 1.798 s after launch and the
window-created event was at 1.947 s. `StartupScene opened` was at 2.157 s; the
doc-backed scene mount began at 2.636 s, but a completed mount transition was
not captured. `/api/ready` first showed clear and later regressed as readiness
producers registered more work. Its first clear sample that remained clear
through the five-second soak was 11.206 s after process launch (about 9.05 s
after `StartupScene opened`), so the 5 s full-readiness target was missed in
this diagnostic run. The run did not capture a clean
`PhysicsTotalDiagnostics.step_time` acceptance series. Sandbox and clean FPS
and physics acceptance remain unmeasured. The owned session exited through
typed `Exit`, releasing ports `4111` and `8086`.

### 2026-09-28 — move recents restore out of the startup barrier

The same capture records `load_recents_at_startup` at 27.047 ms and the
`Startup` schedule at 36.122 ms. The system synchronously read the recents
file, decoded JSON, canonicalized up to 30 paths, optionally rewrote a
deduplicated file, and serialized its change snapshot before `Startup`
completed. The trace attributes the system to a Startup executor thread; it
does not identify that thread as the dedicated window thread, but the schedule
still awaited its completion.

The recents load, path canonicalization, cleanup write, and JSON snapshot now
run in one `AsyncComputeTaskPool` job. `Update` commits the typed result and
keeps Twin/file entries added while the job ran ahead of older disk entries.
Recents persistence waits for this commit so auto-open cannot overwrite the
previous list. The Tracy production rebuild and run used the standard
`optimization/target/debug/luncosim` path, API port `4111`, and the same Apollo
scene/settings. Its 20.28 s capture (`target/optimization-apollo-recents-async-20260928.tracy`,
957 frames, 4,689,790 zones) measured `load_recents_at_startup` at 7.084
microseconds and `Startup` at 13.284 ms. These are diagnostic observations from
a separate capture, not a controlled A/B. Two other simulator processes were
active in sibling worktrees during this run; no Cargo build was active.

`/api/health` first responded 1.518 s after launch and the window-created log
was at 1.613 s. `StartupScene opened` was at 2.000 s; composed mounting began
at 2.467 s, but completion was not captured. Readiness showed clear early,
regressed as later producers registered waits, and first stayed clear at
13.112 s after launch (about 11.112 s after Twin open), missing the 5 s target.
The five-second soak remained clear through the 20.28 s capture. `PreStartup`
measured 190.933 ms, including 184.622 ms in
`application_policy_activation`; worker preparation took 19.416 ms and the
PreStartup wait span was 4.118 microseconds. Policy activation is the next
measured app-startup hotspot to inspect. No clean FPS or
`PhysicsTotalDiagnostics.step_time` acceptance series was captured.

The continuation also moves later recents saves off the app schedule. It compares
typed `Recents`, allows one worker write at a time, then schedules the latest
list after completion. Shutdown drains pending load/write work and saves the
final list. The Tracy-enabled production build passed on the standard
`optimization/target/debug/luncosim` path. Its follow-up Apollo High run used
X11, `--no-vsync --no-throttle --log-diag`, API port `4111`, and a 30.29 s
capture (`target/optimization-apollo-recents-saves-async-20260928.tracy`, 1,970
frames, 9,982,003 zones). Across 1,969 `persist_recents_when_changed` calls,
the system measured 0.001132/0.003286/0.005631/0.064911 ms
p50/p95/p99/max. `load_recents_at_startup` measured 10.570 microseconds in this
separate run. These diagnostic values do not form a controlled A/B; another
simulator was active in the `sysml-integration` checkout. `/api/ready` was clear
with no pending work at the post-capture sample, but its exact transition was
not recorded. The owned session exited through typed `Exit`, releasing ports
`4111` and `8086`. Clean FPS and whole-step physics acceptance remain unmeasured.

### 2026-09-28 — avoid repeated telemetry owner-marker writes

The same 30.29 s Apollo Tracy capture recorded 835 calls to
`retain_physics_telemetry`: the system body measured
0.032971/1.579184/2.455348/5.526404 ms and its deferred-command flush measured
0.000200/0.015980/0.025668/1.746466 ms p50/p95/p99/max. These are diagnostic
values from the run with another simulator active. Source review found that the
physics and Modelica producers queued `SignalSource` on every retained batch,
although it is lifecycle metadata whose removal observer archives that entity's
history. Both producers now queue the marker only when it is absent. No sample,
metadata, or history semantics changed, and no renderer code changed. The
Tracy-enabled production build passed on the standard optimization target. The
30.31 s post-change Apollo High capture (`target/optimization-apollo-signal-source-marker-20260928.tracy`,
1,330 frames, 6,033,475 zones) measured the body at
0.032901/1.167176/2.228365/4.448603 ms and its deferred-command flush at
0.000261/0.000721/0.001463/0.728177 ms p50/p95/p99/max. This first post-change
run overlapped a `lunar-soil` scene test. After that test ended, a second 30.35 s
capture (`target/optimization-apollo-signal-source-marker-final-20260928.tracy`,
1,573 frames, 7,021,977 zones) ran with the same concurrent
`sysml-integration` simulator as the earlier baseline. It measured the system
body at 0.031468/0.895719/1.635008/4.362973 ms and the command flush at
0.000241/0.000642/0.001092/1.176143 ms p50/p95/p99/max. The baseline flush was
0.000200/0.015980/0.025668/1.746466 ms. These separate instrumented runs remain
contention-affected and are not a controlled A/B; no causal timing reduction is
claimed. `/api/ready` was clear after the final capture, but its transition was
not captured. The owned session exited through typed `Exit`, releasing ports
`4111` and `8086`.

### 2026-09-28 — reuse the physics telemetry cache entry

The final marker-write capture above is also the baseline for this pass:
`retain_physics_telemetry` measured 0.031468/0.895719/1.635008/4.362973 ms
p50/p95/p99/max, with an active sibling simulator. Source review found that
steady samples used `HashMap::entry` to confirm an existing `CachedPhysicsSignal`
and then `get_mut` to retrieve that same value. The owner now keeps the occupied
entry and reuses it for recording, while vacant entries still enforce the same
channel limit before insertion. The Tracy-enabled production build passed on
the standard optimization target. The 30.28 s Apollo High capture
(`target/optimization-apollo-physics-cache-entry-20260928.tracy`, 2,125 frames,
8,978,166 zones) measured the system body at
0.026119/0.900509/1.497142/6.604794 ms and its command flush at
0.000200/0.000511/0.001002/2.200764 ms p50/p95/p99/max. The previous capture
measured 0.031468/0.895719/1.635008/4.362973 ms and
0.000241/0.000642/0.001092/1.176143 ms respectively. The same unrelated
simulator remained active, but these are separate instrumented runs, not a
controlled A/B; no causal timing reduction is claimed. `/api/ready` was clear
after the final capture, but its transition was not captured. The owned session
exited through typed `Exit`, releasing ports `4111` and `8086`.

### 2026-09-28 — cache physics telemetry owner associations

Source review found that every retained physics sample asked the shared signal
registry to re-associate the same global owner. `CachedPhysicsSignal` now keeps
the last owner ID and updates the registry only when that ID changes. The
Tracy-enabled production build passed on the standard optimization target. The
30.39 s Apollo High capture
(`target/optimization-apollo-physics-owner-cache-20260928.tracy`, 475 frames,
2,400,800 zones) measured `retain_physics_telemetry` at
0.016792/0.214460/2.890549/217.930030 ms p50/p95/p99/max. Its previous
instrumented capture measured 0.026119/0.900509/1.497142/6.604794 ms. These
captures are not a controlled A/B: concurrent simulator processes were active,
and this capture's large outliers make it unsuitable for attributing a timing
change. No causal reduction or acceptance result is claimed. The first
`/api/ready` sample was taken after startup and stayed clear during the
following 34 s poll; the exact process-start transition was not captured. The
owned session exited through typed `Exit`, releasing ports `4111` and `8086`.

### 2026-09-28 — bound rock batches without pausing physics

The final eight-body Tracy capture was 62 MB and is no longer retained under
`target/` after disk-space recovery. It contained 27 new-rock batches of eight
(max 1.936 ms, p95 1.733 ms, median 0.849 ms), seven recycled-body batches
(max 1.948 ms, p95 1.006 ms, median 0.508 ms), and 33 visual batches (max
0.338 ms, p95 0.184 ms, median 0.045 ms). The earlier 256-body batch reached
70.748 ms in `terrain_rock_spawn_batch` and a 259 ms `system_commands` flush;
those outliers were absent from the eight-body capture. Both captures were
profiler diagnostics. The eight-body trace predates removal of the
rock-specific physics hold, so it establishes batch cost only; a fresh terrain
run after that change remains open.
After removing that hold, `cargo +nightly-2026-02-27 check --locked -j 4 -p
lunco-luncosim --bin luncosim` passed. No terrain fixture was rerun after this
final behavior change.

### 2026-09-28 — isolate first Twin view replay from persistent edit volume

The first projection previously requested the global operation suffix from
generation zero when a document had a non-empty view layer. Its mounted stage
recipe already contained current base/runtime data, but an unrelated runtime
restore or persistent edits could make that global suffix appear incomplete and
force a whole composed-stage rebuild. The document now retains a separate
bounded view-operation journal from the current source baseline; first projection
replays that suffix, and a genuinely expired view suffix still selects the full
rebuild path. Later document updates continue using the existing global
generation cursor.

`cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-usd-document -p
lunco-usd-bevy-runtime-core` passed. No tests or runtime capture were run after
this change in the original commit. A 2026-09-29 production Tracy capture on
the First Drive projection path recorded five `sync_twin_overlays` calls after
Twin open: 0.031, 0.290, 0.042, 0.087, and 1.481 ms. The earlier 261–278 ms
calls did not recur in that capture. It was profiler-affected and did not record
the full Twin-ready transition, so it is diagnostic evidence rather than an
acceptance result.

### 2026-09-29 — compare View and Build with Telemetry open

The Tracy-enabled production binary at `c953f9bde` opened the authored First
Drive scene on owned API port `4111`. The Build perspective supplied the
Telemetry and Graphs panels; the app was maximized and `/api/ready` reported
`ready=true`, `world_hold=false`, and no pending items. The session was not a
clean performance run because Tracy was connected.

In a same-session View-to-Build comparison after maximizing and allowing each
layout to settle, `ReadExposures` reported View at 104.9 FPS / 9.53 ms per frame
and Build at 84.6 FPS / 11.81 ms per frame. The sampled difference was about
2.3 ms per frame, not a sustained 30 ms. The exposed physics-step samples were
1.17 ms in View and 1.54 ms in Build; these profiler-affected samples do not
establish the physics acceptance target.

During the 45.3 s Tracy capture, `render_workbench` measured 0.49 ms p50,
1.20 ms p95, 3.31 ms p99, and 17.00 ms max. `EguiPrimaryContextPass` measured
0.83 ms p50, 2.48 ms p95, 4.19 ms p99, and 17.47 ms max. The largest events
were isolated near the capture end; they were not attributed to a specific
panel. A separate maximized Build capture recorded one 29.08 ms
`render_workbench` event at 2.15 s, while Build activation and window resize
were both occurring. Its p99 was 2.11 ms, so that run also did not show
sustained 30 ms work. The transition and resize were not separated, and neither
capture reproduced the screenshot's 36.4 ms p99 under clean conditions.

The evidence points to roughly 1–2 ms of steady Build-mode UI cost in these
runs, with occasional diagnostic outliers that still need a clean reproduction
and per-panel attribution before changing another hot path. The earlier async
catalog preparation remains in place. No renderer code was changed and no
additional implementation or tests were run for this measurement.

### 2026-09-29 — cache Telemetry filter and branch counts

Source review of the Build Telemetry panel found that repainting recalculated
descendant visibility recursively for each open branch, and case-insensitive
filtering lowercased the query and every row field during each check. The
catalog worker now prepares normalized row/group text, and the panel caches
bottom-up public, complete, and focused counts until catalog, focus, filter,
scope, or archived-row state changes. Painting reuses those counts and checks
prepared strings; current sample values are still read from the signal
registry. The root preflight uses an iterator instead of allocating a root list.

`cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-viz --features ui`
passed. A later Tracy run on this code is recorded below; it is diagnostic and
does not establish a causal improvement or acceptance target. No renderer code
changed.

### 2026-09-29 — reduce Builder row work and attribute panel costs

The Entity List paint pass now borrows its cached labels, reads Shift once per
panel pass, and constructs row tooltip text only for the hovered row. The line
plot toolbar now keeps signal references and metadata in an egui cache keyed by
registry identity and catalog revision; it enumerates the registry only when a
picker opens. Owner labels remain live while the menu is open. Named
`workbench_panel_render` Tracy zones split visible panel costs out of the
aggregate `render_workbench` pass. The 3D renderer was not changed.

The Tracy-enabled production build passed with:
`cargo +nightly-2026-02-27 build --locked -j 4 -p lunco-luncosim --bin luncosim --features tracy`.
The settled scoped capture is
`target/optimization-builder-entitylist-plotted-20260929.tracy`. It used First
Drive High on API port `4111` and Tracy data port `8087`; Build, Telemetry, and
Graphs were active, and the screenshot confirmed the `Shaft_RR.phi` curve. It
ran for 40.28 s (3,921 frames, 17,355,484 zones). After the first 10 s,
`render_workbench` measured 0.418/1.798/2.389/11.041 ms and
`EguiPrimaryContextPass` 0.709/2.139/2.749/11.266 ms p50/p95/p99/max. Panel
p50/p95/p99/max was 0.376/0.618/0.907/2.006 ms for Telemetry,
0.158/0.266/0.407/2.391 ms for Graphs, and 0.220/0.362/0.472/2.327 ms for
Entity List. One earlier active-graph panel-zone capture measured Entity List
at 0.430/0.817/1.219/6.900 ms. That earlier capture overlapped a build in
another checkout; the later capture overlapped a separate terrain simulator
using about 78% CPU in a post-capture process snapshot. The two captures are
not a controlled A/B, so the lower Entity List timings do not establish a
causal reduction. They do show that typical panel paint is below 1 ms p95 per
panel in the later diagnostic, while occasional workbench/UI outliers remain.
More frame attribution is needed before assigning the full slowdown to the
Builder UI.

A separate non-Tracy First Drive run reproduced the Builder layout and active
`network_system.Shaft_RR.phi` graph. Across 25 one-second
`ReadExposures(engine-health)` snapshots, sampled `frame_time_ms` was
18.468/26.028/28.022/28.022 ms and `physics_step_ms` was
0.428/1.024/1.051/1.051 ms p50/p95/p99/max. The process was ready with no
pending work. Other simulator sessions were active, so these are
contention-affected samples, not acceptance measurements. They were taken
before the Entity List paint change; no clean post-change FPS run or full
`PhysicsTotalDiagnostics.step_time` series has been recorded. The reported
near-2x Builder slowdown is therefore still open; neither this change nor the
panel traces prove it fixed.

### 2026-09-29 — settled Builder caches and paired frame sample

The current UI pass removes repeated Builder work in the Entity List, telemetry
browser, and graph panels. Entity List rows borrow cached labels and build row
tooltips only while hovered. Telemetry rows retain label, unit, and formatted
value text, copy only the four theme tokens used by the panel, and reuse
decimated preview points. Graph series borrow retained `PlotPoint` buffers, and
the toolbar builds its scalar signal catalog only while a picker is open. The
`workbench_panel_render` child zones remain for attributing each panel. No 3D
renderer code changed.

A separate 80.4 s Tracy capture used the First Drive scene, High quality, and
the Builder layout with Telemetry selected and `network_system.Shaft_RR.phi`
plotted. The screenshot confirmed the requested panel arrangement. The capture
recorded 3,803 frames and 19,319,163 zones; scene readiness was clear and
Builder panel events began at 10.83 s. After the first 20 s, Entity List
measured 0.388/0.684/0.872/2.458 ms, Telemetry 0.709/1.253/1.752/3.338 ms,
and Graphs 0.207/0.409/0.579/1.560 ms p50/p95/p99/max. These are diagnostic
profiler timings, not acceptance results. They show that the visible panel
bodies do not account for the full frame-time increase.

A no-Tracy paired run on the same First Drive scene confirmed the reported
Builder slowdown in this session. Each perspective settled for 15 s before 12
one-second `ReadExposures(engine-health)` samples. View recorded median frame
time 10.741 ms (7.624–12.853 ms) and median 93.21 FPS; Builder recorded
20.399 ms (12.029–48.094 ms) and median 49.24 FPS, a 1.89x frame-time ratio.
Median physics-step time was 0.499 ms in View and 0.555 ms in Builder. The
session reported `/api/ready` clear. Other simulator sessions remained active,
so this is a contention-affected diagnostic, not clean acceptance; it does
reproduce the user's 1.5–2x observation. The remaining frame cost needs
attribution outside the visible panel bodies. Do not change the 3D renderer.

The latest no-Tracy production build passed with the row-cache cleanup. No
tests were run. The UI changes are committed locally as `cab1f44f8`; the newer
local `main` changes were merged into `optimization`, and local `main` was
fast-forwarded to `eab2fc72f`. Nothing was pushed. This handover remains
intentionally uncommitted.

### 2026-09-29 — virtualize expanded Builder rows

Entity List now retains a panel-local row buffer and paints only rows in its
scroll viewport. Telemetry builds a lightweight borrowed row index from the
expanded portion of its cached catalog and also uses `ScrollArea::show_rows`;
offscreen channel widgets, text layouts, and handlers are not constructed.
Both trees keep branch state on stable entity/group identities, and telemetry
still reads current samples from `SignalRegistry`. The graph data and rendering
settings are unchanged. No renderer code changed.

The production build passed with:
`cargo +nightly-2026-02-27 build --locked -j 4 -p lunco-luncosim --bin luncosim`.
On owned API port `4112`, First Drive High reached clear `/api/ready`; the
Builder screenshot at `target/telemetry-virtualized.png` showed Entities,
Telemetry, Spawn, Inspector, and the plotted `network_system.Shaft_RR.phi`
curve. The app remained live through the capture with no UI panic.

The no-Tracy 12-sample Build run recorded `frame_time_ms` p50/p95/p99/max of
9.689/14.520/14.808/14.880 ms, FPS p50/p95/p99/max of
103.363/149.117/154.287/155.580, and `physics_step_ms` of
0.430/1.021/1.100/1.120 ms. These are contention-affected diagnostics: a
`lunco-usd-avian` Cargo test/build in the sibling `lunar-soil` checkout and two
other simulator sessions were active during sampling. A View sample was not
captured in the same process after these changes, so this is not a controlled
Builder/View comparison or acceptance result. No tests were run for this UI
change. The handover remains open until a clean paired run reaches the stated
FPS and physics targets.

### 2026-09-29 — move live plot preparation off the UI frame

Line plots now snapshot changing histories at most 20 times per second and
build paired/log-transformed/decimated point buffers on Bevy's async-compute
pool. Each plot binding has at most one active build and keeps drawing its last
completed buffer until a newer one is ready. This bounds chart lag to about
50 ms and removes full-history decimation from the UI frame. Renderer code was
not changed.

`cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-luncosim --bin
luncosim` and the corresponding production `cargo build` passed. The updated
First Drive High app reached clear `/api/ready` on owned port `4112`, and the
Builder screenshot `target/async-plot-builder.png` showed the telemetry tree
and `network_system.Shaft_RR.phi` graph populated with no UI panic. Its overlay
sample was 51.1 FPS / 19.6 ms per frame, p99 46.8 ms, while other simulator
sessions were consuming several CPU cores. This verifies rendering only; it is
not an acceptance measurement or a controlled before/after comparison. The
owned app exited through typed `Exit`, and port `4112` closed.

The report remains uncommitted. The handover is still open until an
uncontended Builder/View run demonstrates the performance target.

### 2026-09-29 — skip hidden Builder view-model producers

The Command Deck and Joint State view-model producers now run only when their
panels are visible in `WorkbenchSnapshot`, ordered after
`WorkbenchSnapshotPublishSet`. Command Deck previously rebuilt a display label
and checked possession every app frame; Joint State scanned the scene's joint
and wheel queries every frame for a selected vessel even when its panel was a
hidden dock tab. The existing workbench snapshot is the authoritative
visibility owner. This removes hidden-panel work without changing the visible
readout cadence. `cargo +nightly-2026-02-27 check --locked -j 4 -p
lunco-luncosim-edit-ui` passed. No post-change Builder/View timing was recorded,
so no FPS gain is claimed yet.

### 2026-09-29 — avoid copying closed Spawn palette entries

The Builder Spawn palette now obtains distinct category labels without cloning
one string per catalog entry, then iterates borrowed entries only for expanded
categories. Previously it cloned every spawn entry into temporary category
vectors each frame, including categories whose egui paint closures were not
run. `cargo +nightly-2026-02-27 check --locked -j 4 -p
lunco-luncosim-edit-ui` passed. This is a source-level hot-path reduction; no
post-change FPS comparison was available, so the Builder/View gap remains open.
Local commit `43468a6e5` is integrated on `main` by fast-forward; nothing was
pushed. Profiling sessions in other worktrees remain untouched.

### 2026-09-29 — defer Inspector material labels

The Inspector's material selector still discovers material-bearing entities
under the selected root, but it no longer formats a display label for every
part each frame. It resolves only the active part label during normal painting
and formats the remaining labels when the dropdown opens. This addresses
avoidable UI allocation while retaining the current part list and selection
behavior. Its PBR part subset now reuses those discovered entity identities
instead of making a second child-tree traversal. `cargo +nightly-2026-02-27
check --locked -j 4 -p lunco-luncosim-edit-inspector-ui` passed. The Rover
hierarchy contains 343 entities in the current Builder view; the selected-root
material discovery now stays cached by selected root and `UsdStageRevision`, so
steady repaint no longer traverses that subtree. Non-USD roots or a missing
revision resource force recomputation. No post-change FPS comparison was
recorded, so the reported Builder/View gap stays open. This handover remains
uncommitted.

### 2026-09-29 — avoid steady workbench snapshot allocations

`sync_workbench_snapshot` runs in `Update`, where it previously built and
compared five owned vectors even when the dock and perspective were unchanged.
The snapshot owner now compares borrowed ordered tab, visible-tab, and
perspective iterators first, materializing the replacement only after a
canonical input changes. This removes steady-frame allocations from that
publisher by construction; no FPS delta is claimed. The handover remains
uncommitted.

### 2026-09-29 — borrow Builder dock panels in place

`PanelTabViewer` no longer removes each visible singleton or instance panel
from the layout registry and reinserts it after paint. It borrows the existing
entry directly, then applies the panel's deferred intents after releasing its
`PanelCtx`. This removes per-visible-tab registry churn while preserving panel
state and intent ordering. `cargo +nightly-2026-02-27 check --locked -j 4 -p
lunco-workbench` passed. No post-change Tracy capture or FPS sample is
available, so the performance delta is unmeasured and the Builder/View gap
remains open. Local commit `08758abf1` is integrated into `main` by
fast-forward; nothing was pushed. This handover remains uncommitted.

### 2026-09-29 — share live plot config snapshots

`VizPanel` previously deep-cloned its complete `VisualizationConfig` on every
paint, including signal bindings and style data. `VisualizationRegistry` now
stores shared immutable config snapshots; `get_shared` clones only the `Arc`,
and mutable access uses copy-on-write. Existing borrowed getters, iteration,
and mutable-edit semantics remain intact. `cargo +nightly-2026-02-27 check
--locked -j 4 -p lunco-viz --features ui` passed. No test or post-change Tracy/FPS
run was performed, so the frame-time effect is unmeasured. Local commit
`d43b41142` is integrated into `main` by fast-forward; nothing was pushed. The
handover remains uncommitted.

### 2026-09-29 — move Graphs history copies off the UI frame

`ScalarHistory` now stores completed 256-sample chunks as immutable shared
blocks and retains a bounded mutable tail. Plot snapshots share the completed
blocks and copy only that tail; workers flatten the captured samples. Both the
line-plot panel and the live overlay in Modelica's Graphs panel build updated
point buffers asynchronously at a maximum of 20 Hz and keep painting the last
completed buffer while a build runs. The Graphs panel also uses the shared
`VisualizationConfig` snapshot instead of cloning the full config on each
paint. History readers now use `front`, `back`, and `iter` rather than reaching
into the storage representation.

`cargo check --locked -j 4 -p lunco-signal -p lunco-viz -p
lunco-luncosim-exposures -p lunco-modelica-telemetry -p lunco-modelica-ui -p
lunco-usd-sim-cosim-api -p lunco-usd-sim-telemetry --features lunco-viz/ui`
passed. `git diff --check` passed. No runtime/FPS comparison was captured and
no tests were run, so the Builder/View gap remains open. The production build
was not attempted because the checkout reached 2.0 GB of build output with
about 1.6 GB free. This handover remains uncommitted.

### 2026-09-29 — measure the Builder shell separately

The Tracy production build exposed two existing UI compile errors required to
launch this checkout: the Ports value formatter now accepts the contract's
`Option<f64>` and preserves unavailable values, and SysML requirement sorting
places `Unverified` with `Inconclusive`. The Tracy build then passed. On owned
API port `49189`, `assets/tutorials/sandbox/first_drive.usda` reached
`/api/ready`; `rover_build` displayed Entities, Telemetry, Spawn, Inspector,
and Graphs together. Other simulator sessions were active, and this was an
instrumented run, so its numbers are diagnostic only.

The 70.4-second capture contained 3,875 frames (about 55 FPS during capture).
The sampled Builder overlay showed 27.5 FPS / 36.4 ms. Steady exclusive UI
zones averaged 0.523 ms for `render_workbench`, 0.408 ms for Telemetry, 0.396
ms for Entities, 0.198 ms for Graphs, and 0.086 ms for Spawn; their observed
maxima were 3.368, 2.496, 1.239, 1.328, and 0.314 ms respectively. The custom
menu row averaged 0.017 ms and peaked at 0.163 ms, so its avoided repaint work
is not by itself a material explanation for the reported frame time.

Across the same trace, Bevy `Render`, `Update`, and `PostUpdate` schedule maxima
were 185.509, 147.528, and 70.264 ms; `EguiPrimaryContextPass` averaged 0.304
ms and peaked at 5.362 ms. The Builder panel zones do not explain the sampled
36.4 ms frame. Attribute the schedule excursions and repeat a non-Tracy,
uncontended Builder/View comparison before claiming a gain. The task-local
`target/` was cleaned after capture to build the normal binary; the raw trace
was not retained. This handover remains uncommitted.

### 2026-09-29 — bound live Graphs work to display resolution

The user reports Builder at roughly 10 ms/frame without a populated graph and
20–30 ms/frame with graphs. Source inspection found that the live line-plot
worker only decimated histories above four times chart width; ordinary 2,000
sample histories therefore still sent every sample through line geometry each
repaint. Live and multi-series plots now build min-max buffers on the async
worker at about one sample per logical display point, preserving endpoints and
per-bin extrema. They retain full-data transformed bounds alongside those
buffers, so Fit/autobounds remains based on the complete history. Multi-series
conversion is keyed by source identity, log-Y mode, and display width; the
experiment variable groups and positivity summaries are retained by the
change-gated view model. A same-series replacement keeps drawing the last
completed buffer until its new buffer is ready. No renderer or tree layout
code changed.

`cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-viz -p
lunco-experiments-ui -p lunco-modelica-ui --features lunco-viz/ui` and the
Tracy-featured production build passed. The build emitted two existing
unreachable-pattern warnings in `lunco-controller`; the changed plot crates
were warning-free. No tests were run.

On owned API port `49192`, `assets/tutorials/sandbox/first_drive.usda` reached
`/api/ready`; the `rover_build` perspective displayed the default Graphs plot
with eight Modelica signals added through `AddSignalToPlot`. The separate
60.4-second Tracy capture contained 5,783 frames (about 96 instrumented FPS).
The active plot panel's exclusive body was p50/p95/p99/max 0.188/0.315/0.421/
2.606 ms; an empty default Graphs plot in a separate capture was
0.096/0.165/0.223/0.514 ms. The plot body accounts for about 0.1 ms average
additional UI work in this eight-signal reproduction, not the reported
10–20 ms difference. `run_egui_context_pass_loop_system` p99 was 0.172 ms
(max 0.823 ms). Render, Update, and PostUpdate schedule p99s were 4.306,
1.748, and 4.579 ms, with profiler-affected isolated maxima of 310.868,
47.775, and 133.112 ms. The one-second `--log-diag` samples during the settled
graph window had a 10.86 ms median latest frame-time sample; one sample reached
81.6 ms. These instrumented results do not reproduce a sustained 20–30
ms/frame graph penalty or constitute clean FPS acceptance.

This reproduction used eight Modelica variables through the API's
`AddSignalToPlot` command, which resolves names against the Modelica entity.
The screenshot's `Shaft_RR.phi` is an internal Modelica variable declared by
`AvianShaft.mo`; it was not the same set of plotted signals. Telemetry
drag-and-drop binds the selected channel's full `SignalRef`; this capture did
not bind that exact source/path. The exact user-reported graph case and a clean
no-Tracy Builder/View A/B therefore remain open. The trace was inspected
at `/tmp/builder-graphs-active-20260929.tracy` and removed after analysis. The
owned app exited through typed API `Exit`; port `49192` is closed. This handover
remains uncommitted.

### 2026-09-29 — avoid building the closed Graph signal-picker set

The live Graph toolbar previously allocated a `HashSet` and cloned each bound
`SignalRef` on every paint, although that set was used only when the Add picker
was open. Membership is now checked against the existing bindings inside the
picker callback, so closed-picker frames do no catalog membership work. The
focused `cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-viz --features
ui` passed. This is a source-level hot-path reduction; no graph-frame timing or
FPS improvement is claimed. No tests were run.

### 2026-09-29 — prune time-series hover search

Source inspection of the locked `egui_plot` 0.36.0 dependency found that a
hovered `Line` searches every line segment on each paint. The cached plot-item
adapter now uses the time-series X ordering to stop searching once the
horizontal distance to further segments exceeds the closest candidate found.
It keeps the dependency's general search for phase-space plots, whose X values
can reverse. The focused `cargo +nightly-2026-02-27 check --locked -j 4 -p
lunco-viz -p lunco-modelica-ui --features lunco-viz/ui` and the non-Tracy
production build passed. An owned First Drive app reached clear `/api/ready`
and accepted `ActivatePerspective(rover_build)`, but the exact
`AddSignalToPlot(network_system.Shaft_RR.phi)` request timed out while other
simulator workloads were active and the owned process used about one CPU core.
That run did not exercise graph hover or provide FPS evidence; the app exited
through typed API `Exit`, and port `49221` closed. No tests were run, so the
frame-time effect remains unmeasured. The 3D renderer is unchanged.
The plot hover change is local commit `bd3202bee`, rebased on the newer
`main` and fast-forwarded into the local `main` worktree. I did not run a push;
read-only `ls-remote` now reports `origin/main` at the same commit, and the
local reflog records an `update by push` at 14:40:37 during this integration.
The handover report remains uncommitted.

### 2026-09-30 — incremental telemetry presentation index

The browser consumes coalesced descriptor notifications from `SignalRegistryPlugin` and
maintains its tree, alias groups, and reverse owner-ancestor index across updates. It enumerates
scalar identities once per scene, prepares at most 64 changed descriptors per async batch,
and commits patches without replacing the tree. Selection and steady samples do not prepare
descriptors. Individually superseded descriptors are requeued while unrelated results commit.
Scene teardown cancels workers and clears the index with a newer presentation key.

The production query `InspectTelemetryCatalog` reports queue state, descriptor counters,
and capture/worker/commit milliseconds. The parameterized API runner and authored Rhai
verdict cover channel admission, repeated unit edits, selection, continued live samples,
and absent-channel/invalid-parameter negative cases. The final non-Tracy High-quality
Summer Space School run on owned API port `4187` passed 24 Rhai verdicts (312 checks),
including 20 alternating unit edits. Each edit prepared exactly one descriptor; selection
and steady samples prepared none, and the initial-scan counter stayed at one. The catalog
contained 1,307 channels before the probe and 1,308 after it; the total prepared count grew
from 1,307 to 1,328, accounting for only admission and the 20 edits. Hierarchy notifications
compare consumed facts against both committed and in-flight facts, so rewriting unchanged
label/path/parent data queues no work. A generic resource-seam test also verifies that a real
owner rename updates its dependent channels.

Median capture/worker/commit costs were 0.0420/0.0226/0.0762 ms, with maxima
0.0897/0.0441/0.1521 ms across those edits. Observed command-to-visible-metadata latency
was 41.3 ms median and 61.3 ms maximum, including API transport and frame scheduling.
Initial admission's maximum commit was 2.00 ms with batches bounded to 64 descriptors.
The test used simulation transport rate 0.1, unchanged High rendering quality, and no
vsync/throttle. These are owner timings and update-latency evidence, not whole-app FPS
acceptance. Other owned-by-others sessions in the optimization checkout overlapped the
validation period; none were controlled. Admission and settled screenshots show the tree
available. The runner verified API shutdown. Raw evidence is in
`target/telemetry-review-evidence.json` and `target/telemetry-review-runtime-verdicts.log`.

A separate current-source Tracy build/capture used the adjacent `../tracy` tools and the
same owned API port. Its authored run passed 14 verdicts (182 checks) with ten unit edits.
The bounded 40 s capture includes startup and nine one-row commits: those commits measured
0.146 ms mean and 0.264 ms maximum, with worker preparation at 0.045/0.066 ms mean/max.
After initial admission, 123 producer/poller invocations without an overlapping patch measured
0.135/0.002 ms median and 0.418/0.024 ms maximum respectively. Concurrent optimization
sessions were active, so these are contention-affected Tracy diagnostics. The trace is
`target/telemetry-catalog-current.tracy`, with zone/event CSVs and an idle-window summary
beside it. The normal non-Tracy production binary was restored afterward.

The settled browser panel in that capture rendered in 0.708 ms median, 1.753 ms p95,
and 5.078 ms maximum across 132 calls, including descriptor updates. These costs include
view filtering and painting; patch-commit timings alone do not measure total panel cost.
The export and summary are `target/telemetry-review-panel-events.csv` and
`target/telemetry-review-panel-summary.json`.

Review additionally verifies exact selected entity/USD-path cache dependencies, release of
unused ancestor facts, and per-row supersession without discarding unrelated patches.
Final validation reused the current UI test binary: all 46 tests passed, including alias
promotion/removal, unchanged owner writes, actual renames, selection/sample stability,
bounded batches, and teardown. The signal owner's coalescing/fan-out/no-op-removal test also
passed. Skill catalogue validation and the final diff check passed.

## Acceptance

Close only after clean production High-quality runs of Summer Space School and
sandbox sustain at least 150 FPS without visual-quality loss, full Avian
`PhysicsTotalDiagnostics.step_time` is below 0.5 ms per fixed step with
deterministic cadence preserved, app window/API readiness is within 2 s from
process launch, each Twin mounts its scene within 2 s from Twin-open, and
reaches full readiness within 2 s from Twin-open. Record frame and physics-step
percentiles, all startup milestones, exact scenes/settings, and verify typed
API shutdown. Keep this handover open until those results are demonstrated.

See the [`performance-profiling` skill](../../skills/performance-profiling/SKILL.md)
for current capture commands, and the BigSpace architecture reviews for
dependency ownership and propagation constraints.

### 2026-09-29 — separate graph-history copy and worker time

The captured active plot panel remained below 0.53 ms at p99, but the existing
capture did not expose how much CPU the asynchronous plot-history rebuilds used
across multiple bindings. Added `line_plot_history_snapshot` around rate-limited
app-thread history snapshots and `line_plot_series_build_worker` around the
worker-side pairing, bounds, and decimation work. The mixed experiment/live
overlay path also records `line_plot_scalar_history_snapshot`,
`line_plot_scalar_history_build_worker`, and
`multi_series_plot_points_build_worker`, so the separate flatten and plot-point
stages are visible. These spans record source sample counts, pixel width, and
resulting point counts so a multi-graph capture can distinguish cheap paint
from background CPU contention. A focused export of
`target/builder-graphs-eight-series-workers-20260929.tracy` found 836 history
snapshots taking 1.368 ms cumulative (19 μs max) and 836 series builds taking
4.780 ms cumulative (50 μs max) across the 60.4 s capture. Those paths do not
explain a sustained 10–20 ms/frame penalty in this eight-Modelica-signal case;
the exact Telemetry-dragged channel case remains unmeasured. This is diagnostic
instrumentation only; no FPS improvement is claimed. The instrumentation and
skill guidance are in local commit `6ab45915f`, which was fast-forwarded into
local `main`; no push was performed. Its eight-signal follow-up capture is
recorded below.

### 2026-09-29 — skip the entity-tree snapshot producer while a worker is active

The eight-signal trace `target/builder-graphs-eight-series-workers-20260929.tracy`
contained 28 `populate_entity_tree_view` system events but only 11
`entity_tree_view_snapshot` and 11 `entity_tree_view_derive_worker` spans. The
producer's p50/p95/p99/max was 0.033/0.337/0.648/109.842 ms; snapshot time was
0.170/0.311/0.311/0.523 ms, and worker derivation was
0.140/0.255/0.255/0.482 ms. The 109.842 ms producer event had no nested
snapshot span, so the trace does not attribute it to tree derivation. It does
show the query-heavy producer was invoked more often than snapshots were
started, including no-op calls while a build was pending.

Topology changes now advance the build revision through a small invalidation
system before `ViewModelSet`; the snapshot producer runs only when its dirty
revision has no active worker. This rejects in-flight results without waking
the producer's ECS queries just to mark those results stale. The focused
`cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-luncosim-edit-ui` and
the Tracy-featured production build passed. The focused check was repeated
after optimization fast-forwarded to local `main` at `b2bc78032`; the Tracy
build and capture preceded that fast-forward. No tests were run.

The follow-up Tracy capture
`target/builder-tree-gate-followup-20260929.tracy` ran 40.31 s, recorded 2,650
frames and 12,700,069 zones, and used First Drive High in `rover_build` with
eight plotted Modelica signals. The app reached clear `/api/ready` and exited
through typed API `Exit`; API port 49207 closed. A concurrent Cargo check in
the `sysml-integration` worktree made this a contention-affected diagnostic.
The full filtered CSV export did not finish during this pass, so no post-change
tree timings or FPS improvement are claimed. Local `main`'s
`430121f50 fix(ui): unify hierarchy tree row alignment` is present in the
integrated base; this change does not modify tree row layout. No renderer code
changed. The producer-gating change is local commit `2d27568c1` and has been
fast-forwarded into local `main`; nothing was pushed. This handover remains
uncommitted.

### 2026-09-29 — move application-policy validation out of PreStartup

The earlier startup trace measured `application_policy_activation` at
184.622 ms. Optional-owner filtering and each prepared hook's owner-contract
and entry-arity validation now run during the existing worker preparation;
`PreStartup` checks the authored order and manifest records, then installs the
validated callables in order. The Tracy-enabled production build passed.

The diagnostic capture
`target/policy-prestartup-offload-20260929-150400.tracy` ran 30.37 s (1,212
frames, 6,938,311 zones) on the authored Apollo scene. Three other simulator
sessions were active on ports 4128, 4173, and 48635. The app-thread activation
span measured 0.536 ms: registry clear 0.040 ms, order validation 0.028 ms,
prepared-record validation 0.042 ms, hook installation 0.050 ms, and registry
publication 0.306 ms. Worker source preparation took 12.088 ms and compile,
filtering, per-hook validation, and authored-order selection took 29.992 ms
combined. This differs from the prior 184.622 ms observation but is not a
controlled A/B: the capture is profiler- and contention-affected. At the final
readiness check, `/api/ready` was still false with a `program_failed` item for
`Traverse_x2f_Rover_x2f_Electrical_System`; this run gives no full-readiness or
FPS/physics acceptance result. The owned app accepted typed API `Exit`, exited
successfully, and released API/Tracy ports 4103/8087. The handover remains
uncommitted.

### 2026-09-29 — avoid invalidating plot bindings on steady Graphs paints

The Graphs panel used `PanelCtx::resource_scope` on `VisualizationRegistry` to
read the current binding count on every paint. That helper removes and
reinserts the resource, marking it changed even when the closure did not edit
it. On the next `Update`, `reconcile_persisted_plot_bindings` interpreted this
as a real config edit and rebuilt both entity-to-identity maps by scanning all
`GlobalEntityId` components before reconciling every plot binding. Graphs also
scoped `ActivePlot` every paint, marking it changed even when its value stayed
the same.

Steady paints now read both resources immutably. `ActivePlot` is scoped only
when a plot is first selected or a different plot is hovered; the registry is
scoped only to create a missing default plot. Real registry edits and entity
identity changes still trigger reconciliation. A filtered pre-change
eight-signal Builder trace measured `reconcile_persisted_plot_bindings` at
0.042 ms average, 0.097 ms p95, and 0.370 ms max over 3,595 calls. That false
invalidation is removed, but these timings show it is not by itself the
reported 10–20 ms/frame penalty. The focused
`cargo +nightly-2026-02-27 check --locked -j 4 -p lunco-modelica-ui --features
ui` passed. No updated runtime comparison or FPS gain is claimed yet. A full
offline export of the post-tree-gating capture was stopped after over 80
seconds at full CPU and about 1.2 GB RAM without output. Renderer and tree-row
layout code are unchanged. The source and profiling guidance are in local
commit `d03e77dfb`, fast-forwarded into local `main`; nothing was pushed. This
handover remains uncommitted.

### 2026-09-29 — attribute settled Builder graph and fixed-cycle costs

The owned Tracy production session used `assets/tutorials/sandbox/first_drive.usda`
at High quality in `rover_build`, with eight live Modelica signals in Graphs
and API port 4213. The 60.3-second capture is
`scripts/perf/captures/builder-fps-frame-spikes-20260929.tracy` (3,010 frames,
15,374,306 zones). Another Summer Space School session remained active on port
49361 and GPU utilization varied from 11% to 88%; these numbers are
contention- and profiler-affected diagnostics, not clean FPS acceptance.

In the settled Builder window, panel-body p95/p99/max times were: Graphs
0.646/0.908/1.512 ms, Telemetry 0.832/1.107/2.954 ms, Entities
0.793/1.048/1.617 ms, Inspector 0.129/0.178/0.648 ms, and Spawn
0.192/0.263/0.781 ms. `render_workbench` was 3.769/5.119/8.284 ms p95/p99/max;
`EguiPrimaryContextPass` was 4.124/5.791/9.486 ms. The plotted Graphs panel
does not account for a sustained 10–20 ms frame increase in this reproduction.

The app-thread `run_admitted_fixed_main_schedule` was 9.617/14.951/35.988 ms
p95/p99/max, while the nested `FixedMain` schedule was 6.078/8.705/19.056 ms.
Avian's `run_physics_schedule` was 2.902 ms p95 and 7.172 ms max; this is not
the full-step acceptance sampler. `tick_rhai_scenarios` ran 3,131 times at
0.016/0.045/0.092 ms p50/p95/p99, with one 8.529 ms outlier at trace time
49.881 s. `tick_rhai_scenario_visualization` ran 2,963 times at
0.022/0.056/0.101 ms p50/p95/p99, with a 3.318 ms maximum. The capture had no
child spans to show whether those rare intervals were script callbacks or
exclusive-driver work. Tracy-only subzones now separate Rhai user hooks, event
hooks, native tasks, mission-event projection, and prelude drivers; the
follow-up Tracy build/capture is documented below. The focused
`cargo check -p lunco-scripting-rhai-world -j 4` passed. The subzone changes are
in local commit `740174ce8`, fast-forwarded into local `main`; nothing was
pushed. The first full Tracy build was stopped to avoid overlapping a Cargo
build in `terrain`. That build later finished, and the Tracy production rebuild
and follow-up capture are documented below. No sibling output was removed. No
renderer code changed.

This reproduction added eight Modelica signals through `AddSignalToPlot`; it
did not plot the screenshot's physics Telemetry signal `Dish Gimbal · angular
acceleration`. The exact Telemetry-dragged signal case and a clean same-session
Builder/View A/B therefore remain open. This handover remains uncommitted.

### 2026-09-29 — profile populated Builder plots and batch gravity writes

After the competing `terrain` build finished, `cargo build -p lunco-luncosim
--features tracy -j 4` passed. The first owned capture ran 60.4 s (2,124 frames,
11,234,539 zones) on API port 4213. Builder panels rendered for 1,159 frames;
the default Graphs plot had no live bindings yet. In that interval Graphs body
mean/max was 0.228/4.405 ms. A second owned capture used the same First Drive
High-quality scene in `rover_build` with eight live Modelica series, again on
port 4213. It ran 45.34 s (1,875 frames, 9,613,850 zones) and produced
`target/builder-populated-graphs-rhai-20260929.tracy` (269 MB). The first capture
was `target/builder-rhai-attribution-20260929.tracy` (293 MB). Both are
contention- and profiler-affected diagnostics: the Tutorials session on 49362
and Terrain session on 49403 remained active. The Tracy FPS is not acceptance.

With eight live series, the Graphs body measured 0.380 ms mean, 0.724 ms p95,
1.012 ms p99, and 1.990 ms max. `line_plot_history_snapshot` measured 0.002 ms
mean / 0.092 ms max on the app thread; `line_plot_series_build_worker` measured
0.011 ms mean / 0.534 ms max on the worker. Telemetry body was 0.495 ms mean /
2.601 ms max and Entities was 0.459 ms mean / 1.859 ms max. The Graphs body and
worker work do not explain a sustained 10–20 ms/frame penalty. This did not
reproduce a clean Builder/View comparison or bind the screenshot's exact
physics Telemetry channel.

The populated run's settled main-app p95/p99/max was 34.514/61.241/230.832 ms;
`EguiPrimaryContextPass` was 4.501/6.514/25.260 ms. One 231 ms frame at trace
time 35.592 s overlapped a 100.3 ms `epaint::Tessellator::tessellate_shapes`
span on the render-app thread (longest `tessellate_path` was 68.1 ms). The
empty-plot capture's maximum shape tessellation was 6.70 ms, but these captures
had different durations and UI states, so the comparison is diagnostic only.
The expensive path has not been attributed to a specific widget; no renderer
change is made.

The largest early fixed cycle was 98.2 ms at 6.54 s. Its nested
`system_commands` flush for `lunco_environment::compute_local_gravity` lasted
66.873 ms; later flushes in the same run were at most 0.021 ms. The measured
change is an initial/global-invalidation batch, not steady per-frame gravity
work. The environment system now collects changed `LocalGravity` values and
uses its existing `Commands::try_insert_batch` path once after the query; the
crate README documents the current change-gated/batched contract, and the
profiling skill now directs capture analysis to separate command flush cost.
The build and reprofile evidence appears in the following section. The
handover remains uncommitted.

### 2026-09-29 — verify batched gravity updates in populated Builder

`cargo build -p lunco-luncosim --features tracy -j 4` passed after the gravity
batch change. The owned First Drive High session on API port 4213 ran
`rover_build` with eight live Modelica series for a 60.4-second Tracy capture
(`target/builder-gravity-batch-20260929.tracy`, 2,966 frames,
15,193,811 zones). `--log-diag` was enabled. Other simulator sessions remained
active on ports 4134 and 49362, so this is diagnostic instrumentation under
contention, not clean FPS or physics acceptance.

After the first 10 seconds, the Graphs, Telemetry, and Entities panel bodies
measured 0.366/0.671/0.938/2.119 ms, 0.484/0.866/1.131/3.654 ms, and
0.464/0.855/1.184/2.786 ms mean/p95/p99/max. Inspector and Spawn p95/p99/max
were 0.123/0.179/0.887 ms and 0.185/0.269/0.908 ms. These tree and graph
panels do not explain a persistent 10–20 ms/frame penalty in this diagnostic.
`EguiPrimaryContextPass` was 2.329 ms mean, 4.618 ms p95, 5.742 ms p99, and
13.932 ms max; `render_workbench` was 1.916/3.963/4.844/12.480 ms.

The app-thread `run_admitted_fixed_main_schedule` measured 4.746/10.187/13.907/
26.655 ms mean/p95/p99/max after the first 10 seconds. Avian's full
`PhysicsSchedule` measured 1.928/3.050/3.648/5.363 ms. Neither measurement is
clean acceptance evidence. The `compute_local_gravity` system body peaked at
0.686 ms after startup; its deferred command flush peaked at 0.006 ms. The
initial flush still took 63.814 ms at 3.118 s, close to the prior 66.873 ms
observation. Batching removes per-entity deferred-command setup from this path,
but does not eliminate the one-time structural insertion cost. The next fix
must address that insertion boundary if startup fixed-step responsiveness
requires it.

The first View/Builder comparison attempts were invalid. The preceding trace
`target/builder-view-ab-20260929.tracy` recorded only two Builder panel frames
at second 59 of 60, so it is not a comparison. After my own 4213 session
closed, another process claimed that port with
`assets/scenes/tests/editor/route_interaction/route_interaction.usda`; my new
launch failed to bind, and perspective/graph commands were sent to that other
session before I noticed. I informed the user and stopped all requests to it.
The resulting 2-frame capture was deleted; do not use it as evidence. A valid
same-session comparison using a process-verified port is recorded in the
following section. No renderer code changed.

### 2026-09-29 — compare View and populated Builder in one session

The owned Tracy build ran First Drive High on API port 49123. Before sending
commands, `/proc` confirmed the process executable and working directory were
this checkout's `target/debug/luncosim` and optimization root. The 60.4-second
capture `target/builder-view-ab3-20260929.tracy` contains 2,683 frames and
13,481,769 zones. Tracy was enabled, `--log-diag` was off, and the same process
switched from `sandbox_view` to `rover_build` with the same eight Modelica
signals. Concurrent activity included simulator sessions on ports 4134 and
49362 and a Cargo test in another worktree; this is an instrumented diagnostic,
not clean FPS/physics acceptance.

The selected stable windows were trace seconds 10–35 for View and 44–60 for
Builder. Workbench panel spans first appear at second 39. In View,
`run_egui_context_pass_loop_system` measured 1.646 ms mean, 3.453 ms p95,
4.746 ms p99, and 6.815 ms max; `render_workbench` measured
0.634/1.247/1.581/3.940 ms. In Builder those spans measured
2.690/5.022/8.879/16.575 ms and 1.800/3.376/5.444/10.627 ms respectively.
Builder adds around 1.0 ms to mean Egui pass time and 1.2 ms to mean
workbench time in these windows, but the trace does not reproduce a stable
10–20 ms/frame increase.

With the eight signals, the Graphs, Telemetry, and Entities bodies measured
0.278/0.561/0.874/1.417 ms, 0.376/0.711/1.006/1.766 ms, and
0.365/0.663/1.054/2.605 ms mean/p95/p99/max. The live tree and graph panels
remain below 1.1 ms p99, so they do not support a per-frame deep-clone theory.
Source review shows EntityList caches its derived topology by revision and its
visible rows by tree-open state; TelemetryBrowser builds the grouped catalog
off-thread, caches visibility/row text, and rebuilds its visible-row index only
when catalog, filter, scope, or open branches change. Opening a category
refreshes that index; it does not deep-clone the tree every frame. Immediate-mode
widgets and their paint shapes are still rebuilt for visible rows each frame.

`bevy_egui::output::process_output_system`, which includes `ctx.tessellate`, had
0.259/0.532/0.699/0.980 ms mean/p95/p99/max in View and
1.149/1.703/5.838/83.937 ms in Builder. The large maximum is a single
contention-affected outlier, not a repeatable baseline; this task does not
modify renderer code. `PhysicsSchedule` was 2.119/3.359/4.018/5.754 ms in View
and 1.768/2.803/5.436/9.179 ms in Builder. The app's fixed schedule and physics
timings varied in opposite directions across these windows, so there is no
clean physics or FPS acceptance result. An owned non-Tracy same-session sample
was subsequently taken under the still-active competing workloads; see the
following section. Clean FPS acceptance remains open.

After these measurements, `cargo clean` removed this checkout's `target/`
outputs (12.4 GiB), including the local Tracy files. The recorded metrics and
commands remain here; the raw captures are no longer present.

### 2026-09-29 — sample unprofiled View and Builder frame times

The non-Tracy production build `cargo build -p lunco-luncosim --bin luncosim
-j 4` passed. An owned windowed session loaded
`assets/tutorials/sandbox/first_drive.usda` on API port 49125 with
`--no-vsync --no-throttle`. `/api/ready` reported ready with no hold or pending
work, and `/proc` confirmed the executable and working directory belonged to
this checkout. The API accepted `ActivatePerspective(sandbox_view)` and then
`ActivatePerspective(rover_build)` in the same process. `ReadExposures(engine-health)` was polled 20 times at
500 ms intervals per window after a View warm-up. The default Graphs panel in
Builder was left without `AddSignalToPlot` bindings, so this isolates the
Builder layout from populated plot-series work.

In the settled View window, frame time was 14.63 ms median, 37.31 ms p95, and
52.68 ms maximum; `physics_step_ms` was 0.40 ms median, 0.61 ms p95, and
0.62 ms maximum. In Builder, frame time was 25.26 ms median, 67.19 ms p95, and
72.08 ms maximum; physics was 0.39 ms median, 0.53 ms p95, and 0.79 ms maximum.
The median frame time rose by 10.63 ms (1.73x) while the physics sample stayed
below 0.8 ms. This reproduces a substantial Builder penalty without populated
graphs, but does not locate its owner. Simulator sessions were active on ports
4134 and 4700 during the run, and concurrent Cargo activity was visible though
its checkout was not verified. The reported tails are contention-affected
diagnostics, not acceptance values.

The wider hierarchy review found the Builder Entity and Telemetry trees reuse
revisioned topology/catalog snapshots and rebuild visible-row indexes only
after source, filter, scope, or expansion changes. The USD Prim tree producer
is gated by stage/workbench revisions and skips unchanged projected paths. All
three use the shared tree-row renderer. Expanding a category refreshes its
visible index; it does not clone the full hierarchy on every repaint. Egui
still recreates widgets and paint shapes for rows in the visible scroll region.

The owned app exited through typed API `Exit` and released port 49125. A planned
Tracy rebuild began while new Cargo builds appeared in the Terrain and USD
checkouts; free space fell to 1.4 GiB. That task's build was interrupted before
completion, and `cargo clean` removed only this checkout's outputs (13.8 GiB),
leaving sibling worktrees and shared caches untouched. No new Tracy capture was
produced. The currently available Tracy evidence still shows inexpensive
Graphs/Telemetry/Entities panel bodies and rare Egui tessellation outliers, but
does not reproduce this same-session 1.73x non-Tracy delta. A later run captured
the maximized Build layout with a live `network_system.Shaft_RR.phi` plot and
paired View/Builder windows; it did not recreate the exact Telemetry-dragged
`Dish Gimbal · angular acceleration` series. No renderer or hierarchy-row
layout code changed; this handover remains uncommitted.

### 2026-09-30 — maximize Builder with one live graph

The current non-Tracy production build passed with
`cargo build --locked --offline -j 4 -p lunco-luncosim --bin luncosim`. An
owned process from this checkout loaded `assets/tutorials/sandbox/first_drive.usda`
at High quality on API port `4108`, with `--no-vsync --no-throttle`. `/proc`
confirmed its executable and working directory. `GetReadiness` reported
`ready=true`, `world_hold=false`, and `pending_count=0`. After
`MaximizeWindow`, `CaptureScreenshot` produced
`scripts/perf/captures/builder-current-20260930.png` at 2560×1600. It shows Build with
expanded Entities and Telemetry trees on the left and the Graphs dock open at
the bottom. The visible plot is `network_system.Shaft_RR.phi`, with about 1,500
retained samples. This uses the same line-plot panel path, but is not the exact
`Dish Gimbal · angular acceleration` channel from the supplied screenshot.

In the maximized same-process A/B, each window settled for 15 seconds before
20 `ReadExposures(engine-health)` samples at 500 ms intervals. View frame-time
median/p95/max was 6.17/8.58/9.84 ms, then 5.77/7.74/8.95 ms. Builder with the
live Graphs tab measured 7.59/9.84/12.81 ms, then 7.55/11.95/13.61 ms. The
Builder median was 1.23–1.31× View, approximately 132 FPS versus 162–173 FPS.
Physics-step medians were 0.24–0.26 ms in both modes; the largest observed
physics sample in Builder was 0.65 ms. These runs did not reproduce sustained
20–30 ms frames, but Builder remained below the 150 FPS target.

Before maximizing, one graph-active Builder window measured 14.58 ms median,
18.36 ms p95, and 20.55 ms maximum; the next same-session Builder repeat was
7.15/9.11/9.29 ms. That large variation means the first result is an
unattributed diagnostic, not a repeatable graph cost. This session used
`--log-diag`, and a later process check found other simulator sessions active
on ports `49377` and `49631`; exact overlap with every sampling window is not
known. No clean acceptance run or Tracy attribution was produced. The owned
4108 process exited through `CloseWindow`; its PID and listener were gone on
verification. The other sessions were left untouched. The optimization branch
was fast-forwarded to local `main` at `c9b534ad1`; nothing was pushed. This
handover remains uncommitted.

### 2026-09-30 — Tracy attribution for maximized Builder and live plot

After a checkout-local clean, the Tracy production build passed with
`cargo build --locked --offline -j 4 -p lunco-luncosim --bin luncosim
--features tracy`. The capture server started before the owned First Drive High
session on API port `4108`; the process executable and working directory were
verified in this checkout. Tracy captured 45.32 seconds, 3,696 frames, and
16,964,060 zones to
`scripts/perf/captures/builder-panel-attribution-20260930.tracy`. The scene
reported `ready=true` with no pending work. The API accepted `rover_build`,
window maximize, and `AddSignalToPlot` for
`network_system.Shaft_RR.phi`; the trace contains the Graphs instance and
line-plot history/build spans. This remains a diagnostic trace: Tracy was
enabled, and another simulator session was active on port `49377`; a second
session on `49631` was active by the end, so its exact overlap with the capture
is not established.

Across 1,164 panel renders each, the Graphs body measured
0.125/0.196/0.351/1.289 ms, Telemetry
0.282/0.421/0.623/1.726 ms, and Entities
0.265/0.413/0.601/1.673 ms p50/p95/p99/max. Inspector and Spawn remained below
0.14 ms p99. `render_workbench` measured
0.403/1.476/1.940/8.355 ms; the primary Egui pass measured
0.689/1.761/2.338/8.674 ms. The Egui and workbench maxima occurred around
31.73 s; no panel body exceeded 1.73 ms. For the plotted series, history
snapshotting peaked at 0.012 ms and async series building at 0.017 ms. The
graph's UI history-copy path is not the recurring frame-cost lead in this
capture.

Near the 31.73 s Egui/workbench outlier, the admitted fixed schedule peaked at
5.597 ms, Avian's physics schedule at 1.868 ms, and Bevy's render schedule at
9.345 ms. These overlapping spans are diagnostic and do not add into a single
frame total. The trace does not attribute the isolated Egui maximum to one
panel or reproduce sustained 20–30 ms Builder frames. A post-capture
`ReadExposures` sample in the Tracy-enabled process was 14.82 ms/frame and
1.15 ms physics; it is not acceptance evidence. The controlled non-Tracy
full-size windows above still show Builder below the 150 FPS target at about
132 FPS, with a 1.23–1.31× frame-time ratio versus View. The current profile
does not identify a safe panel-only change that accounts for that remaining
gap. No renderer, UI, or physics source changed in this turn.

The owned `4108` process exited through `CloseWindow`; its PID and ports `4108`
and `8087` were gone on verification. The sessions on `49377` and `49631` were
left untouched. `cargo clean` removed 10.3 GiB from this checkout after the
trace was preserved, restoring 11 GiB free. The handover remains uncommitted.

### 2026-09-30 — bound Modelica compile-to-solve admission

The startup trace showed that a fixed four-operation admission cap can leave
the serialized Rumoca compiler idle while four solve workers lower expensive
DAEs. I changed the combined compile, source-root, and solve-preparation
admission bound to the actual solve-pool worker count plus two staged
operations. The queue remains bounded, and solve completions still commit in
submission order. The pool currently uses one to four threads, so the total
admission ceiling is three to six operations. No renderer code changed.

The production Tracy build passed with
`cargo build --locked --offline -j 4 -p lunco-luncosim --bin luncosim
--features tracy`. In a cold no-scene session, app/window readiness was
observed at 2.386 s from process launch with Tracy enabled. Twin readiness
first reported green 0.82 s after open, then regressed as six Modelica
compilations, physics admission, and deferred USD work arrived. Participants
reported ready 11.24 s after Twin-open; the API later regressed again with USD
connection binding and deferred prims. That run had API timeouts and no stable
readiness soak, so neither the 2 s app target nor the Twin target is accepted
from it. An unprofiled no-scene run under active competing simulator sessions
created its window at 2.05 s and showed readiness clear by 4.16 s after
Twin-open, remaining clear for over 55 s; its readiness monitor did not
capture the exact transition or the required admission-pass boundary. A
separate profiled scene-open run showed clear readiness after 10.71 s and
through a five-second sample window, but also had an immediate false-green
sample and is contention-affected.

The 35.38 s capture
`scripts/perf/captures/sss-lookahead-open-20260930.tracy` shows the six compile
jobs entering the pipeline by 6.47 s from capture start while one cold
`lower_for_live` occupied a solve worker for 7.65 s. The other five jobs were
prepared-solve cache hits (about 1–6 ms); this profile therefore does not
isolate the performance effect of the new lookahead limit. The warm Twin-open
capture `scripts/perf/captures/sss-lookahead-warm-20260930.tracy` instead
recorded one cache miss taking 9.16 s in `lower_for_live`, with cache lookups
of at most 6 ms. These are worker-side readiness costs, not measured UI or
physics-thread stalls. Tracy and the concurrent simulator workloads make all
timings diagnostic only; clean FPS, physics, and readiness acceptance remain
open. The owned sessions were closed, and the other active sessions were left
untouched. This handover remains uncommitted.
