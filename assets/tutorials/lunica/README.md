# Lunica tutorials

The guided lessons for **lunica** (the Modelica workbench). These are ordinary
tutorials — one `.rhai` scenario each — run by the shared launcher
(`crates/lunco-tutorial`). See [`../README.md`](../README.md) for the general
"one source, one launcher" model and how to add a tutorial; this file covers the
lunica-specific bits.

A lunica lesson needs no 3D scene — it coaches over the workbench panels and (for
model-centric lessons) opens a model itself:

- `coach_step(steps, i)` — spotlight a widget + draw the card for step `i`
  (`steps` = a table of `#{ anchor, title, body, focus }`; prelude: `hud.rhai`).
- `on_event(me, evt)` — advance the `this.i` cursor on the card's
  `cmd:TutorialNext` / `Back` / `Skip` / `Goto` bus events.
- `cmd("FocusPanel", #{ id })` — open a panel so its spotlight anchor is on screen.
- `cmd("OpenClass", #{ qualified: "CascadedRCFilter" })` — open a bundled model to
  demonstrate on (any bundled/MSL/workspace class resolves — one `OpenClass`).
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

Registered into the shared launcher by `lunica_tutorials()` in
`crates/lunco-modelica/src/ui/mod.rs` (ids `lunica-*`). `Overview` is
`first_start: true` — the first-run onboarding entry.

## Launching

The **🎓 Tutorials** menu (top of the lunica menu bar) lists every lesson; F1 (and
the perspective help's "Show Tour") starts the Overview. All of these issue the
same `StartTutorial{id}` command. First-run onboarding is decided by the boot
policy (`assets/scripting/policy/boot.rhai`) — not Rust — and shows the Overview
once (persisted under the `tour_seen` setting).

## Anchors (for `spotlight` / `coach_step` focus)

`focus` opens the panel first, then `panel.<id>` spotlights it. lunica panel ids:

| Anchor key | `focus` panel id |
|------------|------------------|
| `panel.lunco.workbench.twin_browser` | `lunco.workbench.twin_browser` |
| `panel.modelica_welcome`             | `modelica_welcome` |
| `model_view.view_toggles`            | — (needs a model open) |
| `model_view.compile_buttons`         | — (needs a model open) |
| `panel.modelica_component_palette`   | `modelica_component_palette` |
| `panel.modelica_diagram_inspector`   | `modelica_diagram_inspector` |
| `panel.modelica_experiments`         | `modelica_experiments` |
| `panel.modelica_inspector`           | `modelica_inspector` |
| `panel.modelica_diagnostics`         | `modelica_diagnostics` |
| `panel.modelica_console`             | `modelica_console` |
| `panel.modelica_journal`             | `modelica_journal` |
| `panel.modelica_plot`                | — (instance panel; spotlight the anchor, don't `focus`) |
| `menu.help`                          | — |

## Editing live

Native reads each `.rhai` **fresh from disk** on every launch (owned by
`lunco_assets::tutorials::tutorial_source`), so edit a file, pick the lesson again
from the 🎓 menu, and your change plays with **no rebuild** (`RunScenario`
hot-reloads the host). wasm serves the `include_dir!`-embedded copy.
