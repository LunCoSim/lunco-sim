# Tutorials — one source, one launcher

A lesson is declared by one curriculum prim: its `.rhai` script is the lesson
logic and its optional USD `payload` is the lesson world. The shared launcher
mounts that payload through the typed scene lifecycle, waits for completion,
then starts the script on a host entity. The coach card / spotlight / objectives
come from the shared HUD + the rhai prelude.

The boundary is intentional: Rust owns the tutorial lifecycle (curriculum
composition, authored perspective, payload mounting, host lifetime, event
attribution, and the Continue/Stay here chain dialog). Rhai owns lesson policy
only (coaching, objectives, action predicates, and semantic event emission).
Rhai does not open scenes, choose perspectives, or start another lesson; it
uses the small prelude surface exposed for those lesson-level concerns.

## Current input labels

The controller owns the resolved user keymap in the persisted
`input_bindings` section of `<OS config dir>/lunco/settings.json`. Tutorial copy must read
semantic labels from that resource instead of spelling physical keys:

```rhai
hint("Drive with " + input_hint("forward") + " and " + input_hint("brake") + ".")
```

`input_binding("forward")` is the raw lookup: it returns the current user-facing
label or `()` when the intent is unbound. Use it when a lesson needs to inspect
or branch on binding state. `input_hint("forward")` is presentation sugar for
tutorial copy: it converts an unbound result into the explicit text `unbound`.
It does not choose a key or provide a control path. Lesson progression should
listen for semantic command events or authoritative state, never for a raw key
event. The bundled defaults are in `assets/config/keybindings.json`.

## Choose the lesson world's time contract

Each 3D lesson payload must choose one of these contracts. The choice belongs in
the payload USD, not in the tutorial script or Rust launcher:

| Contract | USD authoring | Use when |
|---|---|---|
| Fixed light | Reference `lunco://lighting/sun.usda` as a `DistantLight` and author its rotation. Do not apply `LunCoEpochAPI` or reference `lunco://celestial/solar_system.usda`. | The lesson teaches the UI, scene building, possession, or simple controls. The light is stable scenery. |
| Explicit ephemeris | Apply `LunCoEpochAPI`, author `double lunco:time:epochJd = …`, and reference the standard `SolarSystem`; add the root site anchor when the lesson uses lunar geodetics or Earth tracking. | The lesson teaches celestial time, real sun direction, Earth tracking, orbital motion, or another astronomy-dependent result. |
| Reuse an existing world | Reference/payload the scene that already owns the environment and time contract. | The lesson changes policy or behaviour, not the world. |
| No world | Omit `payload` from the curriculum prim. | The lesson is a UI/workbench tour and should preserve the currently open scene. |

The default epoch on `LunCoEpochAPI` is a compatibility default, not an
acceptable tutorial choice. The authored USD lint rule
`epoch-api-missing-time` rejects an applied epoch API without
`lunco:time:epochJd`; fixed-light payloads remain valid because they omit the
celestial opt-in entirely. This prevents a lesson from starting lit and then
changing to an unintended ephemeris direction.

The current corpus follows this split: `sandbox/first_drive.usda`,
`sandbox/sandbox_intro.usda`, and `sandbox/build_base.usda` are fixed-light
worlds; the luncosim Welcome/Controls lessons reuse `first_drive.usda`;
`basic/driving_basics.usda` and `basic/slope_test.usda` author an explicit
epoch and solar system; `basic/rover_variants.usda` reuses the former; the
lander/cosim lessons reuse `scenes/luncosim/lander_ops.usda`; and the lunica
lessons are UI/workbench lessons without a 3D payload.

## Layout

```
assets/tutorials/
  learning_paths.json        # the Welcome-panel MSL curriculum (separate feature)
  sandbox.usda               # the SANDBOX app: which tracks it offers (sublayers)
  lunica.usda  luncosim.usda # ditto, per app
  sandbox/                   # a TRACK
    curriculum.usda          #   its lessons, as prims — the catalog
    sandbox_intro.rhai
    first_drive.rhai   first_drive.usda      # env `.usda` co-located, declared as a payload
    lander_mission.rhai
  basic/                     # a track offered BY sandbox, not named after it
    curriculum.usda  b1_driving_basics.rhai  …
  perspectives/               # luncosim UI tour; no world payload
    curriculum.usda  overview.rhai
  lunica/                    # lunica (Modelica workbench) lessons
    curriculum.usda  overview.rhai  run.rhai  …
```

A **track** is a `curriculum.usda`. An **app** is a layer that sublayers the
tracks it offers — that layer is the whole answer to "which tracks does this app
show, in what order", and it is why `basic` appears under sandbox without any
track declaring `hosts = ["sandbox"]`. A **twin** contributes by composing its
own `sim/tutorials/curriculum.usda`, on the same terms as the engine.

The `perspectives` track is offered by `luncosim` and contains a UI-only tour of
`◉ View`, `⚒ Build`, and `∿ Lunica`. It intentionally omits both a world
payload and `lunco:track:perspective`, because the learner must be free to
switch among all three layouts during the lesson.

## Tutorial copy and flow

Use one card for one idea. Start with the action in imperative language, then
explain what the learner should notice and why it matters. Keep the body to one
or two short paragraphs; use `\n\n` between paragraphs instead of one dense
block. Name the exact menu, panel, tab, or button, and say what success looks
like. Use the resolved `input_hint(...)` label for controls.

Introduce only the concepts needed for the next action. Let a learner skip or
go back, keep the current objective visible, and move from explanation to a
real action as soon as possible. Tours may use Next for reference content;
exercises must advance from an observable user action or authoritative state.
Keep the first-run path short, and leave deeper reference tours available from
the Tutorials menu.

## Add a tutorial (two steps)

1. Drop `tutorials/<track>/<name>.rhai`. Author it with the prelude verbs:
   - `coach_step(steps, i)` + the `on_event` cursor — a guided coach-mark tour.
   - `hint(...)`, `spotlight(anchor, caption)`, `notify_kind(...)` — HUD.
   - `mission(me)` with `objective(...)` — auto-published objectives that advance
     on real actions (`requires_event`, optional `requires_event_source`, and
     `done` predicates); emits `MISSION_COMPLETE`.
   - Setup: `cmd("OpenClass", #{ qualified })`, `set_subsystem(name, on)`.
   - **No scene-opening call** — the world is declared, see step 2.

2. Declare it in `tutorials/<track>/curriculum.usda` — **data, not Rust**:

   ```usda
   def Scope "MyLesson" (
       prepend apiSchemas = ["LunCoProgramAPI", "LunCoTutorialAPI"]
       prepend payload = @lunco://tutorials/sandbox/my_lesson.usda@   # omit = no world
   )
   {
       uniform asset info:sourceAsset = @lunco://tutorials/sandbox/my_lesson.rhai@
       string lunco:tutorial:title = "My Lesson"
       string lunco:tutorial:blurb = "…"
       uniform token lunco:tutorial:difficulty = "beginner"
       uniform token lunco:tutorial:format = "exercise" # or "tour"
       uniform bool lunco:tutorial:firstStart = false   # true = onboarding entry
       rel lunco:tutorial:next = </Sandbox/NextLesson>  # omit to end the chain
   }
   ```

   A track may also author its required workbench presentation once:

   ```usda
   string lunco:track:perspective = "rover_build"
   ```

   The host resolves that identifier through its normal perspective registry.
   Omit the property to keep the host's current presentation; do not encode an
   app or lesson name in Rust.

   The lesson's id is its PRIM PATH, so the chain is a real relationship. The
   launcher mounts the `payload` through `LoadScene` **before** running the
   script. A lesson with no payload is intentionally UI-only; when it follows
   a world lesson, the launcher clears the outgoing scene before running it.

   Asset paths are **scheme-qualified** (`lunco://`, `twin://`) — a bare path is
   ambiguous once a Twin is open; see
   `docs/architecture/55-scene-addressing-and-roots.md`.

That's it — **no rebuild, no Rust**. On native the curriculum *and* the script
are read fresh from disk — edit and relaunch; on wasm scripts are embedded at
build time. `StartTutorial{id}` mounts the world and runs the script on the host.

## Test tutorial behavior without rebuilding Rust

Runtime tutorial checks are authored Rhai scenarios. Put the fixture in
`assets/scenes/tests/<name>.usda` and its observer in
`assets/scenarios/tests/<name>.rhai`. The observer must use the same public
commands and events as a learner, verify a real state change, and finish with
`report_verdict(...)`; it must not reproduce the lesson's control policy.

```bash
target/debug/luncosim test \
  --scene scenes/tests/tutorial_first_drive.usda --max-ticks 6000
```

The powered lander mission is a longer gate; use `--max-ticks 30000` for
`scenes/tests/tutorial_lander_mission.usda` so the descent, rover deployment,
and all waypoint objectives have time to complete.

Edit the lesson or its observer, then rerun the production binary. No core
rebuild is needed for Rhai edits. Use `--validate` only for syntax/preflight;
it is not a runtime tutorial test. Keep Rust tests generic to the scripting or
scene lifecycle boundary, never as a per-lesson list of steps or commands.

## Anchors (for `spotlight` / `coach_step` focus)

Spotlight a workbench widget by its `HelpAnchors` key; `focus` opens the panel
first. lunica panel ids: `lunco.workbench.twin_browser`, `modelica_welcome`,
`modelica_experiments`, `modelica_inspector`, `modelica_diagnostics`,
`modelica_console`, `modelica_journal`, `modelica_component_palette`,
`modelica_diagram_inspector`; model-view anchors `model_view.view_toggles` /
`model_view.compile_buttons` (need a model open). `panel.modelica_plot` is an
instance panel — spotlight its anchor, but don't `focus` it. See `lunica/README.md`.

For luncosim, use the current generic dock anchors: `panel.center`,
`panel.side_browser`, `panel.right_inspector`, and `panel.bottom`; use the real
panel id with `focus` when a lesson needs to open a tab. Menu and toolbar
controls can use their published anchors: `menu.time`, `menu.network`,
`menu.help`, and `toolbar.run`. Perspective tabs use
`menu.perspective.<registered-perspective-id>`, for example
`menu.perspective.rover_build`. Do not invent `panel.<instance>` anchors.

An authored non-empty anchor is required to resolve to a visible widget. The
overlay keeps an empty viewport's normal presentation visible instead of
covering it with a scrim; named anchors are still ringed. While
a lesson is active, its track's authored perspective is temporarily required,
so switching to another perspective automatically returns to the lesson's
presentation before the next card is painted. A missing anchor still fails
when the required perspective itself cannot publish it; that is an authored
UI contract problem, not a user navigation problem. An empty anchor is the
explicit choice for a centred card. Headless Rhai tests verify lesson policy
and state, so they do not require an interactive focus panel.
