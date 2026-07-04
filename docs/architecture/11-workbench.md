# 11 — Workbench (UI/UX Architecture)

> Status: Active · Audience: contributors building UI panels & perspectives
>
> How LunCoSim's user interface is organized: the workbench shell,
> perspectives, panels, viewport, command palette, detachable windows.
> Establishes the framework on top of which all domain-specific UI
> lives.
>
> **Terminology note.** Later sections of this doc (§4 onward) use
> "workspace" in its original Blender/CATIA sense — a layout preset.
> Since then LunCoSim has renamed that concept to **Perspective** and
> uses "Workspace" for the broader editor-session concept (VS Code
> sense). Read those sections with the translation in mind; the
> terminology table in §1 is canonical.
>
> **Shipped & canonical.** `lunco-workbench` is the workbench crate; it
> has replaced `bevy_workbench` and is now depended on by ~10 crates (luncosim,
> lunco-sandbox, lunco-sandbox-edit, lunco-usd, lunco-modelica, lunco-celestial,
> lunco-avatar, lunco-networking, …). The migration described in §13 is done.

## Contents

- [1. What "workbench" means here](#1-what-workbench-means-here)
- [2. Why we're building this](#2-why-were-building-this)
- [3. The standard layout](#3-the-standard-layout)
- [4. Workspaces](#4-workspaces)
- [5. Panel system](#5-panel-system)
- [6. Context-awareness](#6-context-awareness)
- [7. Command palette](#7-command-palette)
- [8. Detachable windows](#8-detachable-windows)
- [9. Window & layout persistence](#9-window--layout-persistence)
- [10. Theming and keybinds](#10-theming-and-keybinds)
- [11. Relationship to `lunco-ui` and domain crates](#11-relationship-to-lunco-ui-and-domain-crates)
- [12. Three LunCoSim apps, different compositions](#12-three-luncosim-apps-different-compositions)
- [13. Migration strategy (from bevy_workbench) — DONE](#13-migration-strategy-from-bevy_workbench--done)
- [14. Open questions](#14-open-questions)
- [Cross-domain URI handling](#cross-domain-uri-handling)
- [See also](#see-also)

## 1. What "workbench" means here

A **workbench** is the application shell of a LunCoSim app — the chrome around
the 3D world. It owns the root window layout, the perspective switcher, the
panel registry, the command palette, keybinds, and detachable window support.

Terminology mapping:

| Concept | Our term | Analogs |
|---------|----------|---------|
| App shell (layout engine) | **Workbench** (`lunco-workbench`) | Eclipse Workbench, VS Code workbench, Qt QMainWindow |
| Editor session (open Twins, active tab, recents) | **Workspace** (`lunco-workspace`) | VS Code Workspace, JetBrains Project |
| Task-specific UI configuration (layout preset) | **Perspective** (`lunco-workbench` trait) | Eclipse Perspective; Blender "workspaces" (same idea, different word) |
| A dockable UI element | **Panel** | VS Code sidebar view, Blender editor area |
| The 3D world | **Viewport** (structural, not a panel) | CAD 3D view |
| Primary navigation category | **Activity** | VS Code activity bar |
| A simulation unit on disk | **Twin** (`lunco-twin`) | A folder with `twin.toml` — recursive |

All defined in [`01-ontology.md`](01-ontology.md) § 4d–§4f.

> **Naming note.** Earlier drafts of this doc used "Workspace" for the
> layout-preset concept (CATIA/Blender naming). That collides with the
> VS Code sense of Workspace we needed for the editor-session type, so
> the trait was renamed to **Perspective** (Eclipse naming). Historical
> code or docs that say "Workspace trait" or `BuildWorkspace` refer to
> the Perspective concept.

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
   Cannot be closed or docked-over. The workbench contributes only the
   viewport's *visibility* (a perspective without a viewport panel hides 3D)
   and *rect* into `lunco_core::SceneViewport`; it never sets camera
   `is_active` — the single-authority reconciler in `lunco-usd-bevy` actuates
   that (see [`17-view-and-intent.md §6`](17-view-and-intent.md)).
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
"lunica" layout, consolidated.

| Slot | Default content |
|------|-----------------|
| Activity | Subsystems (active) |
| Side browser | Twin panel (Modelica section: MSL + Bundled Examples + Workspace), Files panel |
| Right | Modelica Inspector (params, variables), Component Palette |
| Bottom | Plots (time series), Console, Diagnostics |
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
// The Panel trait — lunco-workbench/src/panel.rs
pub trait Panel: Send + Sync + 'static {
    fn id(&self) -> PanelId;                 // newtype over &'static str
    fn title(&self) -> String;
    fn default_slot(&self) -> PanelSlot;     // Left / RightInspector / Bottom / Center / …
    // Render reads through the capability-narrowed `PanelCtx` (no raw `&mut World`);
    // mutations are queued via `ctx.defer(|world| { … })` and applied after paint.
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx);
    // Optional: closable(), transparent_background(), dynamic_title().
}
```

A panel's default slot derives from its `default_slot()` (and `id` substring conventions — e.g. an `id` containing `"inspector"` auto-docks right):

| Category | Default slot | Examples |
|----------|--------------|----------|
| Navigation | Left side | Twin panel, Files panel |
| Inspector | Right side | Properties, Modelica Inspector, Component Palette |
| Tool | Bottom | Diagram Editor |
| Output | Bottom | Console, Plots, Telemetry, Diagnostics |

Users can drag panels between slots, tab them together, collapse them,
or detach them (see § 8).

### 5a. Side-browser architecture — Twin panel + Files panel

The two Navigation-slot panels follow a Dymola/OMEdit-style split:

- **Twin panel** — what you browse "by name." One section per
  domain (`ModelicaSection`, future `UsdSection`, `SysmlSection`,
  `JuliaSection`), each section owning its own internal sub-grouping
  (e.g. Modelica's section nests Modelica Standard Library + Bundled
  Examples + Workspace as collapsing headers). Single tree per
  domain matches Dymola/OMEdit's Package Browser; nesting under a
  per-domain root scales as more domains land.
- **Files panel** — raw filesystem view of the active Twin / open
  Folder. Domain-agnostic.

Sections are pluggable via the `BrowserSection` trait + a registry
resource:

```rust
pub trait BrowserSection: Send + Sync + 'static {
    fn id(&self) -> &str;
    fn title(&self) -> &str;
    fn scope(&self) -> BrowserScope { BrowserScope::Models }
    fn default_open(&self) -> bool { true }
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx);
}

pub enum BrowserScope { Models, Files }
```

Domain plugins push their section impls into
`BrowserSectionRegistry` at `build()` time; the panel iterates the
registry per render, filtered by its `BrowserScope`. Sections emit
user actions (clicks, drags, context-menu choices) into a frame-
scoped `BrowserActions` outbox; a host system drains it and
dispatches.

This keeps the workbench crate domain-agnostic — `lunco-workbench`
ships `FilesPanel`, `TwinBrowserPanel`, and `FilesSection`, but
nothing Modelica-specific. Adding USD/SysML/Julia is one new
section per domain, no central edits.

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
- Commands applicable to the selected entity (discovered via the global command schema)

Integrates with the ontology's command schema pattern — the reflected metadata (name, fields, validation ranges, documentation) makes commands dynamically discoverable for humans and AI agents.

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

## 9. Window & layout persistence

LunCoSim follows **VS Code's two-tier split** for what survives a
restart:

- **Global, app-wide prefs** (theme, perf HUD, **default window
  geometry**) → one shared `~/.lunco/settings.json` via `lunco-settings`
  (§9b). No new file per feature.
- **Per-project volatile UI state** (active perspective, open-document
  list, and — in future — per-window layout) → **global storage keyed by
  a hash of the project path**, *not* written into the Twin folder:
  `~/.lunco/workspace-state/<fnv1a-hex>.json`. This is VS Code's
  `workspaceStorage/<hash>/` model — repos stay clean, no `.gitignore`
  churn, and personal layout never leaks into a shared project.
The `lunco-workbench::window_persistence` module restores the global `WindowGeometry` settings section before the main `Window` is created (default size is configured via `DEFAULT_WINDOW_{WIDTH,HEIGHT}` constants). Volatile UI state is managed via `lunco-workbench::workspace_state`, which loads a per-Twin `WorkspaceState` upon Twin activation and saves it when changes occur.

**Reconciliation.** Restore maps stored string ids back to the panels /
perspectives registered in *this* binary (sandbox and lunica ship
different sets) and **drops anything unknown** — `PanelId` /
`PerspectiveId` hold `&'static str`, so the live registry is the source
of truth, never the file.

**Deferred.** Free-form dock-tree fidelity (arbitrary user split
rearrangements) and document auto-reopen are follow-ups — see the crate
docs. Today restore re-applies the perspective preset and persists the
open-document paths; it does not yet replay per-domain open commands.

### 9a. Recents

Bounded recents lists (10 Twin folders, 20 loose files; most-recent-first,
dedupe-on-push) persist to `~/.lunco/recents.json` via the same
`user_config_dir()` helper. Loaded on startup by `WorkspacePlugin`,
saved when the in-memory list changes (JSON-fingerprint gated to
avoid disk writes on unrelated `WorkspaceResource` mutations). Atomic
write via temp-file + rename — a kill mid-write can't corrupt the
file. A corrupt file silently falls back to empty recents on next
boot.

### 9b. Settings (`lunco-settings`)

User preferences (perf HUD on/off, editor word-wrap, palette filters,
…) persist to a single `~/.lunco/settings.json` via the
`lunco-settings` crate. Layouts and recents stay separate by design
— layouts are TOML and high-structure, recents are high-churn list
state — but everything else funnels through `settings.json`.

The shape mirrors VS Code: one document, namespaced keys. Each
domain crate owns a typed slice that implements `SettingsSection`:

```rust
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq)]
struct PerfHudSettings { enabled: bool }

impl SettingsSection for PerfHudSettings {
    const KEY: &'static str = "perf_hud";
}

// In Plugin::build:
app.register_settings_section::<PerfHudSettings>();
```

After registration the slice is a normal `Resource`. The crate:

- Loads `settings.json` once on startup; deserialises each
  registered section out of its key (or seeds `Default` if absent).
- Persists on change via `Res::is_changed()` — per-section system
  re-serialises into the in-memory mirror, central
  `Last`-schedule flush writes the file.
- Treats absent or corrupt `settings.json` as "use defaults"; never
  panics. Atomic writes (write + rename) keep partial files from
  corrupting on kill.

UI surfaces the same resource three ways — a Settings-menu row
(`WorkbenchLayout::register_settings`), a typed `#[Command]` for
the API/script bus (e.g. `TogglePerfHud`), and direct mutation. All
three converge on the same persisted resource.

**Don't** invent per-feature JSON files for new settings. **Do**
keep the intentional exceptions separate, each for a documented
reason: `recents.json` (different lifetime / churn), the planned
`layouts.toml` (TOML schema, structural), and
`workspace-state/<hash>.json` (per-project, path-keyed, high-churn —
the VS Code `workspaceStorage` analog in §9).

#### 9b.1 Multi-level namespacing

Sections own a top-level `KEY` (e.g. `"perf_hud"`); any nested
structure happens inside the section's typed struct. To group
per-domain settings under a common prefix, use **dotted keys**:

```rust
impl SettingsSection for ModelicaNamingSettings {
    const KEY: &'static str = "modelica.naming";
}
impl SettingsSection for ModelicaCanvasSettings {
    const KEY: &'static str = "modelica.canvas";
}
```

On disk this is still a flat top-level map (`{"modelica.naming": {...},
"modelica.canvas": {...}}`) but the dotted convention groups related
sections in the Settings UI and matches VS Code's `editor.fontSize`
shape. No registry coordination — the dotted key is purely a naming
convention enforced by code review.

Each subsystem registers its own slice in its own `Plugin::build`
(domain crate, panel crate, even an external plugin). Adding a new
setting is one struct + one `register_settings_section` call; no
central allowlist to update.

#### 9b.2 Concrete sections

Domain examples (canonical KEYs to keep things consistent across
crates):

| KEY | Owner crate | Purpose |
|-----|-------------|---------|
| `ui` | `lunco-workbench` | Tab styling (italic for unsaved/Untitled, dirty-dot glyph), font sizes |
| `modelica.naming` | `lunco-modelica` | Class↔file rename behaviour (`Always`/`Ask`/`Never`), default-filename-from-class, tab-title source (class vs filename) |
| `modelica.canvas` | `lunco-modelica` | Diagram defaults (grid snap, default port side, auto-layout) |
| `modelica.canvas.animation` | `lunco-modelica` | Tween/pulse durations, ease curve, per-origin animation policy (Local / Api / Remote — see `20-domain-modelica.md` § 9c) |
| `modelica.canvas.add` | `lunco-modelica` | Auto-focus behaviour on AddComponent (None / Center / FitVisible), batch debounce window |
| `modelica.canvas.collab` | `lunco-modelica` | Remote cursor + selection visibility, user color, follow-user camera (multi-user precursor; deferred) |
| `modelica.editor` | `lunco-modelica` | Source editor word-wrap, tab width, auto-format-on-save |
| `perf_hud` | `lunco-workbench` | Spike threshold, plot rolling window, Twin overlay toggles |
| `journal` | `lunco-twin-journal` | Retention, blob commit policy (`twin.toml` may override) |

#### 9b.3 Per-Twin overrides (planned)

User-global `~/.lunco/settings.json` is the baseline. A per-Twin
`<twin>/.lunco/settings.json` layered on top would let projects
enforce conventions (e.g. a library Twin might pin
`modelica.naming.rename_class_renames_file = "Always"` while a
sandbox Twin keeps `"Never"`). Resolution order:

```
defaults  ←  ~/.lunco/settings.json  ←  <active_twin>/.lunco/settings.json
```

The active-Twin layer would be writable from the UI's "Workspace
settings" toggle (VS Code's pattern). Until implemented, only the
user-global file exists.

#### 9b.4 Settings UI gap

Today the only way to mutate `settings.json` is hand-editing the
file or wiring a typed `#[Command]` per knob. Schema-driven panels
(VS Code's "Settings" UI, Blender's Preferences window) are out of
scope for Phase α but slot in cleanly: each `SettingsSection`
implementation gains an optional `schema() -> SettingsSchema` method
returning `Vec<FieldDescriptor>` (label, doc-comment, default,
control kind), and a single panel walks all registered sections via
`Settings::iter()`. Hand-editing remains the escape hatch.

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
   │     - Shared widgets: TimeSeries, InspectorField
   │     - Re-exports: egui_plot
   │     (Node graphs / diagrams render on `lunco-canvas`)
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
lunco-sandbox = workbench + SpawnPalette + SceneTree + Inspector +
                        ModelicaInspector + 3D viewport
                        (sandbox editor with compact Modelica view)

luncosim              = workbench + all sandbox panels + MissionControl +
                        CelestialBrowser + full 3D world
                        (main client, everything enabled)

lunica    = workbench + CodeEditor + Diagram + PackageBrowser +
                        Telemetry + Graphs + LibraryBrowser
                        (Modelica modeling only, no 3D world needed)
```

Same workbench shell, different panel sets, different default workspaces.
`lunica` opens in the Analyze workspace; `lunco-sandbox`
in Build; `luncosim` in Observe with quick access to all others.

## 13. Migration strategy (from bevy_workbench) — DONE

> This migration is complete. `bevy_workbench` has been retired and
> `lunco-workbench` is the only workbench in `main`. The phase table below is
> kept for historical reference.

**Clean cutover, not parallel coexistence.** Each domain migrates its
panels when `lunco-workbench` can host them; the final commit removes
`bevy_workbench` entirely in one step.

Phases (each one a discrete commit or small commit series):

| Phase | Scope |
|-------|-------|
| 1 | Design complete (this doc). |
| 2 | Build `lunco-doc` crate (Document, DocumentOp, DocumentHost, undo/redo). |
| 3 | Build `lunco-twin` crate (Twin, TwinManifest, DocumentRegistry, CacheRegistry). |
| 4 | Build `lunco-workbench` crate skeleton (root layout, Panel trait, Workspace enum, Welcome Screen). |
| 5 | First panel migration: `SpawnPalette` moves to `lunco-workbench::Panel`. Proves the API. |
| 6 | Migrate remaining `lunco-sandbox-edit` panels. |
| 7 | Migrate `lunco-modelica` panels (Diagram, CodeEditor, Inspector, Telemetry, Graphs, PackageBrowser). |
| 8 | Migrate `lunco-ui` panels (MissionControl). |
| 9 | Add activity bar, status bar, transport controls, command palette, detachable windows. |
| 10 | Retire `bevy_workbench` dep — single commit removing it and all `setup_sandbox` hardcoded-scene code from the three binaries. |

Between phases, short-lived rough edges are acceptable — some panels
may look sparse while migrating, some default layouts may need tuning.
The reward is no transitional scar tissue in the final codebase.

### Migration discipline

- **Panels migrate whole, not partially.** When we start migrating a
  panel, we finish it before moving on. No half-migrated panel sits in
  the codebase.
- **No feature flags for old vs. new UI.** The workbench in `main` is
  the only workbench.
- **Commit per panel or per closely-coupled panel group.** Git history
  reads as "Migrated SpawnPalette to lunco-workbench Panel trait,"
  easy to revert if needed.
- **Examples ship with the apps.** The `setup_sandbox` code in each
  binary becomes an example Twin under `examples/` before being deleted
  from the binary.

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

## Cross-domain URI handling

The workbench owns a small URI dispatch layer so every domain crate
can expose navigable links (Documentation cross-references, resource
refs, external anchors) without reinventing the wheel.

- `UriRegistry` (Bevy `Resource`) holds scheme handlers. Each domain
  plugin registers its own on `build()`:
  - `lunco-modelica` → `modelica://Modelica.Blocks.Examples.PID` → drill-in.
  - Future `lunco-usd` → `usd://stage.usd@</World/Rover>`.
  - Future `lunco-sysml` → `sysml://package::Element`.
- `UriClicked` event carries `{ uri, resolution }`; domain observers
  match on `resolution.doc_kind` and fire their own commands
  (`OpenClass`, `OpenStage`, …).
- Docs-view renderer intercepts egui's `OutputCommand::OpenUrl`, routes
  known schemes through the registry, strips them so the OS browser
  doesn't try to open them. Unknown schemes (http/https/mailto) pass
  through.

OS-level registration (clicking a `modelica://` link in the browser
launches LunCoSim) is a later task — see task #90.

## See also

- [`10-document-system.md`](10-document-system.md) — panels as DocumentViews
- [`01-ontology.md`](01-ontology.md) § 4d — workbench vocabulary
- [`14-simulation-layers.md`](14-simulation-layers.md) — Twin/Run/Scenario control surface
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica-specific panels
- [`research/ui-ux-inspiration.md`](research/ui-ux-inspiration.md) — patterns from professional tools
- `specs/008-developer-experience` — detailed spec
