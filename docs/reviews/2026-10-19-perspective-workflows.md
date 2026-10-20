# Perspective composition and engineering workflows

Status: implemented perspective composition and workflow review. Production
acceptance and screenshot inspection cover the changes described below. The
broader UX recommendations are hypotheses for usability sessions with engineers;
they are not results from observed user research.

## Engineering tasks

| Engineer's request | Necessary context | Useful presentation | Success evidence |
|---|---|---|---|
| Assemble a system from reusable components | Exact Twin, source document, selected assembly | Structure, library, connections, 3D preview, properties | Saved authored topology and validation findings |
| Change a physical law or controller | Exact program, host component, parameters and units | Diagram/source, interfaces, diagnostics | Compiled source and a bounded test |
| Diagnose unexpected motion or an absent response | Selected entity, controlled entity, live scene and simulation time | Scene, ports, forces/joints, causal diagnostics | Trace from input through connection to observed output |
| Compare candidate designs | Named runs, model revision, parameter changes, units | Run list, plots, differences, tolerances | Reproducible comparison with provenance |
| Verify a requirement | Requirement, mapped test, evidence revision | Requirements, verdicts, linked signals and source | Fresh pass/fail evidence against explicit acceptance criteria |
| Operate or demonstrate a mission | Controlled target, camera target, time and readiness | Large scene with concise controls/status | Acknowledged control and visible outcomes |

These are recurring tasks to test with engineers, not a validated ranking of
personas or task frequency. Start usability sessions with the engineer's own
Twin and request, without naming the panel they should use.

## Implemented contracts

- `assets/scripting/policy/workbench_perspectives.rhai` assembles View, Build,
  Analyze, Editor, Lunica and the optional Terrain task from registered views.
  Build groups scene composition tools; Analyze gives signal comparison the
  central surface. Labels retain familiar names with explicit task descriptions.
- `lunco-workbench-core::PerspectiveLayoutPlan` already describes semantic
  slots and initial instance tabs. `lunco-workbench-layout` owns dock
  materialization, cached user arrangements and revision invalidation.
- `Panel::visible_in_perspective` permits independent panel packages to
  contribute through the same layout owner.
- `Panel::preferred_perspective` currently makes explicit panel focus navigate
  to its workflow. This is useful for intentional navigation, but ordinary
  selection or background completion must not unexpectedly switch tasks.
- The Twin manifest's `default_perspective` selects an admitted application or
  `twin/<id>` task. Restoration waits for composition; explicit user navigation
  takes precedence over a late restore, launch selection, personal state and
  authored default, in that order.
- Existing Rhai application/Twin policy manifests provide installation,
  replacement, reload and retirement. Retained runtime UI surfaces provide
  authored HUDs and panels, not a second dock layout engine.
- View/Perspectives and Help/Perspective help read the live registry, including
  Twin tasks. Optional analysis tools have a consistent View/Analyze group.
  The application guide, architecture, contributor skills and navigation
  tutorials describe the same five default tasks and source boundaries.

## Perspective contract

A perspective is a named task presentation over the same Twin and document
owners. It chooses useful views, initial arrangement, context bindings,
interaction intent and relevant actions. It must not create a separate model,
simulation clock, selection authority, experiment registry or save boundary.

Task defaults should distinguish scene composition, model engineering,
operation and analysis. Assembly editing is a focused document workflow;
Modelica, Rhai, SysML and USD are representations opened within tasks. Preserve
existing workflows while making their purpose visible; do not infer a new task
from every file extension or plugin.

A new top-level perspective needs a recurring task, a materially different
layout, identifiable context, a useful first action and a tested completion
path. Specialist Twin tasks may be discoverable in a picker without filling
the global title bar. Advanced tools remain available through menus.

## Rhai assembly

Reuse the existing hook registry and semantic layout plan. Rust supplies typed
capability facts and validates/applies the result; Rhai chooses panel
combinations, availability, labels and layout defaults. Application defaults
and Twin additions must use the same definition/validation/materialization
path. Do not retain a second Rust-authored Build/Analyze policy.

Expose registered singleton panels and instance kinds, stable ids and titles,
canonical instance identities, current Twin/document identity, admitted scene
state and capability revisions. A definition must identify its stable name,
purpose, slots, initial instance tabs, interaction intent and layout revision.
Reject unknown panels, duplicate identities, invalid slots and invalid
references atomically with an actionable owner diagnostic.

Evaluate on capability, policy, document or Twin changes, not every paint.
Use immutable facts and bounded asynchronous preparation. Capture the exact
policy implementation and Twin lease with the job, and validate both at
publication. UI work must not hold simulation ticks.

Dynamic behavior updates available task choices and context-sensitive content.
It must not move panels or steal focus on selection, solver completion or
incoming telemetry. Apply defaults on first activation, explicit reset, or an
explicitly accepted revision update; preserve a user's visited layout.

## Twin shipping and persistence

A Twin ships its Rhai policy through the existing indexed `scope = "twin"`
policy manifest. Definitions are scoped to its exact Twin identity. Keep
stable persisted names distinct from runtime lifetime leases. Twin closure
retires definitions, caches, pending work, help/menu contributions and bound
views before another Twin can reuse those names.

Personal panel placement belongs in existing per-Twin workspace storage.
Authored defaults belong to the Twin and travel with it. A user's rearrangement
must not dirty shared model files. Missing required capabilities produce a
visible definition error; optional capabilities are selected explicitly by the
policy from the supplied facts. Malformed custom UI must not corrupt or
crash the simulation.

Use the existing default-perspective field after definitions are admitted;
activation must resolve an exact registered identity. Explicit user task
selection takes precedence over a policy recommendation. Tutorial constraints
remain explicit and release through their existing lifecycle.

## Interface priorities

1. Keep Twin, document/source, selected object, controlled object, run and time
   visible wherever their ambiguity can change the meaning of an action.
2. Keep navigation spatially consistent: browse on the left, primary work in
   the center, properties on the right, diagnostics/output below when needed.
3. Give analysis named runs, units, comparison context and provenance, alongside
   plots. A curve alone is insufficient evidence for an engineering decision.
4. Use intent labels such as Add component, Connect, Validate, Run, Compare and
   Show source. Provide contextual Show in navigation across representations.
5. Keep menus stable and expose expert tools progressively. Opening an optional
   panel should explain any task switch. Provide reset and a reliable return.
6. Distinguish accepted, preparing, applied, rejected and failed operations in
   existing status owners. Show the affected subject and a next action.
7. Make common tasks keyboard accessible, preserve visible focus, avoid
   color-only verdicts, and test compact windows and display scaling.
8. Teach complete tasks in tutorials, including invalid input and recovery.
   A coach card must target an admitted visible view in the correct source.

## Acceptance

The windowed production runner passes both authored suites:

```sh
LUNCOSIM_BIN=target/debug/luncosim python3 scripts/api/test_workbench_perspectives.py --port 4297
LUNCOSIM_BIN=target/debug/luncosim python3 scripts/api/test_workbench_perspectives.py --port 4301 --scene-free
```

The first suite passes **20 checks**: actual graph bounds, uncluttered Build,
native close/reopen of the final central instance, semantic bottom-tool placement, idempotent task activation,
policy replacement, preservation of visited tools and central intent, explicit
reset, a shipped Twin task, invalid instance kind and layout-field diagnostics,
and Twin A → B → A with fresh identities and retired transient tools.
Closing and restored-layout filtering use native `egui_dock::DockState::remove_tab`
so subtree indices, focus and surfaces remain owned by the docking library.

The runner uses bounded per-phase deadlines and explicit unthrottled, non-vsync presentation for its owned window.

The second suite passes **8 checks** with no mounted USD payload. The real
navigation tour starts under its exact Twin, receives Next and Back while the
scene gate is closed, and retires on Twin replacement. A later navigation event
cannot recreate the outgoing tour. Application observation remains independent
of the scene. Scene-owned work still requires its committed generation.

Current logs and captures are in `target/workbench-perspectives/` and
`target/workbench-scene-free/`. Build, Analyze, the reopened graph, the Twin's
Power study and the scene-free coach card were visually inspected. These
fixtures prove task layout and lifecycle behavior; they do not demonstrate a
populated engineering comparison or validate 3D rendering. Both owned processes
exited and released ports 4297 and 4301. Concurrent builds in other checkouts
were left running; no performance claim is made from these captures.

The production binary builds successfully, and
`cargo check -p lunco-modelica-ui --bin lunica -j 4` passes for the standalone
caller. The saved-center serialization seam
and the negative scene-generation lifecycle seam have focused Rust coverage.
The command reference is regenerated from the live **270-command** schema:
`OpenTab` and `CloseTab` are added and all 268 commands documented on main
remain. Skill catalogue/link validation and Python harness compilation pass.
Standalone Lunica runtime, browser and packaged application acceptance, and
keyboard/accessibility conformance are outside this validation.

## Follow-up engineering requests

| Request | Owning contract | Acceptance |
|---|---|---|
| Make action context visible and navigable | Existing Twin/document/selection/control/run owners; shared presentation projection | Before Save, edit, control or comparison, engineers can identify the affected source, object and run without guessing |
| Make comparison evidence reproducible | Existing experiment, signal and units owners | Choose baseline and candidate, see revision and parameter differences, compare compatible units and save explicit tolerances |
| Turn diagnostics into recoverable tasks | Existing typed operation results and StatusBus | A rejected operation identifies its subject and offers a valid next action; retry reaches a terminal result |
| Verify efficient and accessible navigation | Shared commands, input bindings and theme | Keyboard-only task completion, visible focus, compact-window and scaling checks, and non-color-only verdicts |

Usability sessions should measure unaided task completion, wrong-context edits,
unexpected task switches, recovery and confidence in reported evidence. Establish
baselines before choosing improvement targets.

## Primary references

Recommendations above are our application-specific synthesis. The sources
support the interaction patterns; they do not prove usability of LunCoSim.

- [Nielsen Norman Group: usability heuristics](https://www.nngroup.com/articles/ten-usability-heuristics/):
  visible state, familiar terms, consistency, recognition, recovery and efficient
  expert use guide the interface priorities.
- [Eclipse: perspectives](https://help.eclipse.org/latest/topic/org.eclipse.platform.doc.isv/guide/workbench_perspectives.htm)
  and [editors/views](https://help.eclipse.org/latest/topic/org.eclipse.platform.doc.user/gettingStarted/qs-02b.htm):
  task arrangements and contextual Show in navigation can share editor owners.
- [ParaView: displaying data](https://docs.paraview.org/en/latest/UsersGuide/displayingData.html):
  linked visualizations, explicit active views and comparative displays support
  analysis across representations.
- [MathWorks: comparing simulation data](https://www.mathworks.com/help/simulink/ug/compare-simulation-data.html):
  explicit baseline/comparison runs, units and tolerances support engineering
  interpretation beyond plotting.
- [W3C: keyboard operation](https://www.w3.org/WAI/WCAG22/Understanding/keyboard.html),
  [target size](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html)
  and [status messages](https://www.w3.org/WAI/WCAG22/Understanding/status-messages.html):
  use these as interaction design references for the native UI; this review
  does not claim WCAG conformance.
