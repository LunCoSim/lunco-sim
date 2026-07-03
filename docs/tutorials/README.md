# LunCoSim Tutorials

Two things share this name — don't confuse them:

- **In-app tutorials** — the interactive lessons that ship *inside* each app
  (the **🎓 Tutorials** menu / F1). Coach-mark tours that spotlight widgets and
  advance as you act. See [§ The in-app tutorial system](#the-in-app-tutorial-system).
- **Authoring walkthroughs** — these docs: build-something-real guides where you
  edit data files under `assets/` (`.usda` / `.mo` / `.rhai`), reload, and watch
  it work. See the table below.

## The in-app tutorial system

**One source, one launcher.** A tutorial is a single `.rhai` scenario; the shared
launcher (`crates/lunco-tutorial`) runs it on a host entity via `RunScenario`. The
scenario sets up its *own* environment in `on_start` — `load_scene("scenes/…")`
for a 3D lesson, `cmd("OpenClass", …)` for a modeling lesson — so there is **no
scene-vs-script split**. The coach card / spotlight / objectives come from the
shared HUD (`lunco-workbench::tutorial_overlay`) + the `hud.rhai` prelude.

- **Where they live**: `assets/tutorials/<app>/<name>.rhai` (`lunica/…`,
  `sandbox/…`). Native reads them fresh from disk each launch (edit → replay, no
  rebuild); wasm serves an embedded copy. Loader:
  `lunco_assets::tutorials::tutorial_source`.
- **Launch**: every entry point (🎓 menu, F1 via `EditorIntent::ShowTutorial`, the
  HTTP API, MCP, other scripts) funnels through one `StartTutorial{id}` command.
- **Onboarding is a policy, not Rust**: on a first interactive run, the boot hook
  (`assets/scripting/policy/boot.rhai`, id `boot.entry`) decides to show the
  onboarding tutorial instead of loading the default — one load, no race. Rewrite
  it (or hot-replace by id) to change startup behavior with no rebuild.
- **Add one**: drop `assets/tutorials/<app>/<name>.rhai` + a `register_tutorial(…)`
  row — full recipe in [`../../assets/tutorials/README.md`](../../assets/tutorials/README.md).

## Authoring walkthroughs

| Tutorial | What you build |
|---|---|
| [01 — Lander → Rover mission](01-lander-rover-mission.md) | A lander that flies itself down on a glowing engine plume, releases a rover, and an autopilot that drives the rover through a waypoint course until you take over — with model-driven warnings and on-screen narration throughout. |

Looking for a reference rather than a walkthrough? The full script verb list is
in [`../scripting-guide.md`](../scripting-guide.md), and the design behind
scenarios is in
[`../architecture/34-scenario-and-multidomain.md`](../architecture/34-scenario-and-multidomain.md).
