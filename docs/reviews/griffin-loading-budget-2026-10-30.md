# Griffin loading budget

The original Griffin landing scene reaches physics readiness in **4.494 seconds**
in a normal-release, warm-cache run with no other simulator loading. That run
passes landing, all 394 source hashes, runtime diagnostics and owned shutdown.
A concurrent-simulator repeat takes **7.942 seconds**: consistent under-five-second
loading under contention is **not established**. Cold compiler-cache acceptance
is also unmeasured for the final change. These times exclude application bootstrap
and the later landing maneuver.

## Integration and scope

The application is based on latest main `02ba65ac2`. The October 31 fast-forward
from `389e91911` preserved all 21 non-overlapping dirty/untracked files and merged
the loading work with main's native simulation-host contracts. The user authorized
local integration and proceeding without Trello.
Unrelated terrain, propulsion, runtime UI and waypoint-editor work remains outside
this change.

All application Rumoca consumers pin signed-off commit
`2db205a9d9468c08adda46c36ada02078db9b34f` from `LunCoSim/rumoca` through Git
dependencies and `Cargo.lock`. Builds require no adjacent dependency checkout.
Its owner contracts are documented in
[Rumoca output preparation](rumoca-chain-probe-preparation-2026-10-27.md).

Publication verification on November 1:

- Rumoca main includes the seven signed-off loading commits ending at the pinned
  revision. Fresh locked library tests pass: evaluator 242, native execution 115,
  Solve IR 426 and solver 553; one IR timing diagnostic is ignored.
- LunCoSim's compiler and solver consumers pass
  `cargo check --locked -p lunco-modelica-compiler -p lunco-modelica-solver -j 4`
  against the published Git source. The lockfile changes only the 29 Rumoca-family
  source revisions, preserving other dependency versions and edges.
- Nightly metadata generation rejects Cargo failure and a missing application
  package before publishing a version. The workflow executes its regression test
  before metadata generation; the real local metadata produces
  `0.6.0-nightly.139.2` for run 139, attempt 2.
- This publication reuses the retained canary and production evidence below;
  it does not establish a new full-cohort or packaged desktop acceptance result.

The target is `astrobotic-griffin-1/scenes/griffin_1_surface_ops.usda`, physical
root `/Griffin1SurfaceOps/Lander`, with `landing_handoff` as mission acceptance.
The shared model repository has concurrent media work, including two runtime UI
manifests that the Twin owner correctly rejects. The measurement checkout is
`/home/rod/Documents/models/lunar-base-model-griffin-loading`, based on model
commit `d2d1575a22ce79829fb8f3a655ab7571afdaaae0`. It contains the validated binary
wheel layers, complete installed resources and six original-scene inputs copied
byte-for-byte from the source hashes recorded by the integrated-main baseline.
Its manifest selects the original landing scene. No foreign media/default-scene
edit was reverted, and no duplicate-manifest rejection was weakened.

## Current mechanisms

- Route control and selection query the admitted composed-stage trigger attribute
  through `QueryUsdPrim`. A child without that attribute is excluded; document
  edits retain document validation. This removes redundant document composition.
- Four FLIP wheel layers use native binary USD. Document API `SaveAsDocument`
  created each USDC; canonical native text exports before/after are byte-identical.
  Four references were changed through `ApplyUsdOps`, saved and read back.
  Geometry, materials and physics are unchanged; layer bytes fall from about
  43.0 MB to 8.464 MB. The model's
  `contracts/evidence/loading-geometry-2026-10-30.json` records hashes and sizes.
- `lunco-usd-bevy::UsdPendingChildAdmissions` uses one ordered queue after the
  existing projection-command flush. A single-threaded nested schedule repeats
  parent binding, child admission and reset-stack detachment within one Update.
  Cumulative 4 ms/128-prim and 1 ms/128-child budgets bound all passes, with a
  combined wall limit between atomic batches. Parent readiness, stage generation,
  mount, duplicate and cancellation checks remain authoritative. This is ECS
  ordering, not policy.
- `UsdStageProjectionPlan` reuses native `PurposeInfo` through its existing prim
  index for one immutable extraction. It calls maintained `purpose_info_from`
  for descendants and `purpose_info` at omitted-ancestor boundaries. The same
  result feeds native effective visibility. No inheritance rules are duplicated.
- Transform preparation validates the authored op order once and constructs one
  native `XformQuery` for local matrix, reset and time-variation facts. Inactive
  prims retain metadata without matrix evaluation. Canonical geometry stays f64;
  render decomposition retains its explicit narrowing boundary. State expires
  with the snapshot. No OpenUSD dependency source was modified.
- Rumoca uses checked incremental chain construction, evaluation-only output
  preparation, structural rejection of unrelated assignment candidates, reuse
  of identical ordered refresh schedules and bounded native dispatch functions.
  Structural dependency propagation shares immutable ordered sets; unions detach
  before mutation. Public owned results, wire data, source provenance, input
  rejection and numerical execution retain their contracts. No solver proof,
  equation, precision or readiness check was removed.

The physics assembly owner retains readiness-frozen joints in initial mechanical
membership while excluding authored disabled joints. Live active mass still uses
active links. The focused regression failed before this fix and passes after it;
readiness release no longer changes the initializer's authored mass expression.

The coupled owner waits for existing domain-discovery and pending-port holds,
including generated networks without a participant yet. Its certificate still
validates the complete source, parameters and wiring snapshot. Ready acyclic
participants use their own numerical preparation; missing external samples remain
pending. This prevents publication of an independent network from superseding an
otherwise identical composite source during its construction.

These are generic preparation and projection mechanisms, not changeable lifecycle
or Twin policy. No additional Rhai policy hook is needed. Architecture 61 and the
performance-profiling runbook describe snapshot lifetime and child ordering.

## Product measurements

The driver's `atomic-world-v1` probe reads readiness, expected Twin identity,
spawned root and unpaused physics admission in one read-only World evaluation.
The settled bootstrap must fail that target probe. Screenshot publication is a
separate timestamp; landing, settled terrain, source hashes, diagnostics and
owned shutdown remain required. Earlier sequential-query timings are not directly
comparable: changing the probe improves measurement accuracy, not the application.

| Normal release, same atomic probe and original landing source | Physics ready | Conditions |
| --- | ---: | --- |
| Complete declaration admission, fixed discovery batches | 6.894 s | Another simulator; one initializer; prepared/native hits |
| Bounded discovery, first run | **4.494 s** | No other simulator during loading; prepared/native hits |
| Prior binary comparison attempt | 7.074 s | Another simulator restarted before this run; not a quiet baseline |
| Bounded discovery, repeat | 7.942 s | Concurrent Griffin media simulator; prepared/native hits |

Changing workloads prevent an isolated whole-scene speedup claim. The first
bounded-discovery run loads checked initializer IR in 513.6 ms and constructs
its live solver in 982.0 ms; the contended repeat takes 880.1 ms and 1517.2 ms.
Both construct one initializer with 1785/1785 native cache hits and zero native
compilations. In both, all six networks publish before physics projection settles.
The single-simulator run opens at 01:06:14.990 UTC and reaches readiness around
01:06:19.484 UTC. The other simulator's `/proc` start time is 01:06:32.300 UTC,
confirming that it starts after that loading window; it remains active for the
comparison and repeat. No foreign session was controlled.

The current first-ready image shows terrain and the descent controls; the final
image shows the landed vehicle and its shadow with touchdown controls. All source
hashes match, runtime diagnostics report zero errors/warnings, and the process
and port are gone. The source cohort is the isolated original landing Twin;
concurrent shared-checkout media/UI-manifest changes are not certified by it.

The 240-frame engine history is a tail window, not proof of the worst stall across
the entire load. Cache first-fill timing must be measured separately from these
warm cohorts; bounded artifacts can be evicted by tests or other Twins.

## Focused validation

- Normal production build:
  `cargo build --release --locked -p lunco-luncosim --bin luncosim -j 4` passes.
- `scripts/api/test_usd_child_admission.py` exercises nine authored production
  assertions: initial 64-level projection with 1024 siblings, malformed edits and
  replacement while child admission remains pending. It verifies retired entities
  stay absent and API shutdown completes. The expanded hierarchy takes 24 updates,
  below the 32-update gate. The final discovery-budget build takes 25 updates. Before adding the siblings, the same deep chain took
  72 updates on the single-pass baseline and seven with bounded repeated passes.
  The fixture expansion was authored, saved and read back through the USD API.
- The existing `native_reset_stack` gate passes 10/12 checks on both the
  single-pass baseline and repeated-pass build. The same two live reset-removal
  checks fail; initial deep/shallow reset projection and malformed-edit rejection
  pass. This is retained as an unresolved pre-existing validation limit.
- The production initialization network and receipt retirement pass all 46
  authored checks on the final discovery-budget build. The inconsistent-initial-equation scene exits with its owner
  diagnostic and code 1, without a panic or missing verdict; its port is released.
- Windowed production `route_lifecycle`: 124 authored checks pass, including
  topology edits, invalid targets and live program replacement.
- All 17 native projection-plan tests pass, covering omitted/blocked/unknown
  purpose, non-imageable ancestors, invalid visibility, reset stacks, animation,
  inactive/empty transforms and billion-scale f64 positions. Three alternating
  diagnostic pairs show deep purpose extraction 20.1–23.7% faster. Three final
  transform-query pairs improve synthetic extraction/complete-recipe medians
  by 3.0–9.0%, depending on hierarchy and transform presence.
- Current Rumoca IR: all 427 library tests and scoped lib/test Clippy pass.
  Three alternating same-toolchain register-copy diagnostics improve by 4.37x,
  16.53x and 15.48x at dependency widths 1, 64 and 256.
- Fixed 20-model MSL canary at `2db205a9`: no per-model phase, diagnostic,
  simulation-status or agreement-band delta from `ff830339`; 12 strict-high
  comparisons, no missing/skipped traces, the same six compilation failures and
  two solver failures. This is partial coverage, not a 20/20 pass or full-cohort
  parity. Dependency review and comparison JSON are committed beside that source.
- Prior retained dependency gates cover evaluator chains (242 tests), solver
  schedule reuse (553 tests) and native dispatch (115 tests), including negative
  cases and regressions demonstrated against the preceding implementation.

## Remaining attribution and evidence

A fresh Tracy 0.14.1 warm capture passes native CSV export and framing checks:
314,549,232 compressed payload bytes, 19,486 blocks and 77,950 framing bytes in a
314,627,182-byte file. It attributes 858.6 ms to USD facts, 845.9 ms to initializer
prepared-cache reconstruction and 1523.6 ms to solver construction before the
shared dependency-set change. Exact-zone CPU samples identify dependency-tree
copies in preparation and symbolic shared-value construction plus Cranelift
IR/hash work in the constructor.

The retained pre-change capture showed 43 projection batches totaling 75.3 ms
inside 53 updates totaling 2688.4 ms. The bounded repeated-pass change addresses
hierarchy-dependent update delays; the authored deep-chain comparison above is
its direct evidence. It does not establish the whole-scene five-second target.

The declaration-admission diagnostic captures 51.61 seconds and 6,283,584 zones
with the owned PID 743198 and a verified 1 ms sampling period. Native export
passes; its 69,514,344 bytes contain 69,500,254 compressed payload bytes plus
14,090 framing bytes across 3521 blocks. The run uses
`info,bevy_ecs::schedule::executor=warn,lunco_usd_bevy=debug`; its 8.288-second
readiness is instrumented and does not replace normal-release acceptance.
The binary is stripped, so attribution uses named zones rather than an `nm`
symbol index.

Between first and last network classification, discovery executes 34 updates
and 7.761 ms of CPU work across 2.3245 seconds. Visual projection completes
in 24 updates before FLIP's late classification. This identifies discovery
admission pacing as avoidable delay. The final owner uses a one-millisecond time
budget and 512-candidate cap; unvisited candidates retain their readiness hold
and resume in stable entity order. `domain-admission-tracy/discovery-attribution.json`
retains the exact root events and interval; `domain-discovery-comparison.json`
records normal-build timings, cache counts, source equality and workload limits.

Evidence stays ignored under `.cache/griffin-loading-20261030/`:
`main-integration/dependency-production-first/`,
`main-integration/dependency-production-open-twin/`,
`main-integration/projection-pass-large.log`,
`main-integration/admission-enlarge-receipt.json`,
`main-integration/domain-admission-atomic-0/`,
`main-integration/domain-discovery-initialization/`,
`main-integration/domain-discovery-atomic-0/`,
`main-integration/domain-discovery-atomic-1/`,
`main-integration/domain-admission-tracy/`, purpose/transform comparison JSONs,
`main-integration/dependency-comparison.json`, `dependency-canary-results/` and
`main-integration/loading-tracy-warm/`. Earlier baseline/route/geometry receipts
remain in that root. Task API ports are 5641 and 5642; completed sessions are
closed. Foreign sessions are never controlled.
