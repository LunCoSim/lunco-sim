# Tutorials — one source, one launcher

A tutorial is **one thing: a `.rhai` scenario**. There is no scene-vs-script
split. The shared launcher (`crates/lunco-tutorial`) runs it on a host entity via
`RunScenario`; the scenario sets up whatever it needs in `on_start`. The coach
card / spotlight / objectives come from the shared HUD + the rhai prelude.

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
  lunica/                    # lunica (Modelica workbench) lessons
    curriculum.usda  overview.rhai  run.rhai  …
```

A **track** is a `curriculum.usda`. An **app** is a layer that sublayers the
tracks it offers — that layer is the whole answer to "which tracks does this app
show, in what order", and it is why `basic` appears under sandbox without any
track declaring `hosts = ["sandbox"]`. A **twin** contributes by composing its
own `sim/tutorials/curriculum.usda`, on the same terms as the engine.

## Add a tutorial (two steps)

1. Drop `tutorials/<track>/<name>.rhai`. Author it with the prelude verbs:
   - `coach_step(steps, i)` + the `on_event` cursor — a guided coach-mark tour.
   - `hint(...)`, `spotlight(anchor, caption)`, `notify_kind(...)` — HUD.
   - `mission(me)` with `objective(...)` — auto-published objectives that advance
     on real actions (`requires_event`, `done` predicates); emits `MISSION_COMPLETE`.
   - Setup: `cmd("OpenClass", #{ qualified })`, `set_subsystem(name, on)`.
   - **NOT `load_scene(...)`** — the world is declared, see step 2.

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
       uniform bool lunco:tutorial:firstStart = false   # true = onboarding entry
       rel lunco:tutorial:next = </Sandbox/NextLesson>  # omit to end the chain
   }
   ```

   The lesson's id is its PRIM PATH, so the chain is a real relationship. The
   launcher mounts the `payload` through `LoadScene` **before** running the
   script; a lesson with no payload deliberately leaves the viewport alone.

   Asset paths are **scheme-qualified** (`lunco://`, `twin://`) — a bare path is
   ambiguous once a Twin is open; see
   `docs/architecture/55-scene-addressing-and-roots.md`.

That's it — **no rebuild, no Rust**. On native the curriculum *and* the script
are read fresh from disk — edit and relaunch; on wasm scripts are embedded at
build time. `StartTutorial{id}` mounts the world and runs the script on the host.

## Anchors (for `spotlight` / `coach_step` focus)

Spotlight a workbench widget by its `HelpAnchors` key; `focus` opens the panel
first. lunica panel ids: `lunco.workbench.twin_browser`, `modelica_welcome`,
`modelica_experiments`, `modelica_inspector`, `modelica_diagnostics`,
`modelica_console`, `modelica_journal`, `modelica_component_palette`,
`modelica_diagram_inspector`; model-view anchors `model_view.view_toggles` /
`model_view.compile_buttons` (need a model open). `panel.modelica_plot` is an
instance panel — spotlight its anchor, but don't `focus` it. See `lunica/README.md`.
