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
- Asset identity, traversal, and storage belong to `lunco-assets`; USD-related
  runtime crates do not call `std::fs` for asset bytes. They use the asset
  boundary, while `lunco-usd-compose` supplies USD dependency interpretation
  and OpenUSD assembly (sublayers, references, payloads, variants), re-exported
  by `lunco-usd`. A composed stage is inert:
  it must not launch Modelica, Rhai, behaviour trees, physics, or rendering.
  Those are separate downstream projections. A tutorial projects metadata from
  a supplied stage; it never opens layers itself.
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
  Build it in this worktree, then invoke `target/debug/luncosim` directly; do not
  use an old `sandbox` executable name or hide a rebuild behind `cargo run`.
  Full scene reload is supported; partial object/reference reload remains TODO.
- Establish a behaviour baseline before physics/vehicle changes and rerun it.
  Capture real exit codes and inspect verdicts.
- Use focused tests first, then production luncosim. Use `-j 4`, repository
  `target/`, and regular `sccache`; never use managed temporary build directories
  or custom temporary files. Avoid overlapping Cargo builds.

## Sandbox lifecycle

- Every controllable launch uses an explicit API port:
  `target/debug/luncosim --api 4101` (or another free port).
- Networking is opt-in (`cargo build -p lunco-luncosim --features networking`);
  normal local builds and scene tests must not start a multiplayer host.
- Reuse the existing session for asset, shader, and Rhai reloads through its API.
  When a replacement is required, stop the previous session through API `Exit`,
  verify its process and API port are gone, then launch the replacement. Never
  overlap GUI/API sessions or reuse a port owned by the previous process.
- Keep one production process while iterating. Use `/api/commands` for scene
  reloads, telemetry, screenshots, and tests. Use `ReloadShader` or `RunScenario`
  for live edits instead of relaunching.

## Packaged desktop builds

- For installed-package testing, download the platform installer from the
  dated releases in the main repository:
  `https://github.com/LunCoSim/lunco-sim/releases`. Use exactly one of
  `LunCoSim-Windows-x86_64-Setup.exe`,
  `LunCoSim-macOS-Apple-Silicon.pkg`, `LunCoSim-macOS-Intel.pkg`, or
  `LunCoSim-Linux-x86_64.AppImage`. Do not use a GitHub source archive, an
  Actions artifact, a raw archive, or a debug binary for packaged acceptance.
- `https://github.com/LunCoSim/lunco-sim-updates` is the machine-only Velopack
  repository. It contains runtime-specific feeds and full packages consumed by
  the updater; it is not the place to obtain the human installer.
- The normal Velopack locations are:
  - Windows Setup: `%LOCALAPPDATA%\LunCoSim-win-x64\`, with the live payload in
    `current\`; `Setup.exe --installto <dir>` may override the root.
  - macOS `.pkg`: `LunCoSim.app` in the location selected by Installer,
    normally `/Applications/LunCoSim.app` or `~/Applications/LunCoSim.app`.
  - Linux AppImage: no system installation; the AppImage remains at the path
    where the agent placed it and must be writable and relaunched from that
    same path for updates.
- The per-user asset cache is separate from the application install: Linux
  `~/.cache/lunco/`, macOS `~/Library/Caches/lunco/`, and Windows
  `%LOCALAPPDATA%\lunco\`. Do not mistake this cache for the installed app.
- A real updater test must launch the installed Windows shortcut, installed
  macOS app, or same writable Linux AppImage. Source builds and ordinary
  archives correctly report `NotInstalled` to the Velopack updater.

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
