# LunCoSim agent guide

Compact operating contract. Read `skills/README.md`, `docs/crates-index.md`,
`docs/principles.md`, the architecture index, and the relevant open review first.
If a nested `AGENTS.md` applies, follow the most specific guide and reconcile
it with this one.

## Architecture

- Read the owning source before accepting a bug claim. Check OpenUSD, Modelica,
  Avian, Bevy, or a maintained crate before adding a schema, resolver, field, or
  duplicate mechanism. When renaming/removing an API, crate, type, or binary,
  search all docs and callers and update/remove old references in the same change.
- Apply DRY as a required discovery step: before adding a feature, helper, API,
  parser, setting, or library dependency, search the repository and maintained
  dependencies for an existing capability and its authoritative owner. Reuse
  that mechanism or extend it minimally; add a new mechanism only when no
  suitable owner exists, and record the reason in the change review.
- Do not reinvent the wheel. First check whether an existing mechanism can cover
  the feature with a small extension or composition. Add a new feature or
  refactor only when it is necessary and the outcome is materially better.
- Use the current authoritative contract. Invalid state must fail visibly at its
  owner.
- Keep one owner and one reader path. Review every change for DRY violations,
  unnecessary hardcoding, writes without readers, misplaced policy, and APIs that
  preserve an obsolete contract. Policy and tutorial-specific assertions belong
  in Rhai when the architecture permits; Rust owns shared engine mechanisms,
  kinematics, dynamics, and hot paths.
- Every new mechanism must identify its authoritative owner, real consumer, and
  production-level test.
- USD owns scene facts and standard fields (`doc`, `metersPerUnit`, `UsdShade`,
  `UsdPhysics`). Use composed USD reads for runtime behavior and authored-layer
  reads for authoring/document questions. A composed stage is inert: it does not
  launch Modelica, Rhai, behavior trees, physics, or rendering. Tutorials read a
  supplied stage and never open layers themselves.
- Asset identity, traversal, and storage belong to `lunco-assets`; USD runtime
  crates do not read asset bytes with `std::fs`. `lunco-usd-compose` owns USD
  dependency interpretation and assembly (sublayers, references, payloads,
  variants), re-exported by `lunco-usd`.
- Modelica owns continuous equations/state; behavior trees own sequencing; Rhai
  owns scenario glue/policy; Rust owns engine mechanisms. Production Rhai must
  not use `on_tick` except for test verdicts. Prefer events to polling.
- Assembly tools follow the same ownership rule: Rhai chooses component/socket/
  joint identities and builds explicit plans from composed USD queries; Rust
  validates generic USD topology and commits the plan through the existing
  compound journal boundary. Relationships come from explicit authored
  metadata, and no Rust-side default may hide missing metadata.
- A movable mounted part needs both a rigid body and a joint; hierarchy is not
  attachment. The `nested-body-no-joint` lint guards this.
- Edit schema sources and regenerate with `scripts/gen_schema.py`; update
  `docs/crates-index.md` for crate changes. Persist user preferences through
  `lunco-settings`/`lunco-storage`, not new per-feature files. Do not use JSON
  for internal change detection. Hardcoded tunable values belong in the owning
  settings/resource/component; UI tokens come from `lunco-theme`.
- Twin-scoped timelines, tool libraries, tutorial state, status, and handles
  must be wound down on `TwinClosed` before another Twin can use those names.
  Never keep name-only global state that can run or display a previous Twin.
- Tutorial controls use the controller-owned `input_bindings` settings through
  Rhai `input_binding(...)`/`input_hint(...)`; progression uses semantic commands
  or authoritative state, never raw physical key names.
- Tutorial scene payloads must choose their lighting/time contract in USD:
  fixed authored `DistantLight` with no celestial opt-in, or an explicit
  `LunCoEpochAPI`/`lunco:time:epochJd` plus the celestial payload. The authored
  `epoch-api-missing-time` lint rejects implicit orbital time; do not repair it
  in a script or with a runtime timing workaround.

## Change review and documentation

- After every architectural change, update the canonical documentation and every
  affected skill in the same change. Keep owner, API, lifecycle, and verification
  guidance current; remove obsolete instructions instead of documenting both
  generations.
- Before handoff, review the complete diff for legacy code, shims, fallbacks,
  compatibility branches, duplicated logic, stale comments, and stale docs.
  Update all APIs and call sites together. If a required capability is missing,
  implement it at its authoritative owner or report the blocker explicitly.
- Comments must describe the code as it stands. Do not describe discarded
  approaches, previous solutions, or missing capabilities.
- A replacement is a clean cutover: delete the retired implementation, API,
  caller, fallback, shim, and documentation/test coverage for the old contract
  in the same change. Never add a compatibility alias or a test that preserves
  obsolete behavior, and do not carry the retired path forward in migration or
  history documentation as an alternate implementation.
- API replacement is complete only when every caller, public export, example,
  fixture, test, comment, runbook, and history entry uses the new contract.
  Remove compatibility shims, aliases, fallbacks, old-behavior tests, and
  migration/history descriptions of the retired behavior; never preserve both
  generations behind a conditional or fallback path.

## Coding guide

- Fix the root cause. Do not add a shim, alias, compatibility branch, silent
  fallback, or alternate path to hide missing or invalid behavior. A default is
  allowed only when it is the documented semantic default of an omitted input.
- When changing an interface, update every caller and its tests to the new
  contract, then remove tests for the retired contract; tests must validate only
  the authoritative behavior that should remain.
- Keep Rust lightweight and policy out of the core when Rhai or an existing
  owner can express it. Do not duplicate an API, parser, setting, or assertion;
  update the authoritative interface and all callers together.

## Style guide

- Write short, precise responses: lead with the result, cite exact evidence, and
  state blockers without filler or speculation.
- Keep one canonical explanation per topic. Comments and runbooks explain
  design intent and verification, not retired behavior or duplicated mechanics.

## Tests and runtime

- Test scenes live under `assets/scenes/tests/`, scenarios under
  `assets/scenarios/tests/`. A green gate needs a negative fixture and a real
  verdict. Prefer authored Rhai for acceptance and regression tests whenever
  the public command/query surface can observe the contract; exercise it through
  the production scene-test binary, including negative cases. Keep Rust tests
  minimal and generic: pure lowering/math, serialization, and interpreter or
  lifecycle seams that Rhai cannot observe. Do not duplicate an observable
  runtime assertion in Rust merely because the implementation is Rust; tutorial
  behavior tests belong in authored Rhai so tutorial edits do not require a core
  rebuild.
- Build and invoke only the production binary `target/debug/luncosim` for scene
  tests and visual validation. Do not use an old `sandbox` executable or hide a
  rebuild behind `cargo run`. `--validate` proves preflight only, not runtime
  behavior. Capture real exit codes and inspect authored verdicts.
- Targeted checks are the default for a focused change: format only touched Rust
  files with the repository toolchain (or the directly affected package when a
  package-level check is required), and run the narrowest relevant
  `cargo check`/`cargo test` target. Do not run `cargo fmt --all`, a workspace
  format, or a full suite unless the change spans the workspace or that broader
  scope is explicitly requested.
- Do not run Rust formatters during the edit, debug, or test loop. Run the
  repository formatter once, only after implementation and validation are
  complete and immediately before the final diff review and commit.
- Establish a behavior baseline before physics/vehicle changes and rerun it.
  Use focused tests first, then production luncosim. Use `-j 4`, the repository
  `target/`, and regular `sccache`; never use managed temporary build directories
  or custom temporary files, and avoid overlapping Cargo builds. Minimize
  unnecessary test runs: reuse a valid result when the tested code and inputs
  have not changed, do not duplicate equivalent unit/scene coverage, and run
  the full expensive suite only after a meaningful integration change. Repeat a
  test only when its inputs changed, the previous run was invalidated (for
  example by a clean rebuild), or nondeterminism needs confirmation.
- Documentation, `AGENTS.md`, comment-only, and other minor non-behavioral edits
  do not invalidate a passing test result. Reuse the existing evidence and do
  not restart or repeat tests unless the tested code or inputs changed.
- Choose the smallest targeted check or test that covers the changed owner;
  crate-wide and workspace-wide suites are slow and should be reserved for
  changes that cross those broader boundaries.
- During implementation, use a single smallest sufficient validation command
  for the current change: prefer `cargo check` for compile/API-only edits and
  one named test or authored scenario for a behavior change. Do not run several
  overlapping package suites, repeat a passing command after unchanged inputs,
  or run a production build merely to replace a focused compile check. Before
  commit, run one bounded integration pass that combines only the changed
  owners' tests and the required production/runtime evidence; expand beyond
  that set only when the diff crosses the additional owner or a failure gives
  concrete reason. The pre-commit pass is broader than the edit loop, but it is
  still minimal and must not default to the workspace-wide suite.
- For CPU performance profiling, use the adjacent `../tracy` checkout: build
  the production binary with its opt-in `tracy` feature, start
  `../tracy/capture/build/tracy-capture` before the app, and inspect the
  resulting capture in Tracy. Keep a separate clean run for FPS acceptance;
  profiler overhead is diagnostic, not the product-performance number.
- Full scene reload is supported; partial object/reference reload remains TODO.

## Session lifecycle

- Every controllable launch uses an explicit free API port, for example
  `target/debug/luncosim --api 4101`. Networking is opt-in:
  `cargo build -p lunco-luncosim --features networking`; local builds and scene
  tests must not start a multiplayer host.
- Reuse one production session for asset, shader, and Rhai reloads through its
  API. Before replacement, send API `Exit`, verify the process and port are gone,
  then launch the replacement. Never overlap sessions or reuse an owned port.
  Never use `pkill`. Use `/api/commands` for reloads, telemetry, screenshots,
  and tests; use `ReloadShader` or `RunScenario` for live edits.

## UI, performance, and persistence

- Per-frame work is only for continuous rendering, physics, animation, and input.
  Otherwise use observers, asset events, change detection, revisions, or hashes.
  Heavy parsing, baking, mesh generation, and I/O must not block the UI thread.
- UI dispatches typed commands and does not mutate domain state directly. UI
  colors, spacing, and rounding come from `lunco-theme`.
- Runtime persistence loads are off by default and saves are independent; corrupt
  optional state must not prevent authored scene loading.

## Packaged desktop builds

- For installed-package testing, use exactly one dated installer from
  `https://github.com/LunCoSim/lunco-sim/releases`:
  `LunCoSim-Windows-x86_64-Setup.exe`,
  `LunCoSim-macOS-Apple-Silicon.pkg`, `LunCoSim-macOS-Intel.pkg`, or
  `LunCoSim-Linux-x86_64.AppImage`. Do not use source archives, Actions
  artifacts, raw archives, or debug binaries for packaged acceptance.
- `https://github.com/LunCoSim/lunco-sim-updates` is the machine-only Velopack
  feed, not the human installer source. Windows installs under
  `%LOCALAPPDATA%\LunCoSim-win-x64\` (`current\` is the live payload; Setup.exe
  accepts `--installto <dir>`). macOS installs `LunCoSim.app` where Installer
  selects, normally `/Applications` or `~/Applications`. Linux AppImage has no
  system install: keep it writable and relaunch it from its placed path.
- The user asset cache is separate from the app: Linux `~/.cache/lunco/`, macOS
  `~/Library/Caches/lunco/`, Windows `%LOCALAPPDATA%\lunco\`. A real updater
  test launches the installed app/shortcut or the same writable AppImage;
  source builds and ordinary archives correctly report `NotInstalled`.

## Handoff

- Search with `rg` and exclude `target/`. Run `git diff --check`, focused format/
  checks, and relevant runtime tests. Fix warnings introduced by the change.
  Report exact tests, runtime/API checks, session/port state, and remaining
  blockers; never claim unobserved results.
