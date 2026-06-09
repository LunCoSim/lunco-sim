# 00 — LunCo Architecture Overview

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
Apps (sandbox, lunica, lunco_client)
   │
   ├── Networking (lunco-networking, replication, auth) ← Native Layer 2b
   │
   ├── Domain crates (Documents + Co-Simulation)
   │     lunco-modelica   lunco-usd   lunco-cosim   lunco-celestial
   │     lunco-environment   lunco-avatar   lunco-controller   ...
   │          │                     │                        │
   │          ▼                     ▼                        ▼
   ├── Framework layer
   │     lunco-workbench  ← UI scaffold, docking, perspectives
   │     lunco-ui         ← Widget toolkit + Document traits
   │     lunco-doc        ← Authority, undo/redo, CRUD foundation
   │          │                     │
   │          ▼                     ▼
   └── lunco-core         ← f64 math foundation, CommandMessage, fundamentals
```

## 6. Strategic Roadmap Orientation

We are moving from a **Sandbox** (physics validation) toward a **Mission Stack**:
1. **Core Co-Sim (Built)**: USD + Modelica + Physics integration.
2. **Native Collab (Built/Active)**: WebTransport + Replication.
3. **Mission Timeline (Planned)**: Scheduling, event graphs, and automated CONOPS rehearsal.
4. **HIL/SIL (Planned)**: Hardware/Software-in-the-loop validation for physical flight controllers.
5. **AI Integration (Planned)**: Agent-driven simulation for autonomous mission analysis.

## 7. Reading order for newcomers

1. **[`01-ontology.md`](01-ontology.md)** — vocabulary (Space System, Port, Connection, etc.)
2. **[`10-document-system.md`](10-document-system.md)** — the data model foundation.
3. **[`12-api.md`](12-api.md)** — how to drive the simulation externally.
4. **[`14-simulation-layers.md`](14-simulation-layers.md)** — Twin/Scenario/Run/Model hierarchy.
5. **[`17-view-and-intent.md`](17-view-and-intent.md)** — the 5-layer control model.
6. **[`../../principles.md`](../principles.md)** — the project's non-negotiable rules.
