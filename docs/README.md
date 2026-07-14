# LunCoSim Documentation

Welcome. This directory is the authoritative home for LunCoSim architecture,
design, and reference documentation.

## Application Guide

Documentation for each primary binary and tool in the workspace. The
**[Applications index](apps/README.md)** lists every runnable binary (primary,
utility, and dev) with launch commands and the shared CLI/API surface.

| App | Purpose |
|---|---|
| [**luncosim**](apps/luncosim/README.md) | Flagship lunar-mission simulator (full FSW/robotics/avatar stack) |
| [**Sandbox**](apps/sandbox/README.md) | Ground mobility and physics testing (+ headless [server](apps/sandbox/OPS.md)) |
| [**Lunica**](apps/lunica/README.md) | Modelica engineering workbench |
| [**Assets Manager**](apps/assets-manager/README.md) | Download and process workspace assets |

## Skills (AI agents & contributors)

Task-oriented runbooks in [`../skills/`](../skills/) — each triggers on a kind of
request and distills the docs below into a recipe plus the project-specific
gotchas. Point an agent (or yourself) at these before a hands-on task.

| Skill | Use it when you want to… |
|---|---|
| [**repo-map**](../skills/repo-map/SKILL.md) | Get oriented — repo layout, which binary to run, where a feature lives |
| [**build-usd-scene**](../skills/build-usd-scene/SKILL.md) | Author/edit the 3D world — load scenes, spawn, place, tune objects |
| [**author-scenario**](../skills/author-scenario/SKILL.md) | Write rhai behaviour — missions, waypoints, reactions, coordination |
| [**authoring-vessel-controllers**](../skills/authoring-vessel-controllers/SKILL.md) | Give a vessel a self-driving GNC / autopilot + manual handoff |
| [**compose-multidomain-twin**](../skills/compose-multidomain-twin/SKILL.md) | Assemble a full mission — USD + Modelica + cosim + rhai — into a Twin |
| [**author-tutorial**](../skills/author-tutorial/SKILL.md) | Build a guided interactive lesson / onboarding flow (rhai + HUD) |
| [**inspect-simulation**](../skills/inspect-simulation/SKILL.md) | Observe a running sim — read ports/variables, screenshot the viewport |
| [**run-modelica**](../skills/run-modelica/SKILL.md) | Run/compile/sweep Modelica models over the API |
| [**test-via-api**](../skills/test-via-api/SKILL.md) | Verify a change end-to-end via the API instead of asking the user to click |
| [**lunco-ui**](../skills/lunco-ui/SKILL.md) · [**lunco-theme**](../skills/lunco-theme/SKILL.md) | Build workbench UI panels / use the design tokens |

## Strategic Roadmap

We are evolving from a high-fidelity sandbox into a complete autonomous mission design stack.

| Milestone | Status | Description |
|---|---|---|
| **Space Robotics Core** | ✅ Foundation | Multi-domain co-simulation (USD + Modelica + Avian3D) with f64 precision. |
| **Real-world Validation** | 💭 Idea | HIL/SIL integration for hardware-in-the-loop validation (not yet specced). |
| **Industrial Interop** | 💭 Idea | NASA GMAT for orbital mechanics and **ROS2** for robotics control (not yet specced). |
| **Advanced Physics** | 📝 Planned | [**PINN Terramechanics**](../specs/025-terramechanics/) for high-fidelity soil interaction. |
| **Autonomous Missions** | 📝 Planned | [**Agent-Driven Sim**](../specs/033-agent-driven-simulation/) and [**Mission Replay/Audit**](../specs/020-world-state-and-replay/). |

## Architecture & Framework

| Path | Purpose |
|------|---------|
| [`principles.md`](principles.md) | Non-negotiable project principles (TDD, plugin-first, etc.) |
| [`crates-index.md`](crates-index.md) | Navigation guide for the workspace structure |
| [`tutorials/`](tutorials/README.md) | Step-by-step build guides — start here to author a scene/mission (USD + rhai + Modelica) |
| [`scripting-guide.md`](scripting-guide.md) | How to write rhai scenarios — beginner tutorial + full reference (verbs, sequencing, tools, persistence) |
| [`commands-reference.md`](commands-reference.md) | Every `#[Command]` — the full callable surface (HTTP / MCP / rhai `cmd()`), auto-generated from source |
| [`rhai-integration-design.md`](rhai-integration-design.md) | Rhai scripting design rationale + as-built reference |
| [**`architecture/README.md`**](architecture/README.md) | **Index of the design narrative** — start here for how LunCoSim is structured |
| [`architecture/render-decoupling.md`](architecture/render-decoupling.md) | The material is the boundary — domain crates state appearance *intent*; only `lunco-render-bevy` names `bevy_pbr`, so `--no-ui` links no wgpu/`bevy_render`/egui/winit |
| [`architecture/shader-layers-and-params.md`](architecture/shader-layers-and-params.md) | Shader looks — WGSL-reflected parameters and named texture layers; adding a parameter is editing a shader, not editing Rust |
| [`architecture/31-networking-and-state-sync.md`](architecture/31-networking-and-state-sync.md) | Multiplayer sync — the five replication planes (command/state/content/journal/presence), the wire, area-of-interest routing, policy-as-journal |
| [`architecture/terrain-substrate.md`](architecture/terrain-substrate.md) | Terrain height oracle — one `HeightSource` model from orbit to rover; USD layers, three channels, error-driven detail, solar-system scale |
| [`architecture/01-ontology.md`](architecture/01-ontology.md) | Terminology reference — Space System, Port, Connection, Attribute |
| [`reviews/`](reviews/) | Code reviews and the accepted security posture — **the project does not enforce access control** ([`TODO-rbac-not-enforced.md`](reviews/TODO-rbac-not-enforced.md)); trusted LAN only |
| `../specs/` | Detailed feature specifications (contracts for implementation) |
| `../crates/<crate>/README.md` | Per-crate quick-start (use this when you want to use a crate) |
| [`../scripts/perf/README.md`](../scripts/perf/README.md) | Performance profiling subsystem |
| [`architecture/research/`](architecture/research/) | Historical analysis, inspiration, rejected paths |

## Reading order for newcomers

1. **[`architecture/00-overview.md`](architecture/00-overview.md)** — what LunCoSim is, the three-tier model, crate layers
2. **[`principles.md`](principles.md)** — how we work (TDD, plugin-first, interop, documentation mandate)
3. **[`architecture/01-ontology.md`](architecture/01-ontology.md)** — vocabulary (Space System, Port, Connection, Typed Commands, etc.)
4. **[`architecture/10-document-system.md`](architecture/10-document-system.md)** — the foundational data model: Documents, DocumentOps, DocumentViews
5. **[`architecture/11-workbench.md`](architecture/11-workbench.md)** — UI/UX architecture: workspaces, panels, command palette
6. **[`architecture/12-api.md`](architecture/12-api.md)** — transport-agnostic API layer, typed commands, and queries
7. **[`architecture/13-twin-and-workflow.md`](architecture/13-twin-and-workflow.md)** — what a Twin is, save/load/workflow
8. **[`architecture/17-view-and-intent.md`](architecture/17-view-and-intent.md)** — camera systems and the 5-layer control model
9. Domain docs as relevant: `20-domain-modelica.md`, `21-domain-usd.md`, `22-domain-cosim.md`, `23-domain-environment.md`, `24-domain-sysml.md`

## Numbering convention

Architecture docs follow a numeric prefix:

| Range | Category |
|-------|----------|
| `00`–`09` | Foundation (overview, ontology) |
| `10`–`19` | Framework (document system, workbench, API, viewport, control) |
| `20`–`29` | Per-domain design (Modelica, USD, cosim, environment, SysML) |
| `30`–`39` | Infrastructure & Deployment (Wasm, web workers, networking & state sync, CI/CD) |
| `40`–`49` | Low-level subsystems (Asset IO, axes & units, logging) |
| `90`–`99` | Forward-looking / roadmap (collaboration, advanced features) |
| `research/` | Un-numbered historical / inspiration material |

## Writing new docs

- **Crate READMEs** are for "how do I use this crate right now."
- **Architecture docs** are for "how does LunCoSim fit together." Narrative, rationale.
- **Specs** are contracts — what a feature MUST do. Written before implementation.
- **App READMEs** are for "what is this binary and how do I run it."

One topic, one home. Avoid duplicating content — link instead.

## Doc lifecycle

- **Draft** → live review, prefix title with `> **Draft**`.
- **Active** → current design.
- **Superseded** → kept for history in `research/`.
- **Implemented** → doc describes a design that is now realized in code. Stays active.
