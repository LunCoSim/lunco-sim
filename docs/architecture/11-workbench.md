# 11 — Workbench (UI/UX Architecture)

> How LunCoSim's user interface is organized: the workbench shell, workspaces,
> panels, viewport, command palette, detachable windows. Establishes the
> framework on top of which all domain-specific UI lives.
>
> Status: design. Implementation lives in the planned `lunco-workbench` crate,
> which replaces the current `bevy_workbench` dependency incrementally.

## 1. What "workbench" means here

A **workbench** is the application shell of a LunCoSim app — the chrome around
the 3D world. It owns the root window layout, the workspace switcher, the
panel registry, the command palette, keybinds, and detachable window support.

Terminology mapping:

| Concept | Our term | Analogs |
|---------|----------|---------|
| App shell (layout engine) | **Workbench** (`lunco-workbench`) | CATIA app, VS Code workbench, Qt QMainWindow |
| Task-specific UI configuration | **Workspace** | CATIA workbenches (Part Design, Assembly); Blender workspaces |
| A dockable UI element | **Panel** | VS Code sidebar view, Blender editor area |
| The 3D world | **Viewport** (structural, not a panel) | CAD 3D view |
| Primary navigation category | **Activity** | VS Code activity bar |

All defined in [`01-ontology.md`](01-ontology.md) § 4d.

## 2. Why we're building this

The initial plan used `bevy_workbench` (an `egui_tiles`-based docking crate).
It got us off the ground but revealed architectural mismatches for a
3D-canvas engineering app — see
[`research/ui-ux-inspiration.md`](research/ui-ux-inspiration.md) § "Rejected
paths" for details. Summary:

- **Viewport is not a tile.** `egui_tiles` treats every tile as equal; our
  3D scene must be structurally persistent, always central, never closable,
  never mergeable with siblings.
- **Workspaces > panel presets.** Users doing fundamentally different tasks
  (Build vs. Simulate vs. Observe) need a one-click UI reshape, not just
  different open panels.
- **Detachable windows are a must.** Engineers work across multiple
  monitors; `bevy_workbench` doesn't support tab-drag-out-to-window.

`lunco-workbench` is built around a **SidePanel + CentralPanel** root layout
(the standard egui pattern for CAD/IDE apps), with `egui_tiles` used *inside*
the side panels for tabbed dock trees.

## 3. The standard layout

```
┌─────────────────────────────────────────────────────────────────────┐
│ File  Edit  View  Window  Help        [Cmd+P: search anything]      │ ← menu bar
├─────────────────────────────────────────────────────────────────────┤
│ [🏗️ Build] [🎮 Sim] [📊 Analyze] [🗓️ Plan] [🎬 Observe] ⏮▶⏸ 00:14:32│ ← workspace tabs + transport
├───┬─────────────────────────────────────────────────────┬───────────┤
│   │                                                     │           │
│ A │                                                     │           │
│ c │                                                     │ Properties│
│ t │           3D VIEWPORT                               │ (context- │
│ i │        (the world itself —                          │  aware)   │
│ v │         full height,                                │           │
│ i │         docks anchor around it,                     │           │
│ t │         never DIVIDE it)                            │           │
│ y │                                                     │           │
│   ├─────────────────────────────────────────────────────┤           │
│ b │  Console / Plots / Timeline (toggleable, per-workspace content) │
│ a │                                                     │           │
│ r │                                                     │           │
├───┴─────────────────────────────────────────────────────┴───────────┤
│ Moon surface · g=1.62 · t=00:14:32 · balloon-0 selected · FPS 60    │ ← status bar
└─────────────────────────────────────────────────────────────────────┘
```

**Layout regions (from outside in):**

1. **Menu bar (top)** — File / Edit / View / Window / Help menus +
   right-aligned command-palette search bar.
2. **Transport bar (top, below menu)** — workspace switcher tabs on the
   left, transport controls (play / pause / step / time scrubber / speed)
   on the right.
3. **Activity bar (far left)** — vertical strip of icons for primary
   navigation (Scene / Subsystems / Assets / Console / Search / Settings).
   Click an icon to open its browser in a slide-in panel.
4. **Slide-in side browser (between activity bar and viewport)** —
   content depends on the selected Activity. Resizable, collapsible.
5. **Viewport (center)** — the 3D world. **Structurally persistent** —
   always the central region of the window. Not a panel, not a tile.
   Cannot be closed or docked-over.
6. **Properties / Inspector (right)** — context-aware content for the
   current selection and workspace. See § 6.
7. **Bottom panel (toggleable)** — workspace-dependent: Console, Plots,
   Timeline, etc. Collapsible to zero height.
8. **Status bar (bottom strip)** — sim time, speed, selected entity,
   celestial body, FPS.

## 4. Workspaces

A workspace is a named task-specific UI configuration. LunCoSim ships with
five standard workspaces.

### Build — edit scenes and subsystems

Purpose: construct the colony. Place entities, wire subsystems, author
physical models.

| Slot | Default content |
|------|-----------------|
| Activity | Scene (active), Subsystems, Assets |
| Side browser | Scene tree |
| Right | Inspector (Transform, RigidBody, Modelica attachment, Attributes) |
| Bottom | Spawn Palette (collapsible) |

### Simulate — run and observe

Purpose: run physics + Modelica, watch behaviors. Minimize chrome.

| Slot | Default content |
|------|-----------------|
| Activity bar | Collapsed (icons only) |
| Side browser | Closed |
| Right | Minimal — just current selection label |
| Bottom | Live telemetry (plots for selected entity) |

In this workspace, most docks auto-hide. Toggle back with standard keybinds.

### Analyze — Modelica / subsystem deep dive

Purpose: study and tune an individual subsystem model. This is the
"modelica_workbench" layout, consolidated.

| Slot | Default content |
|------|-----------------|
| Activity | Subsystems (active) |
| Side browser | Library Browser (MSL + project models) |
| Right | Modelica Inspector (params, variables) |
| Bottom | Plots (time series) |
| Center overlay (optional) | Diagram / Code editor (when a model is open) |

Selecting a Space System with a `ModelicaModel` automatically focuses the
editor on that model.

### Plan — mission timeline and events

Purpose: author mission timelines, schedule events, lay out trajectories.

| Slot | Default content |
|------|-----------------|
| Activity | Missions (active) |
| Side browser | Mission outline |
| Right | Event / maneuver properties |
| Bottom | Timeline (primary authoring surface) |

### Observe — cinema / presentation

Purpose: camera-driven observation. No editing.

| Slot | Default content |
|------|-----------------|
| Menu bar | Hidden |
| Activity bar | Hidden |
| Side browser | Hidden |
| Right | Hidden |
| Bottom | Hidden |
| Status bar | Minimal (time + body) |

Just the viewport, transport, and status. User can still show panels via
keyboard if needed.

### User-defined workspaces

Users can customize any layout and save as a named workspace. Ship workspaces
are just the defaults; everything is editable.

## 5. Panel system

Panels live inside side docks or bottom docks. Each panel implements a
small trait:

```rust
// Sketch — in lunco-workbench
pub trait Panel: Send + Sync + 'static {
    fn id(&self) -> &str;
    fn title(&self) -> String;
    fn category(&self) -> PanelCategory;   // Navigation / Inspector / Tool / Output
    fn default_location(&self) -> PanelSlot; // Left / Right / Bottom / Floating
    fn render(&mut self, ui: &mut egui::Ui, world: &mut World);
}
```

Panel categories map to default slot rules:

| Category | Default slot | Examples |
|----------|--------------|----------|
| Navigation | Left side | Scene Tree, Library Browser, Mission Outline |
| Inspector | Right side | Properties, Modelica Inspector, Attribute Editor |
| Tool | Bottom | Spawn Palette, Timeline, Diagram Editor |
| Output | Bottom | Console, Plots, Telemetry |

Users can drag panels between slots, tab them together, collapse them,
or detach them (see § 8).

### Panels as Document Views

Per the Document System design
([`10-document-system.md`](10-document-system.md)), many panels are
`DocumentView<D>` implementations for a specific Document type. The
Modelica Diagram panel views a `ModelicaDocument`, the Scene Tree views a
`UsdDocument`, etc. This lets multiple panels stay in sync automatically:
edit a parameter in the Inspector → the Diagram updates; drag a
component in the Diagram → the Code editor updates.

## 6. Context-awareness

The right-side Inspector panel (and workspace-specific bottom panels) are
**context-aware** — their content changes based on the current selection.

Selecting a balloon entity in Build workspace shows:

- Name, Transform
- RigidBody (Dynamic, Mass)
- AvianSim (connection inputs/outputs)
- ModelicaModel (paused, sim time)
- Modelica parameters (editable DragValues, triggering live recompile)
- Modelica variables (read-only live values)

Selecting the same balloon in Analyze workspace shows the same, plus:

- Library browser scrolled to balloon's `.mo` file
- Diagram and Code panels open on the balloon's model
- Plot panel offers the balloon's variables as checkboxes

Selection flows through a shared `SelectedEntity` resource. All workspaces
read it; the panel system re-renders on change.

## 7. Command palette

Keyboard-invoked (Ctrl+P on Linux/Windows, Cmd+P on macOS), always
available. Fuzzy-search across:

- Actions (workspaces, menu items, panel show/hide)
- Entities (navigate to Space System by name)
- Modelica models (open a `.mo` file)
- Parameters (jump to a named parameter in the Inspector)
- Commands on the `CommandRegistry` of the selected entity

Integrates with the ontology's [`CommandRegistry`](01-ontology.md)
pattern — every Space System registers its available commands with
metadata (name, args, validation ranges, documentation), which the
palette makes discoverable for humans and AI agents.

## 8. Detachable windows

Any panel can be torn out of the main window into its own OS-native
window via drag-tab-to-outside-window (standard IDE gesture) or a tab
context-menu "Detach" action.

Implementation: **egui multi-viewport**. Each detached panel becomes a
deferred viewport; the panel's `render()` runs in that viewport's egui
context rather than the main window's. Detachment is stored in the
layout persistence, so reopening the app restores the detach state.

Multi-monitor workflows this enables:
- Plots on secondary monitor, diagram on primary
- Inspector + console on third monitor
- Workspace switching only affects the main window; detached panels
  stay put regardless of workspace

## 9. Layout persistence

Each workspace has a stored layout. When a user drags a panel, saves, or
switches workspaces:

- Per-user layouts persist to `$XDG_CONFIG_HOME/lunco-workbench/layouts.toml`
  (or platform equivalent)
- Per-project layouts can live in `project.toml` (overriding user defaults)
- Default ship layouts are hardcoded in each workspace module

Layout is a tree of slot occupancies + panel IDs + sizes. Not a TOML
abstraction leak — just what's needed to rebuild the layout.

## 10. Theming and keybinds

- Theming via egui's visuals system. Shipped themes: Dark, Light, High
  Contrast. Per-user customization via `theme.toml`.
- Keybinds via a dedicated registry, each action declaring its default
  binding. User overrides in `keybinds.toml`. Modeled loosely on VS Code's
  keybind system.

Both are simple pass-throughs to egui and `bevy_workbench`-style registries;
no novel design.

## 11. Relationship to `lunco-ui` and domain crates

```
  Apps
   ├── Panel crates (domain-specific UI)
   │    lunco-modelica/ui   lunco-sandbox-edit/ui   lunco-mission/ui
   │         │                     │                       │
   │         ▼                     ▼                       ▼
   ├── lunco-workbench  (app scaffold — this document)
   │     - Root layout (SidePanel + CentralPanel)
   │     - Panel trait + registry
   │     - Workspace enum + per-workspace layout
   │     - Command palette, activity bar, status bar
   │     - Detach / multi-viewport
   │         │
   │         ▼
   ├── lunco-ui  (widget toolkit)
   │     - WidgetSystem (cached widgets)
   │     - Entity-viewer trait
   │     - Shared widgets: TimeSeries, NodeGraph base, InspectorField
   │     - Re-exports: egui-snarl, egui_plot
   │         │
   │         ▼
   └── egui + bevy_egui + egui_tiles (inside side panels only)
```

- `lunco-workbench` is the app framework — layout, workspace, panel host.
- `lunco-ui` is the widget library — draws things inside panels.
- Domain crates contribute **Panel** implementations that use `lunco-ui`
  widgets and `lunco-workbench`'s Panel trait.

Both `lunco-workbench` and `lunco-ui` are LunCoSim-agnostic at their core —
they don't know about balloons, solar panels, or Modelica. Domain knowledge
lives in domain crates.

## 12. Three LunCoSim apps, different compositions

Each binary is ~50 lines of plugin registration:

```
rover_sandbox_usd     = workbench + SpawnPalette + SceneTree + Inspector +
                        ModelicaInspector + 3D viewport
                        (sandbox editor with compact Modelica view)

lunco_client          = workbench + all sandbox panels + MissionControl +
                        CelestialBrowser + full 3D world
                        (main client, everything enabled)

modelica_workbench    = workbench + CodeEditor + Diagram + PackageBrowser +
                        Telemetry + Graphs + LibraryBrowser
                        (Modelica modeling only, no 3D world needed)
```

Same workbench shell, different panel sets, different default workspaces.
`modelica_workbench` opens in the Analyze workspace; `rover_sandbox_usd`
in Build; `lunco_client` in Observe with quick access to all others.

## 13. Migration strategy (from bevy_workbench)

The migration happens incrementally over 6–8 weeks:

| Phase | Scope |
|-------|-------|
| 1 | Design complete (this doc). |
| 2 | Implement `lunco-workbench` MVP: root layout, Panel trait, one workspace. Ship alongside `bevy_workbench` during transition. |
| 3 | Port domain panels — SpawnPalette, Inspector, EntityList, ModelicaInspector. |
| 4 | Port Modelica workbench panels (Diagram, CodeEditor, PackageBrowser, etc.). |
| 5 | Activity bar, status bar, transport controls. |
| 6 | Workspace switcher + per-workspace layouts. |
| 7 | Command palette, detachable windows. |
| 8 | Retire `bevy_workbench` dep. |

Each phase delivers standalone value. The sandbox keeps working throughout.

## 14. Open questions

- **Workspace persistence scope.** Per-user only, or per-project as well?
  Per-project lets teams share layouts; per-user lets individuals customize.
  Likely need both (project defines, user overrides).
- **Detached window survival.** If the user closes a detached window, does
  that detach it from the workspace (so reopening brings the panel back to
  its docked slot) or hide it (so it reopens detached)? VS Code chose
  "reopen docked"; most CAD chose "reopen detached."
- **Activity bar extensibility.** Can third-party plugins add new
  Activities, or is the list fixed? Leaning toward fixed for stability,
  with a "Custom" catch-all.
- **Panel categories mapping to slots.** The default-slot rules should be
  overridable — a user should be able to put the Scene Tree on the right
  if they want.

These resolve during Phase 2 implementation.

## See also

- [`10-document-system.md`](10-document-system.md) — panels as DocumentViews
- [`01-ontology.md`](01-ontology.md) § 4d — workbench vocabulary
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica-specific panels
- [`research/ui-ux-inspiration.md`](research/ui-ux-inspiration.md) — patterns from professional tools
- `specs/008-developer-experience` — detailed spec
