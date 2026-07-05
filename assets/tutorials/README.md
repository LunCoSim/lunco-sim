# Tutorials — one source, one launcher

A tutorial is **one thing: a `.rhai` scenario**. There is no scene-vs-script
split. The shared launcher (`crates/lunco-tutorial`) runs it on a host entity via
`RunScenario`; the scenario sets up whatever it needs in `on_start`. The coach
card / spotlight / objectives come from the shared HUD + the rhai prelude.

## Layout

```
assets/tutorials/
  learning_paths.json        # the Welcome-panel MSL curriculum (separate feature)
  lunica/                    # lunica (Modelica workbench) lessons
    overview.rhai  run.rhai  experiments.rhai  …
  sandbox/                   # sandbox (3D world) lessons
    sandbox_intro.rhai
    first_drive.rhai   first_drive.usda      # env `.usda` co-located, load_scene'd
    lander_mission.rhai
```

Convention: **`tutorials/<app>/<name>.rhai`**. A lesson that needs a 3D world
ships an env-only `.usda` next to it (or reuses one under `scenes/`) and pulls it
in with `load_scene(...)`. A lesson that needs a model just `cmd("OpenClass", …)`.

## Add a tutorial (two steps)

1. Drop `tutorials/<app>/<name>.rhai`. Author it with the prelude verbs:
   - `coach_step(steps, i)` + the `on_event` cursor — a guided coach-mark tour.
   - `hint(...)`, `spotlight(anchor, caption)`, `notify_kind(...)` — HUD.
   - `mission(me)` with `objective(...)` — auto-published objectives that advance
     on real actions (`requires_event`, `done` predicates); emits `MISSION_COMPLETE`.
   - Setup: `load_scene("scenes/…")`, `cmd("OpenClass", #{ qualified })`,
     `set_subsystem(name, on)`.
2. Add an entry to that app's manifest `tutorials/<app>/tutorials.json` — **data,
   not Rust**. The app already loads it via `TutorialPlugin { app: "<app>" }`.
   ```json
   {
     "id": "sandbox-my-lesson",           // app-prefixed (shared progress settings)
     "title": "My Lesson", "blurb": "…",
     "app": "sandbox", "difficulty": "beginner",
     "script": "sandbox/my_lesson.rhai",  // path under assets/tutorials/
     "first_start": false,                // true = the once-only onboarding entry
     "next": null                         // "next-id" to chain on completion
   }
   ```

That's it — **no rebuild, no Rust**. On native the manifest *and* the script are
read fresh from disk (`lunco_assets::tutorials::tutorial_source`) — edit and
relaunch; on wasm both are embedded at build time. `StartTutorial{id}` loads the
script and runs it on the host.

## Anchors (for `spotlight` / `coach_step` focus)

Spotlight a workbench widget by its `HelpAnchors` key; `focus` opens the panel
first. lunica panel ids: `lunco.workbench.twin_browser`, `modelica_welcome`,
`modelica_experiments`, `modelica_inspector`, `modelica_diagnostics`,
`modelica_console`, `modelica_journal`, `modelica_component_palette`,
`modelica_diagram_inspector`; model-view anchors `model_view.view_toggles` /
`model_view.compile_buttons` (need a model open). `panel.modelica_plot` is an
instance panel — spotlight its anchor, but don't `focus` it. See `lunica/README.md`.
