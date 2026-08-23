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

## Change review and documentation

- After every architectural change, update the canonical documentation and every
  affected skill in the same change. Keep owner, API, lifecycle, and verification
  guidance current; remove obsolete instructions instead of documenting both
  generations.
- Before handoff, review the complete diff for legacy code, shims, fallbacks,
  compatibility branches, duplicated logic, stale comments, and stale docs.
  Update all APIs and call sites together. If a required capability is missing,
  implement it at its authoritative owner or report the blocker explicitly.

## Coding guide

- Fix the root cause. Do not add a shim, alias, compatibility branch, silent
  fallback, or alternate path to hide missing or invalid behavior. A default is
  allowed only when it is the documented semantic default of an omitted input.
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
  verdict. Tutorial behavior tests belong in authored Rhai scenarios; Rust tests
  remain generic to the scripting/lifecycle seam so tutorial edits do not require
  a core rebuild.
- Build and invoke only the production binary `target/debug/luncosim` for scene
  tests and visual validation. Do not use an old `sandbox` executable or hide a
  rebuild behind `cargo run`. `--validate` proves preflight only, not runtime
  behavior. Capture real exit codes and inspect authored verdicts.
- Establish a behavior baseline before physics/vehicle changes and rerun it.
  Use focused tests first, then production luncosim. Use `-j 4`, the repository
  `target/`, and regular `sccache`; never use managed temporary build directories
  or custom temporary files, and avoid overlapping Cargo builds.
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
