# LunCoSim Tutorials

Two things share this name — don't confuse them:

- **In-app tutorials** — the interactive lessons that ship *inside* each app
  (the **🎓 Tutorials** menu / F1). Coach-mark tours that spotlight widgets and
  advance as you act. See [§ The in-app tutorial system](#the-in-app-tutorial-system).
- **Authoring walkthroughs** — these docs: build-something-real guides where you
  edit data files under `assets/` (`.usda` / `.mo` / `.rhai`), reload, and watch
  it work. See the table below.

## The in-app tutorial system

**One catalog, one launcher.** A lesson is declared by a USD curriculum prim:
its script is `info:sourceAsset` and its optional world is a `payload`. The
shared launcher (`crates/lunco-tutorial`) mounts that world through the typed
scene lifecycle, waits for the completion edge, and only then starts the script
on a host entity. The coach card / spotlight / objectives come from the shared
HUD (`lunco-workbench::tutorial_overlay`) + the `hud.rhai` prelude.

- **Where they live**: `assets/tutorials/<app>/<name>.rhai` (`lunica/…`,
  `luncosim/…`). Native reads them fresh from disk each launch (edit → replay, no
  rebuild); wasm serves an embedded copy. Loader:
  `lunco_assets::tutorials::tutorial_source`.
- **Launch**: every entry point (🎓 menu, F1 via `EditorIntent::ShowTutorial`, the
  HTTP API, MCP, other scripts) funnels through one `StartTutorial{id}` command.
- **Onboarding is a policy, not Rust**: on a first interactive run, the boot hook
  (`assets/scripting/policy/boot.rhai`, id `boot.entry`) decides to show the
  onboarding tutorial instead of loading the default — one load, no race. Rewrite
  it (or hot-replace by id) to change startup behavior with no rebuild.
- **Shipped lessons** span the LunCoSim, Basic rover, Sandbox, and Lunica tracks. Each lesson authors `lunco:tutorial:format`: a **tour** is guided reference content whose Rhai policy may require coach navigation or a documented user action; an **exercise** may complete only from observed simulator objectives. A source-level curriculum gate rejects exercise scripts that advance from `cmd:TutorialNext`, and production scene gates cover the runtime mechanics. The Welcome-panel [learning paths](../../assets/tutorials/learning_paths.json) remain a separate navigation aid.
- **The catalog is a USD layer**: a TRACK is a prim applying `LunCoTutorialTrackAPI` in `assets/tutorials/<track>/curriculum.usda`; each child applying `LunCoTutorialAPI` is a LESSON, whose script is `info:sourceAsset` and whose world is a `payload` arc. An APP offers tracks by sublayering them from `assets/tutorials/<app>.usda` — that layer stack is the whole answer to "which tracks, in what order".
- **A lesson's world is DECLARED**: the launcher mounts the `payload` through the scene lifecycle before running the script. A lesson with no payload deliberately leaves the viewport alone — absent is a statement, not a missing value.
- **Presentation is authored**: a track may set `lunco:track:perspective` to the identifier registered by the host. The launcher resolves it through the normal perspective registry; there is no app-specific tutorial hook, and an unknown identifier fails the launch.
- **Dynamic Twin-scoped lessons**: a Twin contributes on exactly the same terms — one `sim/tutorials/curriculum.usda` (the *Space School Seminar* track, SS1–SS4), composed when the Twin opens and dropped when it closes. No twin-specific manifest, no second parse.
- **Add one — data, not Rust**: drop `assets/tutorials/<track>/<name>.rhai` and declare a prim for it in that track's `curriculum.usda`. No rebuild. Full recipe in [`../../assets/tutorials/README.md`](../../assets/tutorials/README.md) and the [`author-tutorial`](../../skills/author-tutorial/SKILL.md) skill.

### Runtime tutorial tests

The test is an authored Rhai observer attached to a production scene fixture:

| Asset | Responsibility |
|---|---|
| `assets/scenes/tests/<name>.usda` | Composed world and lesson program |
| `assets/scenarios/tests/<name>.rhai` | Public command/event observation and verdict |

The observer must check the behavior being taught — for example, command
events, live movement or ports, and the final objective — rather than merely
waiting for `MISSION_COMPLETE`. It must not provide a second control path.
Shared assertions and `report_verdict(...)` come from
`assets/scripting/prelude/auto_tests.rhai`.

Run a gate directly after editing Rhai; it uses the already-built production
binary and does not require a Rust rebuild:

```bash
target/debug/luncosim test \
  --scene scenes/tests/tutorial_first_drive.usda --max-ticks 6000
```

`target/debug/luncosim --validate` is only parse/preflight evidence. Generic
Rust tests may protect the scripting/lifecycle seam, but lesson-specific steps,
required events, and command counts belong in Rhai runtime observers.

## Authoring walkthroughs

| Tutorial | What you build |
|---|---|
| [01 — Lander → Rover mission](01-lander-rover-mission.md) | A reusable lander *vehicle* that flies itself down on a glowing engine plume, a scene that drops it into a mission, and an autopilot that drives the released rover through a waypoint course until you take over — with model-driven warnings, on-screen narration, and possession as the one source of control authority. |
| [02 — Author your own controller](02-authoring-a-controller.md) | Build a self-flying vessel from scratch: the control law in Modelica, logic in rhai, sensors + wiring + the `piloted` authority signal in USD — and a pilot who can take over. The layering behind every LunCoSim GNC. |
| [03 — Cosim: when a Model flies physics](03-cosim.md) | How a Modelica program and the physics engine exchange typed values at declared communication points, how USD connections become `SimConnection`s, and how to verify the live chain over the API. |

Each walkthrough pairs with an in-app lesson and the reference **[skills](../../skills/README.md)**:

| Walkthrough | In-app lesson | Reference skills |
|---|---|---|
| 01 — Lander → Rover mission | *Lander & Rover Mission* (luncosim) | [build-usd-scene](../../skills/build-usd-scene/SKILL.md) · [author-scenario](../../skills/author-scenario/SKILL.md) · [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) |
| 02 — Author your own controller | *Script a Rover* (luncosim) | [authoring-vessel-controllers](../../skills/authoring-vessel-controllers/SKILL.md) |
| 03 — Cosim: when a Model flies physics | *Cosim — Model meets Physics* (luncosim) | [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) · [inspect-simulation](../../skills/inspect-simulation/SKILL.md) |

Looking for a reference rather than a walkthrough? The full script verb list is
in [`../scripting-guide.md`](../scripting-guide.md), the design behind scenarios is
in [`../architecture/34-scenario-and-multidomain.md`](../architecture/34-scenario-and-multidomain.md),
and every task skill is indexed in [`../../skills/README.md`](../../skills/README.md).
