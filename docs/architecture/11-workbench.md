# 11 — Workbench (UI/UX Architecture)

> Status: Active · Audience: contributors building UI panels & perspectives
>
> How LunCoSim's user interface is organized: the workbench shell,
> perspectives, panels, and viewport. Command-palette and detachable-window
> sections below are explicit future design, not current behavior.
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
> `lunco-workbench` is the canonical workbench crate, depended on by ~10 crates
> (luncosim, lunco-luncosim, lunco-luncosim-edit, lunco-usd, lunco-modelica,
> lunco-celestial, lunco-avatar, lunco-networking, …).

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
- [12. Workbench apps and the headless launcher](#12-workbench-apps-and-the-headless-launcher)
- [14. Open questions](#14-open-questions)
- [Cross-domain URI handling](#cross-domain-uri-handling)
- [See also](#see-also)

## 1. What "workbench" means here

A **workbench** is the application shell of a LunCoSim app — the chrome around
the 3D world. It owns the root window layout, the perspective switcher, the
panel registry and keybind integration. A command palette and detachable-window
host remain planned capabilities (§7–8).

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

## 2. Why we're building this

The initial plan used `bevy_workbench` (an `egui_tiles`-based docking crate).
It revealed architectural mismatches for a 3D-canvas engineering app:

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
│ File  Edit  View  Window  Help   [Cmd+P]   [title]   ⏮▶⏸  [tabs]   │ ← merged title bar
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

1. **Merged menu/title bar (top)** — File (including Network) / Edit / View / Window / Help menus,
   the centered application title, transport pause/resume, perspective tabs,
   and native window controls. All controls share one full-height row and the
   title-bar height/control-size tokens from `lunco-theme`; the title remains a
   visual centre marker and never owns input.
2. **Workbench body (below the title bar)** — activity bar, side browser,
   viewport, properties/inspector, and the toggleable bottom panel are arranged
   around the persistent central viewport.
3. **Activity bar (far left)** — vertical strip of icons for primary
   navigation (Scene / Subsystems / Assets / Console / Search / Settings).
   Click an icon to open its browser in a slide-in panel.
4. **Slide-in side browser (between activity bar and viewport)** —
   content depends on the selected Activity. Resizable, collapsible.
5. **Viewport (center)** — the 3D world. **Structurally persistent** —
   always the central region of the window. Not a panel, not a tile.
   Cannot be closed or docked-over. The workbench contributes only the
   viewport's *visibility* (a perspective without a viewport panel hides 3D)
   into `lunco_core::SceneViewport`; it never sets camera `is_active` — the
   single-authority reconciler in `lunco-usd-bevy` actuates that (see
   [`17-view-and-intent.md §6`](17-view-and-intent.md)). The 3D renders
   full-window (`SceneViewport::rect` is `None`) and the chrome is layered on
   top of it — see § 3.1.
6. **Properties / Inspector (right)** — context-aware content for the
   current selection and workspace. See § 6.
7. **Bottom panel (toggleable)** — workspace-dependent: Console, Plots,
   Timeline, etc. Collapsible to zero height.
8. **Status bar (bottom strip)** — sim time, speed, selected entity,
   celestial body, FPS, and the latest status event. Warnings and errors
   identify their level and source in the strip; the history popup keeps
   events ordered and distinct in one responsive level/source/message/
   progress row inside a compact popup sized to roughly half the parent
   window (clamped to 420–960 logical px); each message column uses the
   remaining inner width after present progress and action controls; diagnostic
   rows expand or collapse their complete diagnostics when the row is clicked
   and expose that affordance
   through the row cursor and tooltip, while attention rows retain their typed
   action control. Active progress entries are included from the same StatusBus
   reader. Warning and error rows copy the unmodified message without depending
   on the window width;
   attention rows emit the owning typed action.

### 3.1 Rendering contract — how chrome and 3D share the window

The window is drawn by **two layered cameras**, not by tiling:

| Order | Camera | Role |
|-------|--------|------|
| 0 | scene `Camera3d` (`lunco_render::SceneCamera` intent) | renders the 3D **full-window**; **clears** the target |
| 1 | egui host `Camera2d` (`WorkbenchEguiHost`, holds `PrimaryEguiContext`) | paints the chrome on top with `ClearColorConfig::None` so it does not wipe the 3D |

The host is a separate camera because scene cameras are transient (USD spawns
them, `camera_switch` swaps them) while the egui context must be stable.

**Invariant: both cameras must share one main render texture.** Bevy keys a
target's main textures by `(target, usages, format, msaa)`. If the host's key
diverges, Bevy hands it a *private* texture that — because its clear config is
`None` — is **never cleared**, and it silently becomes an accumulation buffer:
chrome that stops being drawn (panels dropped by a perspective switch, a status
bar orphaned by a resize) stays baked in and keeps compositing over the live 3D,
frozen. Only a window resize clears it.

This shipped as a real bug: `SceneCamera` defaults to MSAA ×2 while a bare
`Camera2d` defaults to ×4, so Build's panels ghosted on top of the View
perspective. `sync_egui_host_msaa` (`lunco-workbench/src/viewport.rs`) copies the
scene camera's MSAA onto the host — change-driven, so it only runs when a scene
camera's MSAA actually moves or a camera is newly tagged. **Never give the egui
host its own MSAA / format / HDR setting** — for that camera these are not look
choices, they are the texture-sharing key.

When no window `Camera3d` is active at all (Design perspective, the Modelica
workbench, or the short handoff while a selected camera is being activated),
nothing clears the target — `render_layout` handles that case by painting a
full-window backdrop on egui's background layer. It checks the selected
camera's actual `Camera::is_active` state, not only the selection binding, so
direct perspective switches cannot expose the previous framebuffer.

Domain-owned diagram overlays remain leaf-local. A direct painter uses the
owning leaf's clip rectangle, and a separate egui `Area` constrains itself to
that same measured leaf rectangle. This keeps generated diagram content below
the workbench's menu, status, inspector, and window-control boundaries without
inventing per-window screen offsets. Non-interactive overlays do not become
input owners; modal behavior belongs to the shared modal host.

### 3.2 Render-time resource ownership

`WorkbenchLayout` is the owner of the dock tree, but `render_workbench` removes
that resource for the duration of the egui pass. Panel renderers and observers
must therefore emit typed navigation requests (`ActivatePerspective`,
`OpenTab`, `FocusPanel`, and related commands); they must not read or mutate
`WorkbenchLayout` from a render-time callback. The workbench drains deferred
layout requests before deferred tab requests so a request that changes both
the perspective and the active tab is applied in authored order.

UI projection plugins that read workbench resources install
`WorkbenchPlugin` when it is not already present. In particular,
`lunco_tutorial::TutorialPlugin` owns the launcher projection but relies on
the Workbench owner for `WorkbenchLayout` and `HelpAnchors`; its composition
contract is valid both standalone and inside an existing workbench host.

## 4. Workspaces

A workspace is a named task-specific UI configuration. LunCoSim ships with
five standard workspaces.

### Build — edit scenes and subsystems

Purpose: construct the colony. Place entities, wire subsystems, author
physical models.

| Slot | Default content |
|------|-----------------|
| Activity | Scene (active), Subsystems, Assets |
| Side browser | Spawn Palette, Inspector, Tools |
| Right | Entity List |
| Bottom | Collapsible |

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

### Registered but hidden perspectives

Registration controls whether a perspective can be activated by the typed API
or a guided tutorial. `Perspective::show_in_switcher()` controls only whether
it appears in the default title-bar switcher. Luncosim keeps `terrain_sculpt`
and `editor` registered for their authored flows, but hides them from
everyday navigation; this does not remove or duplicate either authoring mode.

### Guided presentation ownership

A guided tutorial may author a required perspective on its curriculum track.
While that tutorial is active, `WorkbenchLayout` temporarily owns that
presentation: every perspective entry point is constrained to the required
registered perspective. This includes the title-bar switcher, the typed
`ActivatePerspective` command, and internal callers because they all converge
on `activate_perspective`.

This is a runtime presentation constraint, not persisted workspace state. The
tutorial launcher clears it on completion, skip, scene teardown, or failure,
so ordinary user perspective switching resumes immediately afterward. The
purpose is to keep view-local `HelpAnchors` visible; a learner must not be
turned into a failed tutorial merely by selecting a full-screen perspective.

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
    // mutations emit typed events/resources and are applied after paint.
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx);
    // Optional: closable(), transparent_background(), dynamic_title().
}
```

The workbench owns panel-surface presentation. Ordinary dock and side-panel
bodies use the theme's translucent `DesignTokens::overlay_backdrop` by default;
`WorkbenchAppearanceSettings` can opt into the opaque mantle surface. Standard
`PanelCtx` content frames do not add a second backdrop, and the complete panel
leaf remains chrome for pointer routing in both modes.

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

### 5b. Shared browser search

Both navigation panels expose one transient `BrowserQuery` owned by the
workbench. The query is presentation state, so domain sections do not keep
parallel search resources or parse source during paint. Each section applies
the same case-insensitive substring contract to its own authoritative display
names, qualified paths, and authored file paths:

- a matching leaf keeps its authored parents visible and open while filtering;
- a matching parent keeps that parent's complete authored subtree available;
- generated ids are routing details, not the primary search or display label;
- an empty result is explicit, not an apparently empty or collapsed tree;
- the active query and transient row state are cleared on the active `TwinClosed`
  edge, so a replacement Twin cannot inherit stale navigation state.

The workbench owns only the field and the search affordance. USD, Modelica,
scene-closure, library, and raw-file sections remain responsible for their
own loading/error/empty rows and for emitting the existing typed navigation
actions.

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

**Status: planned, not implemented.** Ctrl+P/Cmd+P is intentionally unbound;
the current discoverability surface is the menu bar, including the grouped
View panel list and perspective switcher. The planned palette will fuzzy-search:

- Actions (workspaces, menu items, panel show/hide)
- Entities (navigate to Space System by name)
- Modelica models (open a `.mo` file)
- Parameters (jump to a named parameter in the Inspector)
- Commands applicable to the selected entity (discovered via the global command schema)

Integrates with the ontology's command schema pattern — the reflected metadata (name, fields, validation ranges, documentation) makes commands dynamically discoverable for humans and AI agents.

## 8. Detachable windows

**Status: planned, not implemented.** Panels currently remain in the main
workbench dock. There is no placeholder `PanelSlot` or compatibility path that
pretends a panel is detached; registered panels omitted from a preset use
`PanelSlot::Hidden` and open in the side browser through View.

The intended implementation is **egui multi-viewport**. Each detached panel becomes a
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
  geometry**) → one shared `<OS config dir>/lunco/settings.json` via `lunco-settings`
  (§9b). No new file per feature.
- **Per-project volatile UI state** (active perspective, open-document
  list, and — in future — per-window layout) → **global storage keyed by
  a hash of the project path**, *not* written into the Twin folder:
  `<OS config dir>/lunco/workspace-state/<fnv1a-hex>.json`. This is VS Code's
  `workspaceStorage/<hash>/` model — repos stay clean, no `.gitignore`
  churn, and personal layout never leaks into a shared project.
The `lunco-workbench::window_persistence` module restores the global `WindowGeometry` settings section before the main `Window` is created (default size is configured via `DEFAULT_WINDOW_{WIDTH,HEIGHT}` constants). Volatile UI state is managed via `lunco-workbench::workspace_state`, which loads a per-Twin `WorkspaceState` upon Twin activation and saves it when changes occur.

An explicit host launch may provide a one-shot
`WorkspaceStateRestorePolicy` initial perspective. The policy is consumed when
the first real Twin becomes active, so a scene-oriented launch can open its
View presentation without allowing a stale Design/Lunica choice to cover it.
Later Twin switches and ordinary launches continue to restore the persisted
per-Twin perspective.

**Reconciliation.** Restore maps stored string ids back to the panels /
perspectives registered in *this* binary (luncosim and lunica ship
different sets) and **drops anything unknown** — `PanelId` /
`PerspectiveId` hold `&'static str`, so the live registry is the source
of truth, never the file.

**Deferred.** Free-form dock-tree fidelity (arbitrary user split
rearrangements) and document auto-reopen are follow-ups — see the crate
docs. Today restore re-applies the perspective preset and persists the
open-document paths; it does not yet replay per-domain open commands.

### 9a. Recents

Bounded recents lists (10 Twin folders, 20 loose files; most-recent-first,
dedupe-on-push) persist to the shared config directory's `recents.json` via the same
`user_config_dir()` helper. Loaded on startup by `WorkspacePlugin`,
saved when the in-memory list changes (JSON-fingerprint gated to
avoid disk writes on unrelated `WorkspaceResource` mutations). Atomic
write via temp-file + rename — a kill mid-write can't corrupt the
file. A corrupt file silently falls back to empty recents on next
boot.

### 9b. Settings (`lunco-settings`)

User preferences (perf HUD on/off, editor word-wrap, palette filters,
…) persist to a single `<OS config dir>/lunco/settings.json` via the
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

UI surfaces the same resource three ways — a named Settings submenu row
(`WorkbenchLayout::register_settings_submenu`), a typed `#[Command]` for
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
| `download` | `lunco-settings` | Shared download concurrency, attempt budget, exponential backoff, and delay cap |
| `journal` | `lunco-twin-journal` | Retention, blob commit policy (`twin.toml` may override) |
| `input_bindings` | `lunco-controller` | Resolved keyboard and look-button bindings shared by avatar control, help, input injection, and Rhai tutorials |
| `ui.entity_list.grid_scope` | `lunco-luncosim-edit` | Active-Twin entity-tree visibility: `current` for the active `ActivePhysicsFrame`, or `all` for every mounted BigSpace grid |

#### 9b.3 Per-Twin overrides

User-global `<OS config dir>/lunco/settings.json` remains the baseline for user
preferences. Project-owned configuration lives in the Twin manifest, where
the project can enforce behavior that should travel with the project. The
missing-declared-assets prompt is the first implemented example:
`twin.toml [downloads] suppress_missing_prompt = true` hides the startup
consent window for that project. When the prompt is shown, only manifest entries
with `recommended = true` participate in startup consent; other declared
resources remain available from Settings ▸ Data & libraries without blocking a
scene. Omitted/false preserves the default of showing consent for recommended
entries. The popup checkbox and Settings ▸ Data & libraries edit this same
manifest-owned value.

The generic `[settings]` table in `twin.toml` is the per-Twin override
mechanism. It is a namespaced scalar map, so a new project-owned preference
does not require a new Rust field, settings section, or command. The active
Twin is the only reader/writer scope; closing it removes that scope before the
next Twin can become active. Omitted keys preserve the authored surface's
declared default.

The entity tree uses this generic boundary for its grid scope. The key
`ui.entity_list.grid_scope` accepts the text values `current` and `all`; an
omitted key means `current`. `current` resolves only the live
`ActivePhysicsFrame`, while `all` includes named entities from every mounted
BigSpace grid. The workbench Settings menu writes the active Twin's manifest
through `SetTwinSetting`, so the selection follows the Twin and is not a
global UI preference. An invalid value is shown as a tree error rather than
silently selecting a different grid.

For example, the shipped camera-status surface is on by default:

```toml
[settings]
"ui.camera_status" = true
```

An explicit `false` hides it. Rhai reads the same map with
`get_twin_setting("ui.camera_status")` and writes it through the generic
`set_twin_setting(...)` command; the runtime surface declares its key and
default in `runtime_surfaces.json`. This is project policy, not a user-global
diagnostic preference. User-global settings remain in
`~/.lunco/settings.json` for concerns such as perf HUDs, input visualisation,
theme, and window geometry.

#### 9b.4 Twin settings view

The workbench's **Twin settings** panel walks the active Twin's generic
`[settings]` map as a change-driven view model. It groups keys by namespace
and renders the stored scalar type directly: booleans, integers, numbers, and
text. Edits dispatch the generic `SetTwinSetting` command;
`ResetTwinSetting` removes a key so the owning authored surface's declared
default applies. There is no per-setting Rust list or duplicate policy layer.

The panel is Twin-scoped and is cleared on `TwinClosed`; workspace changes
rebuild the snapshot, so setting edits, reload, Twin replacement, and reset
use the same manifest/storage owner. The panel is intentionally limited to
the generic scalar map. Structured manifest sections such as `[usd]`,
`[downloads]`, and `[journal]` remain owned by their domain/workspace
surfaces, and user-global preferences remain in `settings.json`.

## 10. Theming and input bindings

- Theming via egui's visuals system. Built-in themes: Dark, Light, High
  Contrast. Per-user customization is a typed section in `settings.json`.
- Avatar and vessel input bindings are owned by `lunco-controller` as the typed
  `InputBindingsSettings` section in `<OS config dir>/lunco/settings.json`. The bundled
  defaults are the data in `assets/config/keybindings.json`; the same resolved
  resource feeds the live input map, help surfaces, input injection, and Rhai
  tutorial labels. There is no separate `keybinds.toml` registry.

Both are simple pass-throughs to egui and `bevy_workbench`-style registries;
no novel design.

## 11. Relationship to `lunco-ui` and domain crates

```
  Apps
   ├── Panel crates (domain-specific UI)
   │    lunco-modelica/ui   lunco-luncosim-edit/ui   lunco-mission/ui
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

### Modal and authored-policy boundary

`lunco-ui::modal` owns the single egui modal host and its queued outcomes. A
modal body may be custom-painted, but the host owns the scrim, focus, Esc
dismissal, button outcomes, and the `CloseModal` command. That command is
available through the same API/Rhai command funnel as every other typed
command, so dismissing a consent window never stops the simulation, download
tasks, or API transport.

The dataset consent body remains egui because the retained HTML/Rhai UI layer
does not yet provide the complete modal contract: a modal host with queue and
outcome lifecycle, dynamic repeated rows, checkbox/input state events,
viewport-aware scrolling, and typed command dispatch from the HTML surface.
Rhai remains the policy owner (`assets/scripting/policy/dataset_provisioning.rhai`):
it decides prompt versus skip from supplied facts;
Rust owns only shared transport, persistence, and UI mechanics. An eventual
HTML modal should add those host capabilities first, then move this body
without duplicating the current command or policy paths.

## 12. Workbench apps and the headless launcher

The two workbench binaries share the shell; the server binary reuses the same
simulation composition without linking the GUI:

```
luncosim       = workbench + SpawnPalette + SceneTree + Inspector +
                        ModelicaInspector + 3D viewport
                        (ground-physics simulator)

luncosim-server = the same simulation composition as `luncosim`, headless
                        and without winit/egui

lunica         = workbench + CodeEditor + Diagram + PackageBrowser +
                        Telemetry + Graphs + LibraryBrowser
                        (Modelica modeling only, no 3D world needed)
```

Same workbench shell for the two windowed apps, with different panel sets and
default perspectives. `lunica` opens in the Analyze perspective; `luncosim`
opens in the View perspective for an explicit scene launch.


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
- `specs/008-developer-experience` — detailed spec
