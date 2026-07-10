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
- **Shipped lessons**: *sandbox* — Sandbox Intro → First Drive → Lander & Rover
  Mission, plus an authoring track (Build a Scene → Script a Rover → Inspect the
  Simulation → Cosim); *lunica* — a 7-lesson workbench course + the Welcome-panel
  [learning paths](../../assets/tutorials/learning_paths.json). The sandbox also
  embeds the full lunica modeling IDE as its **Design workspace** (open `.mo`,
  compile, run, plot) — the lunica lessons run there too, though they are
  currently registered under *lunica*; see the [sandbox app doc](../apps/sandbox/README.md).
- **Add one — data, not Rust**: drop `assets/tutorials/<app>/<name>.rhai` + an entry
  in `assets/tutorials/<app>/tutorials.json` (the app's `TutorialPlugin { app }`
  scans it). No rebuild. Full recipe in
  [`../../assets/tutorials/README.md`](../../assets/tutorials/README.md) and the
  [`author-tutorial`](../../skills/author-tutorial/SKILL.md) skill.

## Authoring walkthroughs

| Tutorial | What you build |
|---|---|
| [01 — Lander → Rover mission](01-lander-rover-mission.md) | A reusable lander *vehicle* that flies itself down on a glowing engine plume, a scene that drops it into a mission, and an autopilot that drives the released rover through a waypoint course until you take over — with model-driven warnings, on-screen narration, and possession as the one source of control authority. |
| [02 — Author your own controller](02-authoring-a-controller.md) | Build a self-flying vessel from scratch: the control law in Modelica, logic in rhai, sensors + wiring + the `piloted` authority signal in USD — and a pilot who can take over. The layering behind every LunCoSim GNC. |
| [03 — Cosim: when a Model flies physics](03-cosim.md) | How a Modelica model and the physics engine share a timestep: the lander's `modelicaModel` + `SimConnection` wiring, Modelica `when` events on the bus, and verifying the live chain over the API. |

Each walkthrough pairs with an in-app lesson and the reference **[skills](../../skills/README.md)**:

| Walkthrough | In-app lesson | Reference skills |
|---|---|---|
| 01 — Lander → Rover mission | *Lander & Rover Mission* (sandbox) | [build-usd-scene](../../skills/build-usd-scene/SKILL.md) · [author-scenario](../../skills/author-scenario/SKILL.md) · [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) |
| 02 — Author your own controller | *Script a Rover* (sandbox) | [authoring-vessel-controllers](../../skills/authoring-vessel-controllers/SKILL.md) |
| 03 — Cosim: when a Model flies physics | *Cosim — Model meets Physics* (sandbox) | [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) · [inspect-simulation](../../skills/inspect-simulation/SKILL.md) |

Looking for a reference rather than a walkthrough? The full script verb list is
in [`../scripting-guide.md`](../scripting-guide.md), the design behind scenarios is
in [`../architecture/34-scenario-and-multidomain.md`](../architecture/34-scenario-and-multidomain.md),
and every task skill is indexed in [`../../skills/README.md`](../../skills/README.md).
