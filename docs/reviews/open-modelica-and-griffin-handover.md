# Modelica initialization and Griffin Twin handover

Updated 2026-10-07. Declaration-scoped Modelica defaults are fixed and committed;
current main is integrated into optimization. Griffin Twin readiness and mission
acceptance remain open. This report is the continuation point for those issues.

## Branch and work preservation

Work checkout: `/home/rod/Documents/luncosim-workspace/optimization`.
The implementation tree was clean at `db330e4f8` before this documentation update.

| Commit | Result |
|---|---|
| `cd1c7918b` | Editor and simulation preparation performance work committed |
| `b4b3aabae` | Modelica inputs initialized in declaration scope |
| `804c54144` | Main revision integrated, including differential routing and native mass-property changes |
| `db330e4f8` | Merge of main into optimization |

The defaults fix and final merge did not modify main. Nothing was pushed.
Existing authored changes in lunar-base-model were preserved and were neither
staged nor committed by this task. Trello was skipped at the user's instruction.
Preservation stashes remain; do not restore or drop them blindly:

- `fdd0400a7ab294a0a77ab3881d6b3135fb0076a3`: original optimization work.
- `60f2059b347128e77bc6b1bc2e72a4e5f6c7c4b7`: concurrent main work preserved for integration.

## Completed initialization fix

Two paths caused collisions. Compiler/worker admission stripped input bindings
and collected defaults by leaf name, allowing separate classes and instances to
overwrite one another. Editor compile dispatch independently seeded inputs from
the document-wide index, including nested declarations and function parameters.

The current contract preserves original source declarations. Rumoca's existing
DAE classification retains actual external input bindings as initialization
expressions at fully qualified variable paths. Internal and connected bindings
retain equation ownership. The initialized solver supplies initial input values;
editor dispatch preserves existing runtime values and reads parameter metadata
only from the selected class. AST numeric defaults serve source presentation,
not runtime initialization. Native live, reset, batch, CLI and browser host paths
consume the compiled initialization; explicit input overrides remain authoritative.

The old source stripping, default maps, suffix matching and worker reseeding APIs
were removed with their callers. Prepared solve cache version **6** rejects
artifacts from the previous initialization contract. The merge retains main's
strict compiled-content-derived `prepared_source_key` in cached rebuilds.
Generic equation lowering and initialization belong to the compiler/solver;
there is no changeable scenario policy requiring a Rhai hook here.

The new vendored `rumoca-phase-dae` source matches the existing Rumoca pin
`eaa5291ff610085cfc02f9673fcb393245feaa9b` (0.9.20). Its local source changes are
limited to input initialization/classification in `src/lib.rs` and binding
conversion in `src/binding_conversion.rs`.

Primary implementation and regression entry points:

- `third_party/rumoca/rumoca-phase-dae/src/{lib.rs,binding_conversion.rs}`.
- `crates/lunco-modelica-compiler/src/lib.rs`.
- `crates/lunco-modelica-worker/src/worker.rs` and its `worker/` modules.
- `crates/lunco-modelica-execution/src/bin/{modelica_run.rs,lunica_worker.rs}`.
- `crates/lunco-modelica-ui/src/bin/lunica.rs`.
- `assets/scenarios/tests/modelica_scoped_input_defaults.rhai`.
- `scripts/api/run_editor_scene_test.py`: production scenario attachment with
  active Twin ownership, real Rhai verdict, schema capture and owned API shutdown.

Architecture, crate index, workaround documentation, co-simulation guidance and
the run-modelica skill were updated. The integrated runtime schema regenerated
the unchanged command reference: 262 commands across 54 crates.

## Retained validation evidence

Paths below are relative to this checkout. `target/perf/` is generated local
evidence, not a committed archive; preserve relevant logs before cleaning it.

| Validation | Result and scope | Evidence |
|---|---|---|
| Compiler declaration-scope test | PASS; pre-merge, default semantics unchanged by merge | `target/perf/scoped-defaults-compiler-test.log` |
| Worker/execution focused check | PASS; pre-merge | `target/perf/scoped-defaults-final-check.log` |
| Integrated production build | PASS | `target/perf/scoped-defaults-merged-build.log` |
| Integrated windowed production Rhai gate | **PASS, 20/20** | `target/perf/scoped-defaults-merged-runtime.log` |
| Integrated runtime command schema | 262 commands, 54 crates; generated reference unchanged | `target/perf/scoped-defaults-merged-runtime.schema.json` |
| Skill catalogue validation | PASS, 43 skills | Recorded during implementation |
| Landing and engine manifest gates | Both exit 1, readiness timeout, **NO-VERDICT**; before final editor cleanup and merge | `target/perf/griffin-scoped-defaults-gates.json`, corresponding logs below |
| Post-merge default Twin opening | Contract and diagnostics clear; readiness still held during bounded cold preparation | `target/perf/griffin-default-merged.{log,json}` |

Focused commands already run successfully; a documentation-only handover does
not require rerunning them:

```sh
cargo test --locked --offline -j 4 -p lunco-modelica-compiler --lib runtime_input_defaults_follow_declaration_scope -- --nocapture
cargo check --locked --offline -j 4 -p lunco-modelica-worker -p lunco-modelica-execution --tests --bin modelica_run
cargo build --locked --offline -j 4 -p lunco-luncosim --bin luncosim
```

The integrated production regression used:

```sh
LUNCOSIM_BIN=target/debug/luncosim \
LUNCOSIM_EPHEMERAL_SETTINGS=1 LUNCOSIM_ISOLATED_RUN=1 \
LUNCOSIM_CONFIG=target/perf/scoped-defaults-merged-runtime.config \
python3 scripts/api/run_editor_scene_test.py \
  --port 4261 --timeout 90 \
  --scene /home/rod/Documents/luncosim-workspace/optimization/assets/scenes/fixtures/usd_query_api/site.usda \
  --scenario lunco://scenarios/tests/modelica_scoped_input_defaults.rhai \
  --log target/perf/scoped-defaults-merged-runtime.log
```

The gate covers distinct class defaults, repeated instances, inheritance,
parameter/function expressions, arrays, internal bindings, continuous state,
one qualified instance override, reset preserving that override, a visible
non-crashing invalid-default diagnostic and a successful successor batch solve.
It asserts a 30-unit initial integrated rate and 33 after the qualified override.
The fixture has no Sun and reports a separate missing-Sun diagnostic; the gate
does not claim zero global runtime diagnostics for that fixture. Browser/wasm
runtime acceptance and a full suite were not run.

## Open Griffin readiness and mission acceptance

Canonical Twin:
`/home/rod/Documents/models/lunar-base-model/twins/astrobotic-griffin-1`.
Default scene: `scenes/griffin_1_surface_ops.usda` within that Twin.

Earlier independent production manifest runs used these exact scene/verdict
pairs, with `--max-ticks 21000 --readiness-timeout 120`:

| Scene relative to Twin | Verification | Owned API port | Result |
|---|---|---|---|
| `tests/griffin_surface_ops_contract.usda` | `GriffinSimulationAccuracyRequirements::Verify_GriffinLandingStability` | 4262 | Runner error after 120-second readiness bound; no authored verdict |
| `tests/griffin_engine_commands.usda` | `GriffinPropulsionRequirements::Verify_GriffinEngineCommands` | 4263 | Runner error after 120-second readiness bound; no authored verdict |

Logs: `target/perf/griffin-landing-scoped-defaults.log` and
`target/perf/griffin-engine-scoped-defaults.log`. Those runs preceded final editor
dispatch cleanup and integration, so they do not establish that the final merged
binary still exceeds the same 120-second bound.

The earlier windowed default-scene probe on port 4264 held three tickets: USD
physics admission, MainPropulsion_System and AttitudePropulsion_System.
Their compilation succeeded, but solver preparation had not completed.
Evidence: `target/perf/griffin-default-scoped-defaults.{log,json}`.

The **post-merge** windowed probe opened the exact default scene, settled its
time policy and sampled readiness five times at three-second intervals.
The last sample reported `ready=false`, `world_hold=true`, `faulted=false`,
with five pending tickets: USD physics admission, AttitudeActuation_System,
MainPropulsion_System, AttitudePropulsion_System and FLIP_System. Preparation
was progressing; AttitudeActuation lowering completed in about 11.45 seconds
just before shutdown. `RuntimeDiagnostics.errors` was zero and the active Twin
contract had no verification-registry or SysML source errors.

This bounded cold-start observation proves successful opening and clear reported
contracts. It does **not** prove a permanent preparation stall or successful
physics/mission execution. Do not declare the Twin operational yet.

Next work, in order:

1. Launch the final production binary from this checkout on a newly verified
   free API port. Use the exact Twin/default scene and allow a longer bounded
   cold-preparation interval. Record readiness transitions, preparation jobs,
   runtime diagnostics and the eventual authored verdict. Keep other sessions
   untouched; do not bypass the hold or clock boundary.
2. If preparation remains held, trace the worker's `SolvePreparationPool::submit`
   and the solver's `simulation_session::lower_for_live` boundary, including
   compiled identity, completion, cancellation and admission fencing. The
   current evidence does not identify the root cause.
3. Rerun both manifest gates on the final binary after readiness is resolved,
   then validate the required surface-operation, pilot/ramp and FLIP routes.
   Require authored verdicts before reporting mission acceptance.

## Open aggregate-output readback defect

The defaults probe also exposed an independent observation defect. A root
aggregate `output Real y` reads zero in live snapshots and batch output while
the continuous state driven by the same equation integrates at the correct
30-unit rate, or 33 after override. Qualified input values and a simple nested
output read correctly. The committed defaults gate asserts integrated state and
qualified slots; it does **not** establish correct aggregate-output readback.

Add a separate authored Rhai regression that requires the aggregate output to
match the integrated rate in live and batch observation, then correct the
authoritative solver output reconstruction owner. Elimination/alias
reconstruction is an investigation lead, not a confirmed root cause. Do not
accept zero or add a fallback to conceal the defect.

## Performance and session boundaries

Uncontended FPS, physics cadence and startup acceptance remain open. Earlier
Tracy and contended measurements are diagnostics, not product acceptance.
See [the performance handover](open-400fps-performance-handover.md) for those
targets and owner findings. Historical evidence removed by target cleanup must
not be presented as currently inspectable.

All task-owned apps exited through the API and ports **4261–4264** were confirmed
released. There is no owned session to resume. Preserve sibling worktrees,
other live sessions, shared Cargo/sccache caches and authored Twin changes.
The next runtime worker must establish its own PID, executable, cwd and free
port before issuing commands. This handover itself adds documentation only.
