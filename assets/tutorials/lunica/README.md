# Lunica tutorials (rhai-scripted coach-mark tours)

These are the guided lessons for **lunica** (the Modelica workbench). Unlike the
sandbox tutorials — which load a USD scene with a `lunco:scriptPath` orchestrator
— a lunica lesson is a **standalone rhai scenario**: there is no 3D scene, just a
coach-mark tour over the workbench panels.

Each `*.rhai` here is an ordinary scenario for the `lunco-scripting` runtime
(added to lunica in `build_modelica_core`). It drives the shared coach card:

- `coach_step(steps, i)` — spotlight a widget + draw the card for step `i`
  (`steps` is a table of `#{ anchor, title, body, focus }`; prelude: `hud.rhai`).
- `on_event(me, evt)` — advance the `this.i` cursor on the card's
  `cmd:TutorialNext` / `Back` / `Skip` / `Goto` bus events.
- `cmd("FocusPanel", #{ id })` — open a panel so its spotlight anchor is on screen.
- `cmd("OpenClass", #{ qualified })` — open a bundled model to demonstrate on.
- `emit("MISSION_COMPLETE", 0)` — mark the lesson done (menu ✓).

## The curriculum

| File | Lesson | Concept |
|------|--------|---------|
| `overview.rhai`    | Lunica Overview        | first-run tour of the whole workbench |
| `workspace.rhai`   | 1 · Your Workspace     | Twins, browser, libraries, learning paths |
| `model.rhai`       | 2 · Open & View a Model| the four views + graphical composition |
| `run.rhai`         | 3 · Compile & Run      | Interactive vs Fast Run, live inputs |
| `experiments.rhai` | 4 · Experiments & Sweeps | parameter overrides + sweeps |
| `plots.rhai`       | 5 · Plots & Results    | graphs, diagnostics, console |
| `scripting.rhai`   | 6 · Automate           | scripts + HTTP API + MCP |
| `onboarding.rhai`  | *(gate, not a lesson)* | first-run policy — shows the Overview once, in rhai |

`onboarding.rhai` is the rhai reimplementation of the old `tour_seen` behaviour:
attached once per process at startup, it reads/writes the persisted `tour_seen`
flag via `get_setting`/`set_setting("TutorialSeen.onboarded", ..)` and only shows
the Overview on a genuine first run. The gate is *in rhai*, not Rust.

## Launching

The **🎓 Tutorials** menu (top of the lunica menu bar) lists every lesson; Help ▸
Show Tour (and F1) replays the Overview. The catalog + menu live in
`crates/lunco-modelica/src/ui/help_overlay.rs` (`TUTORIALS`).

## Editing live

Source loading is owned by `lunco-assets` (`tutorials::lunica_tutorial_source`):

- **Native** reads the `.rhai` fresh from disk on **every** launch. So: edit a
  file here, pick the lesson again from the 🎓 Tutorials menu (or F1), and your
  change plays immediately — **no rebuild**. `RunScenario` hot-reloads the host.
- **wasm** (no filesystem) serves the `include_dir!`-embedded copy; a rebuild
  bakes in your edits there.

The launcher (`help_overlay.rs`) never touches `include_str!` — it asks the asset
crate for the source by id.

## Adding a lesson

1. Drop a new `*.rhai` here (copy an existing one — they share the `on_event`
   cursor driver; just change `steps()`).
2. Add a `LunicaTutorial { … }` row to `TUTORIALS` in `help_overlay.rs`.

Anchors you can spotlight (`focus` = the panel id to open first):

| Anchor key | `focus` panel id |
|------------|------------------|
| `panel.lunco_twin_browser`     | `lunco_twin_browser` |
| `panel.modelica_welcome`       | `modelica_welcome` |
| `model_view.view_toggles`      | — (needs a model open) |
| `model_view.compile_buttons`   | — (needs a model open) |
| `panel.modelica_component_palette` | `modelica_component_palette` |
| `panel.modelica_diagram_inspector` | `modelica_diagram_inspector` |
| `panel.modelica_plot`          | `modelica_plot` |
| `panel.modelica_experiments`   | `modelica_experiments` |
| `panel.modelica_inspector`     | `modelica_inspector` |
| `panel.modelica_diagnostics`   | `modelica_diagnostics` |
| `panel.modelica_console`       | `modelica_console` |
| `panel.modelica_journal`       | `modelica_journal` |
| `menu.help`                    | — |
