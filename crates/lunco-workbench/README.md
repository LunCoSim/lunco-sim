# lunco-workbench

LunCoSim's own workbench shell — the engineering-IDE frame we render every
panel inside. Native replacement for
[`bevy_workbench`](https://github.com/LunCoSim/bevy_workbench), tailored
to the Document System and the multi-domain composition workflow
documented in [`docs/architecture/11-workbench.md`](../../docs/architecture/11-workbench.md).

```text
┌─────────────────────────────────────────────────────────────┐
│ menu bar  ·  command palette                                │
├─────────────────────────────────────────────────────────────┤
│ workspace tabs  ·  transport controls                       │
├───┬─────────────────────┬──────────────┬────────────────────┤
│ A │                     │              │                    │
│ c │   side browser      │   VIEWPORT   │   Inspector        │
│ t │   (per activity)    │              │ (context-aware)    │
│ i │                     │              │                    │
│ v │                     ├──────────────┤                    │
│   │                     │ bottom dock  │                    │
├───┴─────────────────────┴──────────────┴────────────────────┤
│ status bar                                                  │
└─────────────────────────────────────────────────────────────┘
```

## Core types

| Type | Role |
|------|------|
| [`Panel`] | Trait every dockable UI implements: `id`, `title`, `default_slot`, `render(&mut Ui, &mut World)` |
| [`PanelId`] | Stable identifier newtype |
| [`PanelSlot`] | Dock region: `SideBrowser` / `RightInspector` / `Bottom` / `Floating` |
| [`WorkbenchLayout`] | Bevy resource tracking what's docked where |
| [`WorkbenchPlugin`] | Installs the frame renderer into a Bevy app |
| [`WorkbenchAppExt::register_panel`] | Ergonomic `app.register_panel(MyPanel)` extension |
| [`Workspace`] | Trait for a named slot-assignment preset (e.g., Build, Simulate) |
| [`WorkspaceId`] | Stable workspace identifier |
| [`WorkbenchAppExt::register_workspace`] | Ergonomic `app.register_workspace(MyWs)` extension |

## Minimal usage

```rust,no_run
use bevy::prelude::*;
use bevy_egui::{egui, EguiPlugin};
use lunco_workbench::{
    Panel, PanelId, PanelSlot, Workspace, WorkspaceId,
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

struct BuildWorkspace;
impl Workspace for BuildWorkspace {
    fn id(&self) -> WorkspaceId { WorkspaceId("build") }
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
        .register_workspace(BuildWorkspace)
        .run();
}
```

Run the demo:

```bash
cargo run -p lunco-workbench --example hello_workbench
```

The `hello_workbench` example demonstrates two workspaces (Build and
Simulate) with different slot presets — click the tabs in the transport
bar to switch.

## What ships in v0.1 (today)

- Frame layout (menu / transport / activity / side / viewport / right / bottom / status)
- `Panel` trait with uniform `&mut World` signature (no `ui` vs `ui_world` split)
- Default-slot registration — panel goes where its author said it should
- **Workspaces** — register any number, their tabs appear in the transport bar, clicking applies the slot preset
- First-registered workspace auto-activates
- Bottom-dock toggle and activity-bar toggle under the View menu

## What's explicitly NOT shipped yet

- **Standard workspace presets** (Build / Simulate / Analyze / Plan / Observe) — the mechanism is there, host apps define the five LunCoSim workspaces themselves as they migrate panels
- **Layout persistence** — dock sizes and the active workspace reset on launch
- **Command palette** — `Ctrl+P` unbound
- **Detachable windows** — `PanelSlot::Floating` is a placeholder
- **Tabbing and splitting** — one panel per slot in v0.1
- **Theming / keybinds** — egui defaults only

Each of these lands in its own commit once a concrete panel migration
needs it. We don't pay for what we don't use.

## Design rationale

See [`docs/architecture/11-workbench.md`](../../docs/architecture/11-workbench.md)
for the full design — why these slots, why workspaces later, how panels
relate to the Document System.

### Why not keep using `bevy_workbench`?

We've been using `bevy_workbench` (our fork) for early UI work. It gave
us dock persistence and tabbing on day one. But its `WorkbenchPanel`
trait splits rendering into `ui(&mut Ui)` and `ui_world(&mut Ui, &mut World)`,
and every nontrivial panel ends up in the `ui_world` branch anyway.
`lunco-workbench` collapses that to a single `render(&mut Ui, &mut World)` —
matching how real panels want to be written.

The migration is **clean cutover**: panels move across one at a time,
and once every panel is migrated `bevy_workbench` is dropped in a single
commit. Both shells coexist in the workspace during the migration so
production binaries keep working.

## Crate graph

```
bevy + bevy_egui
   │
   ├── bevy_workbench   ← retiring
   │
   └── lunco-workbench  ← this crate
          ▲
          │ panels plug into (via Panel trait)
          └── lunco-modelica, lunco-sandbox-edit, lunco-cosim, …
```
