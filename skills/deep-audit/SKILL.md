---
name: deep-audit
description: Run a multi-domain audit of the workspace (USD compliance, performance, DRY/reinvention, legacy/shims, robotics-sim best practices, resilience, UX) with parallel read-only reviewers, then execute fixes as a no-shim migration plan. Use for periodic health audits or before large refactors.
---

# Deep audit — multi-domain review → no-shim migration plan → batched execution

Workflow for auditing the whole workspace or a subsystem and converting findings
into an ordered, executable remediation plan.

Reports live in `docs/reviews/`. A closed report is **deleted** once its findings land —
git keeps it, and a stale report reads as an open problem list. A finding that will not be
fixed soon graduates to its own `docs/reviews/open-<name>.md`, which stays.

## Phase A — parallel read-only review

1. **One reviewer per domain, launched in parallel, all read-only.** Every prompt contains:
   "Do NOT run cargo, builds, or tests; do not edit files." Reviewers return RAW structured
   findings (`file:line | severity | dimension | defect | evidence`), max ~30, prioritized,
   plus a 5-line maturity verdict — data for the coordinator, not prose for a human.
2. **Dedup against prior reviews.** Every reviewer first skims `docs/reviews/*.md` (and
   `git log -- docs/reviews/` for closed ones) and
   reports only NEW or still-unfixed issues. Have reviewers explicitly re-verify known lore
   (memory items, fixed-bug patterns) and mark each ✅ fixed / ❌ still open — verified-fixed
   findings are as valuable as new ones.
3. **The audit dimensions** (adjust per run, but these are the standing set):
   - **USD compliance** — follow OpenUSD conventions, not parallel inventions: composition
     arcs, defaultPrim, UsdPhysics/UsdGeom/UsdLux names with their real semantics.
     **Minimize custom `lunco:` schema surface**: before declaring a new `lunco:` attribute,
     check whether a USD-native concept already expresses it (kind, purpose, variants,
     payloads, `doc`, existing applied APIs, physics schemas). Every `lunco:` attr that IS
     authored must be declared in `schema.usda` (the staleness gate
     `scripts/run_rust_tests.sh -p lunco-usd --filter schema_generation::` enforces generatedSchema sync).
     Name-squatting check: places that adopt Pixar/Omniverse NAMES but diverge semantically.
   - **Modelica/cosim conformance** — flattening, connect semantics, initialization,
     events; master-algorithm honesty (declared ZOH/Jacobi contract vs. actual behavior);
     input-strip coverage at EVERY source seam; solver claims in comments match the code.
   - **Performance** — per-frame allocations, systems without run conditions/change
     detection, O(n²)/full-set rebuilds, per-sample virtual dispatch in bake/solve inner
     loops, main-thread work that belongs on `AsyncComputeTaskPool`, caches whose cap is
     below the resident set (a defeated cache reads as "working" in a profile of an idle
     scene — always check cap vs. resident bound), missing `AssetId` dedup on shared assets,
     unconditional component writes that dirty change detection.
   - **DRY / reinvented wheels** — hand-rolled code where a workspace dep or the openusd
     fork already provides it (REUSE openusd — fix the FORK); duplicate numeric cores for
     the same concept (two spline evaluators, two force laws); dead dependencies; a fix
     applied to one of two parallel code paths (the OTHER path still has the bug — always
     grep for the twin).
   - **Legacy & shims** — dual APIs, back-compat aliases, "temporary" bridges, advertised
     surfaces with no implementation, comments describing architecture that was never built
     (the "cached DAE" lie pattern). The rule is ONE FORM: the migration plan must delete
     the superseded path in the same phase that lands the new one.
   - **Robotics-industry best practices** — fixed-timestep discipline, determinism
     (peer-identical bakes/sums, no hash-order iteration into physics), frame safety
     (grid-absolute vs render frame typed via `lunco_core::coords::{GridPos,RenderPos}` —
     new pose-carrying APIs must use them), unit/precision hygiene (no f32 downcasts of
     grid-absolute values), tire/terramechanics fidelity honestly stated, joint lifecycle
     (attach in avian `Prepare`, born-disabled collision), readiness gating.
   - **Resilience** — wedge states (in-flight flags with an early-return that never clears
     them; every guard must fail visibly at its owning boundary), panics on malformed input (checked
     arithmetic on header-controlled sizes), all-or-nothing loads (one bad asset must skip
     +report, not stall the scene), infinite per-frame retries with no give-up, failures
     that never reach the UI (`warn!` is not surfacing — trigger an Error-severity
     `TelemetryEvent`; the StatusBus observer fans it to the status bar and Diagnostics).
   - **UX for robotics engineers** — inspector derives from schema (never hardcodes),
     disabled controls carry `on_disabled_hover_text` saying what would enable them, no
     literal RGB (DesignTokens), change-driven panels, and the operate-and-observe set:
     TF/frame gizmos, joint-state, CoM/inertia/forces gizmo, telemetry browser → plot.
   - **Mission-modeling capability** — what system-level domains exist and at what fidelity
     (power, thermal, comms/link budget, orbits, timelines-with-resources), measured
     against STK/GMAT/Basilisk-class expectations; rank gaps with the cheapest credible
     path (reuse: anise/hifitime, rhai over existing query substrate, pure asset changes).

## Phase B — the report

One file: `docs/reviews/YYYY-MM-DD-<name>.md`. Findings tabulated per domain with stable
IDs (U1…, P1…, T1…, C1…, A1…, X1…, M1…, W1…, S1…) — the IDs are how fix agents are tasked
later, so keep them stable. End with the **migration plan**: ordered phases, each phase
deleting the superseded form in the same phase (no shim survives a phase boundary), with a
sequencing rationale (correctness before perf; change-granularity before profiling — idle
churn drowns measurements; schema before inspector; substrate types before mechanical
sweeps). Record execution state in the memory file as phases land.

## Phase C — batched execution

Follow `skills/subagent-batches` exactly: disjoint file lots, agents NEVER run cargo,
and the coordinator runs focused checks with `-j 4` after the batches land. Run a
broader suite only when the audit scope requires it; set
`CARGO_PROFILE_TEST_STRIP=debuginfo` when large Bevy test binaries make disk usage
material. Additional rules:

- **Attribute every test failure before fixing.** Preserve the working tree and rerun
  the failing target when needed. Separate failures caused by the current change,
  asset/test drift, and intentional negative fixtures before deciding code-fix versus
  test-update.
- **Substrate first, consumers fanned out.** For a cross-cutting type change, land the core
  types yourself (Lot 0), then launch consumer lots in parallel against the new signatures;
  compile breaks between lots are expected and resolved by the single end check.
- **Cross-file handoffs are the coordinator's job.** Agents report edits they couldn't make
  outside their lot (a caller in another lot's file, a cache-version bump); apply them
  yourself between batches — never let one drop.
- Findings discovered DURING execution go back into the report as an addendum section, not
  into the void.

## Definition of done

Every phase: checks cover the touched owners, failures are attributed and
dispositioned, superseded forms are deleted, and the report is updated. Rerunning
Phase A against a clean tree must not reproduce fixed findings.
