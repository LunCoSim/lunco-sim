# lunco-workbench

LunCoSim's own workbench shell — the engineering-IDE frame we render
every panel inside. Native replacement for
[`bevy_workbench`](https://github.com/LunCoSim/bevy_workbench), tailored
to the Document System and the multi-domain composition workflow
documented in [`docs/architecture/11-workbench.md`](../../docs/architecture/11-workbench.md).

```text
┌─────────────────────────────────────────────────────────────┐
│ menu · command palette · title · transport · perspective tabs│
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
| Task-specific UI chrome preset | **[`Perspective`]** | `lunco-workbench-core` (trait) | Eclipse Perspective, Blender "workspace" |
| Editor session (open Twins, active tab, recents) | **Workspace** | `lunco-workspace` (wrapped here as `WorkspaceResource`) | VS Code Workspace, JetBrains Project |

None of these are `Twin` — that's the *simulation unit* on disk, a
folder with a `twin.toml`. See [`lunco-twin`](../lunco-twin/README.md)
for that.

## Core types

The stable panel, menu, perspective, tab-navigation, source-view, scene-state,
and pending-close contracts live in `lunco-workbench-core`. That contract crate
also owns scheduling labels and command payloads; this crate owns their
concrete observers. Reusable icons, text editors, and hierarchy rows live in
`lunco-workbench-widgets`. This crate owns the concrete egui/egui_dock shell and
publishes `WorkbenchSnapshot` for consumers that need current layout facts.
Per-Twin session persistence is provided by `lunco-workbench-state`; this shell
supplies that package's layout-provider adapter for dock capture and restore.
The Twin and Files browser is a separate reusable feature package,
`lunco-workbench-browser`, which consumes the core/widget contracts without
linking this shell; hosts compose both explicitly when they need those
navigation surfaces. Guided HUDs and coach-mark tours are likewise an optional
host-level feature in `lunco-workbench-guided-ui`; the base shell publishes the
generic anchor and render-set contracts but does not install guided behavior.

| Type | Role |
|------|------|
| `lunco_workbench_core::Panel` / `PanelCtx` | Contract every dockable UI implements |
| `lunco_workbench_core::PanelId` / `PanelSlot` | Stable panel identity and semantic dock region |
| `WorkbenchSnapshot` | Published shell-independent view of active perspective, tabs, docked panels, and each dock leaf's active visible tab |
| `WorkbenchLayout` | Private shell resource tracking the concrete `egui_dock` tree |
| [`WorkbenchPlugin`] | Installs the frame renderer + WorkspacePlugin into a Bevy app |
| [`lunco_workbench_core::WorkbenchPanelAppExt::register_panel`] | Ergonomic `app.register_panel(MyPanel)` contract registration |
| `lunco_workbench_core::Perspective` | Trait for a named slot-assignment preset (Build, Simulate, …) |
| `lunco_workbench_core::PerspectiveId` | Stable perspective identifier |
| [`WorkbenchAppExt::register_perspective`] | `app.register_perspective(MyPerspective)` |
| [`WorkspaceResource`] | Bevy `Resource` wrapping `lunco_workspace::Workspace` (open Twins + documents + active selectors) |
| [`WorkspacePlugin`] | Registers `WorkspaceResource` + the `RegisterDocument` / `UnregisterDocument` observer pair |
| [`TwinAdded`] / [`TwinClosed`] / [`DocumentOpened`] / [`DocumentClosed`] | Fine-grained session events observers react to |

## Minimal usage

```rust,no_run
use bevy::prelude::*;
use bevy_egui::{egui, EguiPlugin};
use lunco_workbench::{WorkbenchAppExt, WorkbenchPlugin};
use lunco_workbench_core::{
    Panel, PanelCtx, PanelId, PanelSlot, Perspective, PerspectiveId,
    PerspectiveLayoutPlan, PerspectiveSlotPlan,
};

struct SceneTreePanel;
impl Panel for SceneTreePanel {
    fn id(&self) -> PanelId { PanelId("scene_tree") }
    fn title(&self) -> String { "Scene Tree".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }
    fn render(&mut self, ui: &mut egui::Ui, _ctx: &mut PanelCtx) {
        ui.label("• Colony");
    }
}

struct BuildPerspective;
impl Perspective for BuildPerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("build") }
    fn title(&self) -> String { "🏗 Build".into() }
    fn layout(&self) -> PerspectiveLayoutPlan {
        let mut plan = PerspectiveLayoutPlan::new();
        plan.side_browser = PerspectiveSlotPlan::new().single(Some(PanelId("scene_tree")));
        plan
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

Applications that need the standard Twin and Files navigation add the browser
feature alongside the shell:

```rust,no_run
use bevy::prelude::App;
use lunco_workbench::WorkbenchPlugin;
use lunco_workbench_browser::TwinBrowserPlugin;

App::new()
    .add_plugins(WorkbenchPlugin)
    .add_plugins(TwinBrowserPlugin);
```

Hosts that render authored guided scenarios also add
`lunco_workbench_guided_ui::GuidedOverlayPlugin` after `WorkbenchPlugin`.

The workbench is embedded by the apps that use it — run one of them to see it live:

```bash
cargo run --bin luncosim     # ground-physics luncosim
cargo run --bin lunica      # Modelica workbench
```

## What ships today

- **`egui_dock`-backed dock tree** — drag tabs to rearrange, drag to
  edges to split, double-click tabs to maximise, multiple tabs per
  region.
- `Panel` trait with a capability-limited `PanelCtx` render context.
- Default-slot registration — panel goes where its author said it
  should the first time it's registered.
- `PerspectiveLayoutPlan` materialization — the shell turns semantic slot
  declarations from `lunco-workbench-core` into the concrete dock tree.
- Multi-instance tabs can be seeded by a perspective with
  `PerspectiveLayoutPlan::open_instance`; the instance panel's `default_slot()`
  determines the insertion region and cached user layouts remain authoritative.
- **Perspectives** (renamed from the earlier `Workspace` trait — the
  latter is now taken for the editor session concept). Register any
  number; registered perspectives are API/guided-available, and those
  opting into the default switcher appear in the transport bar. Clicking
  a visible tab applies its slot preset by rebuilding the dock.
- First-registered perspective auto-activates.
- 3D-friendly: when no panels are docked the central region stays
  transparent so a Bevy 3D scene shows through.
- **`WorkspaceResource`** — single source of truth for open Twins +
  documents + the active Twin / Document / Perspective.
- **`lunco_workbench_state::WorkspaceStateRestorePolicy`** — a host may provide a one-shot initial
  perspective for an explicit launch request. It takes precedence during the
  first Twin restore and is then consumed; ordinary user perspective changes
  continue to persist per Twin.
- The shell is browser-agnostic. Add `lunco-workbench-browser` when the
  application needs Twin/Files navigation; that package provides the standard
  panels and accepts domain `BrowserSection` implementations.

The workbench derives `egui::Visuals` once per `Theme` revision and shares
that snapshot with every panel surface. Panel render paths should consume the
active style or theme tokens; they must not rebuild a full visuals palette per
frame.

Ordinary dock and side-panel bodies use the theme's translucent
`DesignTokens::overlay_backdrop` by default, so the active scene remains
visible without sacrificing text contrast. `WorkbenchAppearanceSettings` owns
the opt-out to an opaque mantle body, while standard `PanelCtx` content frames
stay transparent over the shared body. The full panel leaf remains workbench
chrome for pointer routing in either mode.

## Not yet built

- **Standard perspective presets** (Build / Simulate / Analyze / Plan /
  Observe) — host apps define them as they migrate panels.
- **Command palette** — planned; `Ctrl+P` is intentionally unbound.
- **Detached panel windows** — planned; the current panel contract is dock-only.

## Design rationale

See [`docs/architecture/11-workbench.md`](../../docs/architecture/11-workbench.md).

Authoring-only perspectives can override `Perspective::show_in_switcher()`
to stay out of the everyday title-bar navigation while remaining available to
`ActivatePerspective` and guided guideds.

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
   ├── lunco-workbench-state ← session persistence and codec boundary
   │       ├── lunco-storage  ← storage-backed state I/O
   │       ├── lunco-doc      ← document snapshots and origins
   │       └── lunco-workspace
   ├── lunco-twin        ← Twin struct + manifest + recursion
   ├── lunco-workspace   ← editor session type (headless)
   │
   ├── lunco-workbench-core ← contracts (Panel, Perspective, Snapshot)
   │       ▲
   │       │ panels and perspectives implement these contracts
   │       └── lunco-workbench ← this crate (editor shell + WorkspaceResource)
   │              ▲
   │              └── lunco-workbench-browser ← optional Twin/Files feature
   │                     ▲
   │                     │ browser panels and section contract used by app/domain UI
   │                     └── lunco-modelica-ui, lunco-luncosim-edit-ui, …
```
