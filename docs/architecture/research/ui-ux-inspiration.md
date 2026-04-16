# UI/UX Inspiration — Professional Tool Analysis

> **Research document.** Surveys UI patterns from professional engineering,
> creative, and editor software to inform LunCoSim's UX design. Original
> analysis captured 2025-04, distilled 2026-04.
>
> Current-state UI design is in [`../11-workbench.md`](../11-workbench.md).

## The fundamental question

LunCoSim combines tasks that no single existing tool handles well:

| Task | Done well by |
|------|--------------|
| Build a 3D scene | Fusion 360, CATIA, Blender |
| Model physical subsystems | Dymola, Amesim, OMEdit |
| Run real-time simulation | Unity, Unreal |
| Plan missions | GMAT, STK |
| Program / script | VS Code, JetBrains |
| Collaborate live | Omniverse, Figma |

No tool covers all of them. LunCoSim must invent a UX that borrows the best
from each. This document catalogs those patterns.

## Dymola / OMEdit — Layered Model Editor

```
┌─────────────────────────────────────────────────────────┐
│ [Libraries Browser]    ← left dock (persistent)        │
├─────────────────────────────────────────────────────────┤
│  [📊 Diagram] [📝 Text] [🎨 Icon] [📄 Docs]  ← LAYERS  │
│                                                         │
│    Component block diagram or code editor              │
├─────────────────────────────────────────────────────────┤
│  [Plots] [Variables]  ← bottom (post-simulation)       │
└─────────────────────────────────────────────────────────┘
```

**Pattern**: One model at a time. Layer tabs switch the central view.
Side panels are peripheral. Great for single-system modeling.

**What to borrow**:
- Layered views of the same document (diagram / code / docs) — directly
  informs our Document System (multiple `DocumentView`s of one `Document`).
- Library browser as persistent left dock.
- Plots at the bottom after simulation.

**What NOT to copy**:
- Single-model focus. LunCoSim edits a whole colony, not one `.mo` file.

## Simcenter Amesim — Mode-Driven Workflow

```
SKETCH Mode      SUBMODEL Mode    PARAMETER Mode    SIMULATION Mode
 (build)          (configure)      (tune)            (run)
```

**Pattern**: Canvas always visible. Side panels and tools change per mode.
Users think "I'm sketching" vs "I'm tuning parameters" vs "I'm simulating."

**What to borrow**:
- The mode concept: fundamental to a task-oriented tool. Maps to LunCoSim's
  Workspace abstraction (Build / Simulate / Analyze / Plan / Observe).
- Canvas-always-visible: the 3D viewport as the document, never hidden
  behind modal dialogs.

## FreeCAD / CATIA — Workbenches

```
┌─────────────────────────────────────────────────────────┐
│ [Part Design] [Assembly] [FEM] [Path]  ← WORKBENCHES   │
├──────────┬────────────────────────────────┬─────────────┤
│ Scene    │        3D VIEWPORT             │ Properties  │
│ Tree     │     (primary focus)            │ Inspector   │
├──────────┴────────────────────────────────┴─────────────┤
│  Tasks / Parameters  ← bottom panel                     │
└─────────────────────────────────────────────────────────┘
```

**Pattern**: Switching workbenches changes the entire layout and toolbar.
3D viewport is always central. Panels adapt to current mode.

**What to borrow** (heavily):
- **The whole model.** This is the closest analog to what LunCoSim needs.
- Workbench switcher as a top-level tab row — one click reshapes the UI.
- Scene tree (left), properties (right), viewport (center) as default.
- Workbench name clarity: "Part Design" / "Assembly" / "FEM" tells you
  exactly what the current tool set is for.
- Source of our `lunco-workbench` crate name.

**Differences for LunCoSim**:
- Our workbenches are verbs (Build, Simulate, Plan), not document types.
- We have a stronger "simulation running" state that FreeCAD doesn't —
  our Simulate workspace maximizes viewport and minimizes chrome.

## Blender — Workspaces

```
┌──────────────────────────────────────────────────────────┐
│ Layout | Modeling | Sculpting | UV | Texture | Shading   │ ← workspaces
├────────────────────────────────────────────────────────┬─┤
│                                                        │ │
│                   3D Viewport                          │ │
│                   (per-workspace layout varies)        │ │
│                                                        │ │
└────────────────────────────────────────────────────────┴─┘
```

**Pattern**: Workspaces as top-level tabs. Each workspace has a completely
different layout. Fully customizable. Saved in the file.

**What to borrow**:
- The workspace tab row at the very top of the window.
- Per-workspace layout customization, persisted per user.
- Keyboard-centric workflow (tab to switch modes, N for properties, T for
  tools).

## VS Code — Generic Docking

```
┌──────────┬──────────────────────────────────────────────┤
│ 🗂️       │  editor_tab_1.rs | editor_tab_2.rs  [X][X]   │
│ Activity │                                              │
│ Bar      │           code editor area                   │
│          │                                              │
├──────────┴──────────────────────────────────────────────┤
│  Terminal / Problems / Output / Debug Console           │
└─────────────────────────────────────────────────────────┘
```

**Pattern**: Activity bar (left strip) for primary navigation. Side bar
shows the selected activity. Everything is a dockable panel. Fully
customizable. Command palette (Ctrl+P) for search + action.

**What to borrow**:
- **Activity bar** — primary navigation (Scene / Subsystems / Assets /
  Console / Search). VS Code pattern maps well to multi-domain tools.
- **Command palette** (Cmd+P) — universal keyboard-first action search.
  Killer feature for power users; will integrate with the
  `CommandRegistry` of each Space System.
- Detachable windows (VS Code supports drag-tab-out-to-new-window).
- Minimal default UI; progressive disclosure.

**What NOT to copy**:
- VS Code is file/buffer-centric. LunCoSim is scene/entity-centric — our
  "open documents" aren't a stack of files but a live 3D world.

## NVIDIA Omniverse / Nucleus — Collaborative USD

**Pattern**: Multiple DCC tools (Maya, Blender, 3dsMax) collaborate live
on a shared USD scene via the Nucleus protocol. Edit in any tool → changes
propagate to all. Op-based synchronization.

**What to borrow**:
- Op-based editing model — directly informs our Document System.
- USD-as-the-stage concept — our USD domain is the scene document.
- Ambition for live collaboration.

**What NOT to copy**:
- Single-document focus (one USD stage). LunCoSim has many document
  types (Modelica + USD + SysML + Mission), all cross-referencing.

## Fusion 360 — Cloud-Native CAD

**Pattern**: Feature-tree-driven parametric modeling. Cloud-stored documents.
Single-document-per-file. Strong timeline/history panel.

**What to borrow**:
- Feature tree in left dock — every modification tracked.
- Parametric / reversible editing — all operations recorded and rewindable
  (our Document System's op-based editing does the same).

## DAWs (Logic, Reaper, Ableton) — Timeline Editing

**Pattern**: Transport controls (play / stop / loop / record) prominent at
top. Timeline at bottom with automation lanes. Always-visible channel strip.

**What to borrow**:
- Transport controls at top of window: play / pause / time / speed.
- Timeline panel for Mission workspace (events, maneuvers, milestones).
- Automation / parameter curves over time.

## Game HUDs — Minimal Chrome During Play

**Pattern**: UI hides during gameplay. Radial right-click menus. Mini-map
overlay. Notification toasts. Pause menu for full controls.

**What to borrow** (for Simulate workspace):
- Auto-hide most docks when simulation is running — maximize viewport.
- Notification toasts for events (system failure, low power) without
  stealing focus.
- Radial context menus for in-viewport actions.

## The three levels of LunCoSim UI

Synthesizing the above, LunCoSim operates at three distinct UI levels:

```
Level 1: Colony View (primary, most of the time)
┌─────────────────────────────────────────────────────────┐
│  3D VIEWPORT (primary)                                  │
│  🚀 Rover  🏗️ Habitat  ☀️ Solar Array  🛰️ Comms       │
│                                                         │
│  Click any object → its Modelica editor overlay opens  │
└─────────────────────────────────────────────────────────┘

Level 2: Model Editor (Analyze workspace)
┌──────────┬───────────────────────────────┬──────────────┐
│ Library  │  Diagram + Code (split/tab)   │  Telemetry   │
│ Browser  │                               │  Properties  │
├──────────┴───────────────────────────────┴──────────────┤
│  Plots / Simulation Results                             │
└─────────────────────────────────────────────────────────┘

Level 3: Mission Dashboard
┌──────────┬───────────────────────────────┬──────────────┐
│ Colony   │  System Overview Map          │  Alerts      │
│ Tree     │  (power flow, comm links)     │  Health      │
├──────────┴───────────────────────────────┴──────────────┤
│  Live Telemetry (all subsystems)                        │
└─────────────────────────────────────────────────────────┘
```

Each level maps to a Workspace in the current design. See
[`../11-workbench.md`](../11-workbench.md) for the consolidated design.

## Rejected paths

### "Use bevy_workbench as-is"
*(The original 2025 decision; superseded.)*

Initial plan was to use `bevy_workbench` for docking, persistence, and
panel registration. This was adequate for early prototypes but revealed
architectural mismatches:

- `egui_tiles` egalitarian tile model — no first-class viewport concept,
  leading to the `CenterSpacer` kludge (transparent panel reserving space
  for the 3D scene).
- Left and right docks merge when the center panel closes (tile-tree
  behavior, not suited to CAD-style layouts).
- No detachable windows (multi-monitor engineering workflows need this).
- No app-defined workspace modes (only Edit/Play/Pause).

The `lunco-workbench` crate replaces `bevy_workbench` with a
SidePanel+CentralPanel root layout where the viewport is a structural
constant, not a tile.

### "Parse all Modelica Icon annotations"

For the Modelica diagram editor, one considered path was full-fidelity
parsing of each MSL component's `annotation(Icon(...))` block to render
exact Dymola-style shapes. Rejected for Phase 1 because:

- MSL has hundreds of components; parsing all annotations is large work.
- The ~20 components in our library can have their shapes hardcoded in
  `show_body()` (zigzag resistor, parallel-plate capacitor, etc.) in far
  less time.
- Annotation parsing can be added later for user-defined components
  without breaking the hardcoded-shape path.

See [`../20-domain-modelica.md`](../20-domain-modelica.md) for current
Modelica-editor status.

## Sources

- Dymola User Guide (Dassault Systèmes)
- OMEdit Manual (OpenModelica)
- FreeCAD documentation
- Simcenter Amesim workflow (Siemens)
- Modelica Specification v3.4 §18 (Annotations)
- VS Code UX patterns (Microsoft)
- Blender Manual — Workspaces and Editors
- NVIDIA Omniverse Nucleus documentation
- Fusion 360 user workflow analysis

## See also

- [`../11-workbench.md`](../11-workbench.md) — current workbench design
- [`../10-document-system.md`](../10-document-system.md) — the data model that supports live sync
- [`../20-domain-modelica.md`](../20-domain-modelica.md) — Modelica-specific UI design
