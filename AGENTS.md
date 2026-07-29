# LunCoSim agent guide

Compact operating contract. Read `skills/README.md`, `docs/crates-index.md`,
`docs/principles.md`, the architecture index, and the relevant open review first.

## Design

- Read owning source before accepting a bug claim; comments describe intent.
- Check OpenUSD, Modelica, Avian, Bevy, or a maintained crate before adding a
  schema, resolver, field, or duplicate mechanism.
- No legacy paths, shims, aliases, duplicate spellings, or writes without readers.
  Replace the old mechanism and remove its traces in the same change. Do not
  retain a fallback, compatibility branch, or migration path that preserves an
  incorrect former behaviour. A default is permitted only when it is the
  documented semantic default of the authoritative input (for example, an
  omitted USD attribute), is exercised by a reader, and does not alter an
  explicit authored value.
- Use composed USD reads for runtime behaviour and authored-layer reads for
  authoring/document questions. USD owns scene facts and standard fields such as
  `doc`, `metersPerUnit`, `UsdShade`, and `UsdPhysics`.
- Modelica owns continuous equations/state; behaviour trees own sequencing; Rhai
  owns scenario glue/policy; Rust owns engine mechanisms, kinematics, dynamics,
  and hot paths. Production Rhai must not use `on_tick` except for test verdicts.
- A movable mounted part needs both a rigid body and a joint; internal geometry
  must not become an unconnected body. The `nested-body-no-joint` lint guards this.
- Generated USD schema files come from `scripts/gen_schema.py`; edit source schema
  and regenerate. Update `docs/crates-index.md` for crate changes.

## Tests and runtime

- Test scenes are under `assets/scenes/tests/`, scenarios under
  `assets/scenarios/tests/`. A green gate needs a negative fixture and a real verdict.
- Use only the production `luncosim` binary for scene tests and visual validation.
  Full scene reload is supported; partial object/reference reload remains TODO.
- Establish a behaviour baseline before physics/vehicle changes and rerun it.
  Capture real exit codes and inspect verdicts.
- Use focused tests first, then production luncosim. Use `-j 4`, repository
  `target/`, and regular `sccache`; never use managed temporary build directories
  or custom temporary files. Avoid overlapping Cargo builds.

## Sandbox lifecycle

- Every controllable launch uses an explicit API port:
  `target/debug/luncosim --api 4101` (or another free port).
- Reuse the existing session for asset, shader, and Rhai reloads through its API.
  When a replacement is required, stop the previous session through API `Exit`,
  verify its process and API port are gone, then launch the replacement. Never
  overlap GUI/API sessions or reuse a port owned by the previous process.
- Keep one production process while iterating. Use `/api/commands` for scene
  reloads, telemetry, screenshots, and tests. Use `ReloadShader` or `RunScenario`
  for live edits instead of relaunching.

## Performance, UI, and persistence

- Per-frame work is only for continuous rendering, physics, animation, and input.
  Otherwise use observers, asset events, change detection, revisions, or hashes.
- UI colours, spacing, and rounding come from `lunco-theme`; UI dispatches typed
  commands and does not mutate domain state directly.
- Heavy parsing, baking, mesh generation, and I/O must not block the UI thread.
- Persist through `lunco-settings` and `lunco-storage`. Runtime persistence loads
  are off by default and saves are independent; corrupt optional state cannot stop
  authored scene loading. Do not use JSON for internal change detection.

## Handoff

- Search with `rg` and exclude `target/`.
- Run `git diff --check`, focused formatting/checks, and relevant runtime tests.
- Fix warnings introduced by the change. Report exact tests, runtime/API checks,
  luncosim port/session state, and remaining blockers. Never claim unobserved results.
