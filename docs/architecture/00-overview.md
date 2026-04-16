# 00 — LunCoSim Architecture Overview

> **Read this first.** Everything else in `docs/architecture/` elaborates on sections here.

## 1. What LunCoSim is

A **3D-canvas systems-engineering tool** for designing, simulating, and operating
lunar colonies and space missions.

It is not a game. It is not a CAD program. It is not a Modelica IDE.
It combines patterns from all three, plus operations-simulator patterns from
STK / GMAT and collaborative-editing patterns from NVIDIA Omniverse.

The user experience spans multiple tasks:

| Task      | What the user does |
|-----------|--------------------|
| Build     | Places buildings, rovers, habitats; wires subsystems |
| Simulate  | Runs physics + behavioral models; watches the colony live |
| Observe   | Pilots, flies around, inspects subsystem telemetry |
| Plan      | Schedules missions, events, maneuvers |
| Debug     | Replays events, tweaks parameters, chases failures |
| Share     | Exports models, collaborates with other users |

Each task needs a different UI configuration — but all share the same underlying
data and physics model.

## 2. The architectural tiers

LunCoSim is organized in three distinct tiers. Every contributor should
understand which tier a piece of code belongs in.

```
┌──────────────────────────────────────────────────────────────────────┐
│                     TIER 3: Views  (UI panels)                       │
│  Scene tree · 3D viewport · Diagram editor · Code editor · Plots     │
│  Parameter inspector · Mission timeline · Property editors · ...     │
└────────────────────────────────┬─────────────────────────────────────┘
                                 │ observes + emits ops
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│            TIER 2: Runtime  (Bevy ECS projection)                    │
│  Live entity world · Physics solver · Modelica stepper · USD stage   │
│       Environment providers · Cosim propagation · Rendering          │
└────────────────────────────────┬─────────────────────────────────────┘
                                 │ projected from
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│           TIER 1: Documents  (persistent, canonical)                 │
│  USD scene · Modelica models · SysML structure · Mission events      │
│         Connections · Environment configuration · Assets             │
└──────────────────────────────────────────────────────────────────────┘
```

- **Documents** (Tier 1) are the source of truth. They persist to disk,
  version in git, export to external tools, and survive app restarts.
- **ECS runtime** (Tier 2) is a live projection of the documents plus
  derived simulation state. It's transient — a cold-started simulator
  rebuilds it from documents every time.
- **Views** (Tier 3) observe documents and runtime, render projections,
  and emit user edits as typed operations back to documents.

This pattern is standard in professional engineering SW:

| Tool             | Tier 1 (document)     | Tier 2 (runtime)        | Tier 3 (views)           |
|------------------|-----------------------|-------------------------|--------------------------|
| Dymola           | `.mo` files           | BDF solver              | Diagram, Code, Plots     |
| Fusion 360       | `.f3d`                | Parametric kernel       | 3D view, Feature tree    |
| Omniverse        | USD stage             | RTX renderer            | Maya, Blender, USDview   |
| **LunCoSim**     | USD + Modelica + ...  | Bevy ECS                | lunco-workbench panels   |

### Why this matters

LunCoSim is currently **ECS-first** — the Bevy world is treated as the source
of truth. That works for a game where the world is spawned once and runs.
For a simulator that users *edit, save, collaborate on, and export*, this is
wrong. Documents must be the source of truth; ECS is derived.

The **Document System** (see [10-document-system.md](10-document-system.md))
is how we make this architectural shift. It's the single most important
foundational design in the project.

## 3. The domains

Each domain in LunCoSim is a document type with its own editing and
simulation semantics:

| Domain        | Document     | Crate(s)                          | Status  |
|---------------|--------------|-----------------------------------|---------|
| Scene / world | USD          | `lunco-usd`, `lunco-usd-*`        | active  |
| Behavior      | Modelica     | `lunco-modelica`, `rumoca` (fork) | active  |
| Co-simulation | Connections  | `lunco-cosim`                     | active  |
| Environment   | Bodies/env   | `lunco-environment`, `lunco-celestial` | active  |
| Structure     | SysML        | (future) `lunco-sysml`            | planned |
| Missions      | Event graph  | (future) `lunco-mission`          | planned |
| Collaboration | Op stream    | (future) `lunco-collab`           | horizon |

Cross-document references are first-class: a Modelica model is attached to a
USD prim; a SysML block specifies a Modelica realization; a mission event
targets an entity in USD.

## 4. Crate layering

```
Apps
  (rover_sandbox_usd, lunco_client, modelica_workbench, future ones)
   │
   ├── Panel crates (domain-specific UI)
   │     lunco-modelica/ui    lunco-sandbox-edit/ui    lunco-mission/ui
   │          │                     │                        │
   │          ▼                     ▼                        ▼
   ├── Domain crates (documents + simulation)
   │     lunco-modelica   lunco-usd   lunco-cosim   lunco-celestial
   │     lunco-environment   lunco-avatar   lunco-controller   ...
   │          │                     │                        │
   │          ▼                     ▼                        ▼
   ├── Framework layer
   │     lunco-workbench  ← app scaffold: layout, workspaces, palette, detach
   │     lunco-ui         ← widget toolkit + Document/DocumentView traits
   │          │                     │
   │          ▼                     ▼
   ├── lunco-core         ← pause, time warp, SelectableRoot, fundamentals
   │
   └── External
         bevy · bevy_egui · egui_tiles · egui-snarl · egui_plot · avian3d · rumoca
```

Layers go strictly downward. A crate only depends on crates in its own layer
or below. This keeps the dep graph acyclic and the conceptual model clean.

**Never pull domain knowledge into framework crates.** `lunco-workbench` knows
nothing about Modelica or USD. `lunco-ui` knows nothing about balloons or
solar panels. Domain knowledge lives in domain crates.

## 5. UI/UX design principles

Details live in [11-workbench.md](11-workbench.md). The high-level principles:

1. **The 3D world is the document.** Viewport is always central, always visible.
   Panels are scaffolding around it.
2. **Workspaces, not just panels.** One click at the top reshapes the whole UI
   for the current task (Build / Simulate / Analyze / Plan / Observe).
3. **Panels are reusable entity viewers.** Same `DiagramPanel` works in the
   workbench, as a 3D overlay, or in a mission dashboard.
4. **Context-awareness.** Selecting an entity prioritizes panels relevant to
   it. The Inspector is a live reflection of the current selection.
5. **Edit anywhere, see everywhere.** A parameter change in a form updates
   the diagram. A component drag in the diagram updates the source text.
   A source edit updates all views. This is the Document System at work.
6. **Progressive disclosure.** Default layout is minimal. Power users hide
   chrome and drive from keyboard + command palette.
7. **Detachable windows.** Any panel can pop out to its own OS window for
   multi-monitor workflows. First-class feature, not a hack.

## 6. Cross-cutting concerns

### Pause, time warp, and paused entities

Covered in depth in [22-domain-cosim.md](22-domain-cosim.md). Summary:

- Per-entity `SimPaused` marker component (in `lunco-core`)
- Global Avian pause via `Time<Physics>::pause()`
- Global Modelica pause via (future) resource
- Speed control via `Time<Virtual>::set_relative_speed()`

Documents are editable regardless of pause state.

### Undo / redo

Handled by the Document System. Every edit is a typed operation with a
defined inverse. See [10-document-system.md](10-document-system.md).

### Save / load

Per-domain file formats: `.mo`, `.usda`, `.sysml`, etc. A "project" is a
bundle of documents in a directory structure. No custom monolithic save format.
This makes LunCoSim interoperable with existing tools (you can edit a `.mo`
file in Dymola, edit a `.usda` file in USDView, etc.).

### Collaboration (future)

The Op-based edit model sets us up for Nucleus-tier network sync later.
See [30-collab-roadmap.md](30-collab-roadmap.md) when it's written.

## 7. Roadmap orientation

At time of writing (April 2026):

- ✅ Tier 2 (ECS runtime) is mature. Physics, cosim, Modelica, USD loading all work.
- ✅ Tier 3 (views) has most panels, built on `bevy_workbench` (being retired).
- ❌ Tier 1 (documents) not formalized — source of truth is ECS today.

The arc is:
1. **Phase 1:** build `lunco-workbench` (app scaffold) to replace bevy_workbench.
   Simultaneously design the Document System. (~8 weeks)
2. **Phase 2:** implement the Document System in `lunco-ui`. Migrate one
   domain (Modelica) end-to-end. (~4–6 weeks)
3. **Phase 3:** migrate USD to the Document System. (~3 weeks)
4. **Phase 4:** add SysML, Mission, other domain documents as needed.
5. **Phase N:** collaboration layer.

Each phase delivers standalone value. The documents system is high-leverage
but not a prerequisite for features — work can parallelize.

## 8. Where to look next

- **Framework design:** [10-document-system.md](10-document-system.md) and [11-workbench.md](11-workbench.md)
- **Domain specifics:** 20-domain-*.md files (written when each domain migration begins)
- **Historical research:** [99-research/](99-research/) contains inspiration and rejected-path writeups
- **Crate-level quick starts:** each crate has its own `README.md` focused on
  "how do I use this crate right now," not architecture.
