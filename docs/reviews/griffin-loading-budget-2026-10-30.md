# Griffin loading budget

The less-than-five-second target is **not met**. The current normal release
reaches physics readiness in **6.894 seconds** with prepared/native cache hits
and another simulator active. It constructs one coupled initializer, completes
the original landing scene, preserves its source hashes and reports zero runtime
diagnostic errors/warnings. This excludes application bootstrap and the later
landing maneuver. Instrumented timings are diagnostic, not product acceptance.

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
| Single projection pass, complete initial joint topology | 6.819 s | 128 prim/child caps; prepared/native hits |
| Independent acyclic numerical admission | 6.409 s | One coupled initializer; prepared/native hits |
| Repeated projection passes before declaration admission fix | 7.325 s | Two identical-source initializers; thermal wiring superseded the first |
| Current complete declaration admission | 6.894 s | One coupled initializer; prepared/native hits |

Another simulator was active throughout these cohorts; they are not quiet-machine
acceptance. The current initializer loads checked prepared IR in 660.5 ms and
constructs its live solver in 1465.4 ms, with 1785/1785 native cache hits and zero
native compilations. Physics projection settles about 2.66 seconds after opening;
thermal/FLIP declarations finish later, before coupled capture at about 3.83
seconds. A fresh diagnostic capture is needed to attribute that remaining gap.

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
  below the 32-update gate. Before adding the siblings, the same deep chain took
  72 updates on the single-pass baseline and seven with bounded repeated passes.
  The fixture expansion was authored, saved and read back through the USD API.
- The existing `native_reset_stack` gate passes 10/12 checks on both the
  single-pass baseline and repeated-pass build. The same two live reset-removal
  checks fail; initial deep/shallow reset projection and malformed-edit rejection
  pass. This is retained as an unresolved pre-existing validation limit.
- The production initialization network and receipt retirement pass all 46
  authored checks. The inconsistent-initial-equation scene exits with its owner
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

Evidence stays ignored under `.cache/griffin-loading-20261030/`:
`main-integration/dependency-production-first/`,
`main-integration/dependency-production-open-twin/`,
`main-integration/projection-pass-large.log`,
`main-integration/admission-enlarge-receipt.json`,
`main-integration/domain-admission-atomic-0/`,
`main-integration/domain-admission-initialization/`, purpose/transform comparison JSONs,
`main-integration/dependency-comparison.json`, `dependency-canary-results/` and
`main-integration/loading-tracy-warm/`. Earlier baseline/route/geometry receipts
remain in that root. Task API ports are 5641 and 5642; completed sessions are
closed. Foreign sessions are never controlled.
