# Griffin loading budget

The less-than-five-second target is **not met**. The current normal release
reaches physics readiness in **7.659 seconds** with prepared/native cache hits
and another simulator active. It completes the original landing scene, preserves
its source hashes and reports zero runtime diagnostic errors/warnings. The first
run with the current compiler identity takes **13.545 seconds** while populating
prepared artifacts. Neither timing includes application bootstrap or the later
landing maneuver. Do not present microbenchmarks or instrumented timings as
product acceptance.

## Integration and scope

The application is based on latest main `02ba65ac2`. The October 31 fast-forward
from `389e91911` preserved all 21 non-overlapping dirty/untracked files and merged
the loading work with main's native simulation-host contracts. The user authorized
local integration and proceeding without Trello. No remote push is authorized.
Unrelated terrain, propulsion, runtime UI and waypoint-editor work remains outside
this change.

The approved local Rumoca worktree is `../rumoca-griffin-loading`, branch
`fix/griffin-loading`, at signed-off commit `2db205a9d9468c08adda46c36ada02078db9b34f`.
All application Rumoca consumers use that same relative path. The dependency is
local and unpublished; the adjacent checkout is required to build this integration.
Its owner contracts are documented in
[Rumoca output preparation](rumoca-chain-probe-preparation-2026-10-27.md).

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
  existing projection-command flush. Direct children enter after the parent
  commits; child projection runs on the following update. The 4 ms/64-prim and
  1 ms/128-child budgets, parent readiness, stage generation, mount, duplicate
  and cancellation checks remain authoritative. This is ECS ordering, not policy.
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

These are generic preparation and projection mechanisms, not changeable lifecycle
or Twin policy. No additional Rhai policy hook is needed. Architecture 61 and the
performance-profiling runbook describe snapshot lifetime and child ordering.

## Product measurements

The driver requires a qualified bootstrap, then the expected Twin, spawned root,
clear `GetReadiness` and `physics_state_ready`. It records first-ready screenshot
publication separately, completes landing and a settled measurement window,
checks source hashes and diagnostics, and exits the owned app before enforcing
`--max-load-seconds 5`. Keep entrypoint, cache state and concurrent workloads
consistent when comparing timings.

| Normal release cohort | Physics ready | Conditions |
| --- | ---: | --- |
| Earlier quiet warm | 5.313 s | Before latest-main integration; no other simulator/build |
| Integrated-main baseline | 6.042 s | Before native query/set reuse; quiet loading window |
| Current dependency first fill | 13.545 s | Exact-file open; prepared misses, native hits |
| Current dependency warm | 7.659 s | Direct Twin open; prepared/native hits; another simulator |

The current warm run publishes its first screenshot at 8.066 s and reaches landing
handoff at 29.300 s. Terrain, lander, shadows and touchdown controls are visible.
All source hashes match, diagnostics report zero errors/warnings, and the process
and port are gone. Its initializer spends 741.5 ms loading checked prepared IR
and 1393.2 ms constructing the live solver. A separate exact-file warm attempt
reached readiness at 7.330 s, but its driver exited with signal 15 before writing
its receipt; it is incomplete evidence. Its owned app was verified and closed
through API `Exit`. Direct Twin opening does not establish a loading improvement.

The differing workloads and revisions prevent an isolated whole-scene speedup
claim. The original scene in the shared model checkout remains subject to its
concurrent duplicate UI-manifest issue; the isolated receipt does not certify
that shared checkout.

## Focused validation

- Normal production build:
  `cargo build --release --locked -p lunco-luncosim --bin luncosim -j 4` passes.
- `scripts/api/test_usd_child_admission.py` exercises nine authored production
  assertions: initial 64-level projection, malformed edits and replacement while
  child admission remains pending. It verifies retired entities stay absent and
  API shutdown completes. The final integrated hierarchy takes 73 updates, below the 88-update gate.
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

Its projection-admission interval has 43 projection batches totaling 75.3 ms,
inside 53 updates totaling 2688.4 ms. Batches commonly finish below the 4 ms
budget while the 64-item cap limits progress. This is a candidate for a bounded
pacing experiment, not proof that increasing the cap preserves responsiveness
or reaches the loading target.

Evidence stays ignored under `.cache/griffin-loading-20261030/`:
`main-integration/dependency-production-first/`,
`main-integration/dependency-production-open-twin/`,
`main-integration/child-admission-final.log`, purpose/transform comparison JSONs,
`main-integration/dependency-comparison.json`, `dependency-canary-results/` and
`main-integration/loading-tracy-warm/`. Earlier baseline/route/geometry receipts
remain in that root. Task API ports are 5641 and 5642; completed sessions are
closed. Foreign sessions are never controlled.
