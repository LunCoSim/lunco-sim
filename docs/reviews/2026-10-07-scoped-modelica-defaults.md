# Declaration-scoped Modelica defaults

The runtime default collision is fixed at two owners. Rumoca DAE classification
retains external declaration bindings as initialization expressions at qualified
input paths, while internal and connected bindings remain equations. Compiler
and worker source admission preserve original declarations. Editor compile
dispatch retains runtime values and reads parameter metadata only from the
selected class; its document-wide index does not supply runtime input defaults.
Prepared solve cache version 6 excludes artifacts from a different initialization
contract. The CLI and browser host consume the same compiled initialization.

This is generic equation lowering and initialization, rather than a changeable
scenario policy; it belongs to the compiler/solver and does not require a hook.
The existing strict compiler and live/batch solvers are the real consumers.

Validation on optimization, 2026-10-07:

- Focused compiler declaration-scope test: PASS, including library admission,
  distinct class defaults, repeated instances, inheritance, internal bindings,
  and visible rejection of an unresolved default.
- Native worker checks and production build: PASS. Removal warnings were fixed.
- Windowed production Rhai gate: `TESTS_OK 20`,
  `MODELICA_SCOPED_INPUT_DEFAULTS: PASS` on owned port 4261. It observes qualified
  defaults, arrays, parameter/function expressions, integrated state, exact
  instance override, reset preserving the override, invalid-default diagnostics,
  and a valid successor batch trajectory.
- Skill catalogue validation: PASS, 43 skills.

Logs are under `target/perf/scoped-defaults-*`. Every owned process exited and
its API port was released. Earlier performance evidence is not a clean FPS
acceptance result; this focused change does not claim performance acceptance.

## Lunar-base-model acceptance remains blocked

The canonical Twin is
`/home/rod/Documents/models/lunar-base-model/twins/astrobotic-griffin-1`.
Its existing dirty authored work was preserved. The production default scene
opened in a windowed owned session on port 4264. `ReadActiveTwinContract` reported
no verification-registry or SysML source errors, and `RuntimeDiagnostics` reported
zero errors. Physics remained held with three readiness tickets: USD physics
admission, MainPropulsion_System, and AttitudePropulsion_System. The assemblies
compile successfully; their solver preparation did not complete in this run.

The landing-stability and engine-command manifest verifications, run on owned
ports 4262 and 4263, each exhausted the 120-second scene-readiness bound without
an authored verdict. Those runs preceded the final editor-dispatch cleanup;
the final default-scene run confirms the same pending propulsion preparations.
Reports are in `target/perf/griffin-{landing,engine}-scoped-defaults.log` and
`target/perf/griffin-default-scoped-defaults.json`. These are blockers, not passes.
Trace the two preparation jobs and repeat the authored gates after resolving
that owner boundary before declaring this Twin operational.

The defaults probe also exposed an independent eliminated aggregate-output
readback issue: its integrated state consumes the correct equation, but the
aggregate output alias reads zero. The current regression validates the
continuous state and qualified slots; alias reconstruction needs separate solver
coverage and correction. It does not establish full Twin output acceptance.

## Integration with current main

Main commit `804c54144` was merged into optimization after the defaults fix.
The conflict resolution retains its strict compiled-content identity for solver
cache reuse and removes runtime default maps from both cached rebuild paths.
The integrated production build passes, and the owned windowed defaults gate
passes `TESTS_OK 20` again. Its runtime schema regenerated the unchanged command
reference: 262 commands across 54 crates. Integrated logs and schema are under
`target/perf/scoped-defaults-merged-*`.

The bounded post-merge default-Twin recheck on port 4264 reports zero retained
runtime errors and a clear manifest/SysML contract. It remains held during cold
solver preparation; the final sampled readiness still included the propulsion,
actuation, and FLIP assemblies. Some preparations completed before Exit, but
physics readiness and mission acceptance were not reached in this bounded run.
Evidence is `target/perf/griffin-default-merged.{log,json}`. All owned ports are
released. Main and the Twin's authored files were not modified by this fix/merge.
