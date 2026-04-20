# lunco-workbench

LunCoSim's own workbench shell — the engineering-IDE frame we render
every panel inside. Native replacement for
[`bevy_workbench`](https://github.com/LunCoSim/bevy_workbench), tailored
to the Document System and the multi-domain composition workflow
documented in [`docs/architecture/11-workbench.md`](../../docs/architecture/11-workbench.md).

```text
┌─────────────────────────────────────────────────────────────┐
│ menu bar  ·  command palette                                │
├─────────────────────────────────────────────────────────────┤
│ perspective tabs  ·  transport controls                     │
├───┬─────────────────────┬──────────────┬────────────────────┤
│ A │                     │              │                    │
│ c │  Twin Browser       │   VIEWPORT   │   Inspector        │
│ t │  (twins + docs)     │              │ (context-aware)    │
│ i │                     │              │                    │
│ v │                     ├──────────────┤                    │
│   │                     │ bottom dock  │                    │
├───┴─────────────────────┴──────────────┴────────────────────┤
│ status bar                                                  │
└─────────────────────────────────────────────────────────────┘
```

## Three concepts, three different things

Part of the motivation for this crate was to untangle three ideas that
share the word "workspace" in other tools:

| Concept | Our term | Lives in | Analogy |
|---|---|---|---|
| Editor shell (dock engine + panel registry) | **Workbench** | `lunco-workbench` (this crate) | Eclipse Workbench, VS Code workbench |
| Task-specific UI chrome preset | **[`Perspective`]** | this crate (trait) | Eclipse Perspective, Blender "workspace" |
| Editor session (open Twins, active tab, recents) | **Workspace** | `lunco-workspace` (wrapped here as `WorkspaceResource`) | VS Code Workspace, JetBrains Project |

None of these are `Twin` — that's the *simulation unit* on disk, a
folder with a `twin.toml`. See [`lunco-twin`](../lunco-twin/README.md)
for that.

## Core types

| Type | Role |
|------|------|
| [`Panel`] | Trait every dockable UI implements: `id`, `title`, `default_slot`, `render(&mut Ui, &mut World)` |
| [`PanelId`] | Stable identifier newtype |
| [`PanelSlot`] | Dock region: `SideBrowser` / `Center` / `RightInspector` / `Bottom` / `Floating` |
| [`WorkbenchLayout`] | Bevy resource tracking what's docked where |
| [`WorkbenchPlugin`] | Installs the frame renderer + WorkspacePlugin into a Bevy app |
| [`WorkbenchAppExt::register_panel`] | Ergonomic `app.register_panel(MyPanel)` extension |
| [`Perspective`] | Trait for a named slot-assignment preset (Build, Simulate, …) |
| [`PerspectiveId`] | Stable perspective identifier |
| [`WorkbenchAppExt::register_perspective`] | `app.register_perspective(MyPerspective)` |
| [`WorkspaceResource`] | Bevy `Resource` wrapping `lunco_workspace::Workspace` (open Twins + documents + active selectors) |
| [`WorkspacePlugin`] | Registers `WorkspaceResource` + the `RegisterDocument` / `UnregisterDocument` observer pair |
| [`TwinAdded`] / [`TwinClosed`] / [`DocumentOpened`] / [`DocumentClosed`] | Fine-grained session events observers react to |
| [`TwinBrowserPanel`] | Built-in side-panel shell for domain `BrowserSection` impls |
| [`BrowserSection`] / [`BrowserSectionRegistry`] | Pluggable section trait — Modelica / USD / future domains register one each |

## Minimal usage

```rust,no_run
use bevy::prelude::*;
use bevy_egui::{egui, EguiPlugin};
use lunco_workbench::{
    Panel, PanelId, PanelSlot, Perspective, PerspectiveId,
    WorkbenchAppExt, WorkbenchLayout, WorkbenchPlugin,
};

struct SceneTreePanel;
impl Panel for SceneTreePanel {
    fn id(&self) -> PanelId { PanelId("scene_tree") }
    fn title(&self) -> String { "Scene Tree".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }
    fn render(&mut self, ui: &mut egui::Ui, _world: &mut World) {
        ui.label("• Colony");
    }
}

struct BuildPerspective;
impl Perspective for BuildPerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("build") }
    fn title(&self) -> String { "🏗 Build".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_side_browser(Some(PanelId("scene_tree")));
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(EguiPlugin::default())
        .add_plugins(WorkbenchPlugin)
        .register_panel(SceneTreePanel)
        .register_perspective(BuildPerspective)
        .run();
}
```

Run the demo:

```bash
cargo run -p lunco-workbench --example hello_workbench
```

## What ships today

- **`egui_dock`-backed dock tree** — drag tabs to rearrange, drag to
  edges to split, double-click tabs to maximise, multiple tabs per
  region.
- `Panel` trait with uniform `&mut World` signature.
- Default-slot registration — panel goes where its author said it
  should the first time it's registered.
- Slot-setter DSL (`set_side_browser` / `set_center` /
  `set_right_inspector` / `set_bottom`) — convenience for Perspective
  presets.
- **Perspectives** (renamed from the earlier `Workspace` trait — the
  latter is now taken for the editor session concept). Register any
  number; their tabs appear in the transport bar; clicking applies the
  slot preset by rebuilding the dock.
- First-registered perspective auto-activates.
- 3D-friendly: when no panels are docked the central region stays
  transparent so a Bevy 3D scene shows through.
- **`WorkspaceResource`** — single source of truth for open Twins +
  documents + the active Twin / Document / Perspective.
- **Twin Browser** — built-in side-panel that renders `BrowserSection`
  impls contributed by domain plugins, reading the active Twin from
  `WorkspaceResource`.

## What's explicitly NOT shipped yet

- **Standard perspective presets** (Build / Simulate / Analyze / Plan /
  Observe) — host apps define them as they migrate panels.
- **Layout persistence** — dock changes reset on launch (egui_dock has
  serde support for the tree; wiring is a follow-up).
- **Command palette** — `Ctrl+P` unbound.
- **`PanelSlot::Floating`** — placeholder; egui_dock's window support
  is there but not wired to our `Panel` registration yet.
- **Theming / keybinds** — egui defaults only.

## Design rationale

See [`docs/architecture/11-workbench.md`](../../docs/architecture/11-workbench.md).

### Why Perspective instead of "Workspace"?

Different tools use the same word for different things. The table
above summarises the three ideas we care about. Blender calls its
layout presets "workspaces"; Eclipse calls them "perspectives". When
we needed a term for the bigger thing (the VS-Code-style editor
session containing many open Twins + recents + settings), **Workspace**
was the clear industry choice. That forced the renaming of the layout
preset to **Perspective** — a compact, precise, established term with
no naming collision.

## Crate graph

```
bevy + bevy_egui
   │
   ├── lunco-storage     ← I/O trait + backends
   ├── lunco-doc         ← Document trait, DocumentId, DocumentOrigin
   ├── lunco-twin        ← Twin struct + manifest + recursion
   ├── lunco-workspace   ← editor session type (headless)
   │
   └── lunco-workbench   ← this crate (editor shell + WorkspaceResource)
          ▲
          │ panels plug into (via Panel trait)
          └── lunco-modelica, lunco-sandbox-edit, lunco-cosim, …
```
