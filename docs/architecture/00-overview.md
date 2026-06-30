# 00 — LunCo Architecture Overview

> Status: Active · Audience: everyone — canonical entry point for architecture decisions
>
> **LunCo: virtual universe to design real space missions.**
> This is the canonical starting point for all architectural decisions.

## 1. What LunCo is: The CONOPS Platform

LunCo is not a game, nor is it a simple physics simulator. It is a **System-Level Robotics Co-Simulation Platform** designed for **Concept of Operations (CONOPS)** development.

While traditional simulators focus on isolated physics (how a wheel turns), **LunCo focus on the System-of-Systems**:
- **Behavioral Integrity**: How power, thermal, and software subsystems interact (Modelica/ROM).
- **Structural Integrity**: How the mission adheres to the engineering blueprint (SysML v2).
- **Visual & Scene Composition**: How complex environments are assembled from modular parts (OpenUSD).
- **Operational Integrity**: How multiple human and robotic agents collaborate in real-time (WebTransport).

## 2. The Architectural Tiers

LunCo is organized in three distinct tiers, ensuring that the **blueprint** (Documents) always drives the **execution** (Runtime).

```
┌──────────────────────────────────────────────────────────────────────┐
│                     TIER 3: Views  (Collaboration)                  │
│  Scene tree · 3D viewport · Diagram editor · Code editor · Plots     │
│  Mission timeline · Property editors · Telemetry Dashboards          │
└────────────────────────────────┬─────────────────────────────────────┘
                                 │ observes + emits ops
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│            TIER 2: Runtime  (CONOPS Projection)                      │
│  Live entity world · Physics solver · Modelica stepper · USD stage   │
│       Environment providers · Cosim propagation · Rendering          │
└────────────────────────────────┬─────────────────────────────────────┘
                                 │ projected from
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│           TIER 1: Documents  (Authority / Source of Truth)           │
│  USD composition · Modelica models · SysML structure · Mission events│
│         Connections · Environment configuration · Assets             │
└──────────────────────────────────────────────────────────────────────┘
```

- **Tier 1: Documents** are the authoritative source of truth. They are the "Digital Twin" blueprints that exist independently of the simulation.
- **Tier 2: Runtime** is a high-fidelity projection of those documents. It is the execution engine where co-simulation happens across multiple domains.
- **Tier 3: Views** are the collaborative windows into the digital universe. Edits made here are synchronised across all participants and saved back to the authoritative Tier 1 documents.

## 3. Native Collaboration

Collaboration is not an "add-on" feature; it is a **native architectural requirement**. 
- **CRDT-based Edits**: Every modification to a document (Modelica code, USD scene, SysML structure) is an operation replicated across the network via CRDTs.
- **State Synchronisation**: The Bevy ECS runtime is transparently replicated, allowing multiple users to see, drive, and inspect the same mission state simultaneously.
- **Authority Management**: Roles (Observer, Operator, Admin) define who can possess vessels or mutate mission parameters.

## 4. The Composition Layer (OpenUSD)

**OpenUSD is our composition format, not our simulation engine.** 
We use USD to describe **what the world looks like** and how its hierarchy is structured. We then "hook" simulation behaviors into this hierarchy:
- A USD Prim represents a component.
- A `lunco:model` attribute on that Prim points to a Modelica behavioral model.
- A `lunco:port` attribute defines where software or power connections attach.

This separation allows for seamless interoperability with NVIDIA Omniverse and other industrial 3D tools while maintaining engineering rigor.

## 5. Crate Layering

```
Apps (luncosim, lunco-sandbox, lunica)
   │
   ├── Networking (lunco-networking, replication, auth) ← Native Layer 2b
   │
   ├── Domain crates (Documents + Co-Simulation)
   │     lunco-modelica   lunco-usd   lunco-cosim   lunco-celestial
   │     lunco-environment   lunco-avatar   lunco-controller   ...
   │     lunco-scripting   ← rhai world-bridge + op-graph generators
   │          │                     │                        │
   │          ▼                     ▼                        ▼
   ├── Session / Twin layer
   │     lunco-workspace  ← editor session (open Twins + active doc/perspective)
   │     lunco-twin       ← Twin filesystem container + document-kind registry
   │     lunco-storage    ← I/O backend (read/write only)
   │          │
   │          ▼
   ├── Framework layer
   │     lunco-workbench  ← canonical UI scaffold, docking, perspectives, File menu
   │     lunco-ui         ← Widget toolkit + Document traits
   │     lunco-doc        ← Authority, diagnostics substrate, CRUD foundation
   │     lunco-doc-bevy   ← Bevy bridge: DocumentDiagnostics, open/new document
   │          │                     │
   │          ▼                     ▼
   └── lunco-core         ← f64 math foundation, Mutation<P> command substrate, fundamentals
```

## 6. Strategic Roadmap Orientation

We are moving from a **Sandbox** (physics validation) toward a **Mission Stack**:
1. **Core Co-Sim (Built)**: USD + Modelica + Physics integration.
2. **Native Collab (Built/Active)**: WebTransport + Replication (`lunco-networking` landed, RBAC policy substrate in place).
3. **Scripting (Built)**: `rhai` world-bridge + op-graph generators (`lunco-scripting`, `lunco-tools-rhai`).
4. **Experiments (Built)**: parameter sweeps + parallel runs (`lunco-experiments`).
5. **Mission Timeline (Planned)**: Scheduling, event graphs, and automated CONOPS rehearsal.
6. **HIL/SIL (Planned)**: Hardware/Software-in-the-loop validation for physical flight controllers.
7. **AI Integration (Planned)**: Agent-driven simulation for autonomous mission analysis.

## 7. Reading order for newcomers

The canonical newcomer reading order lives in **[`docs/README.md` → Reading order for newcomers](../README.md#reading-order-for-newcomers)** — follow it there (kept in one place so the paths don't diverge).

From this doc, the natural next reads are `01-ontology.md` (vocabulary), `10-document-system.md` (data model), then `12-api.md` (driving the sim). Note: `14-simulation-layers.md` is a design doc for a layer that is **largely not yet implemented** — read it as intent, not current behaviour.
