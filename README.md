# LunCoSim: open-source system-level simulation for space missions 🌎🚀🌚

[![Discord](https://img.shields.io/discord/979381990220513320?style=flat-square&label=Discord&logo=discord&logoColor=white&color=5865F2)](https://discord.gg/A6U3GdvQum)
[![X](https://img.shields.io/badge/Follow-%40LunCoSim-000000?style=flat-square&logo=x&logoColor=white)](https://twitter.com/LunCoSim)
[![LinkedIn](https://img.shields.io/badge/LinkedIn-LunCoSim-0A66C2?style=flat-square&logo=linkedin&logoColor=white)](https://www.linkedin.com/company/luncosim/)
[![YouTube](https://img.shields.io/badge/YouTube-Subscribe-FF0000?style=flat-square&logo=youtube&logoColor=white)](https://www.youtube.com/@LunCoSim)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-brightgreen?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)

**LunCoSim** is an open-source system-level co-simulation platform for space missions. It connects OpenUSD scene composition, equation-based Modelica behavior, rigid-body mechanics, terrain, mission policy, and API control so teams can study how a vehicle and its mission behave together.

[**Website**](https://lunco.space/) | [**Documentation Hub**](docs/README.md) | [**Join Discord**](https://discord.gg/A6U3GdvQum)

---

## 🛰 The mission: make system behavior executable

Subsystem studies answer important discipline questions. LunCoSim adds the mission context: the rover, environment, resources, controls, and operations can be exercised together so the interactions between them are visible before a design decision hardens.

### Current platform capabilities

| Capability | Current path | Engineering value |
|---|---|---|
| **System composition** | **OpenUSD** | Compose the vehicle, environment, ports, parameters, and authored mission topology from inspectable scene data. |
| **Equation-based behavior** | **Modelica + dynamic synthesis** | Generate connected electrical, thermal, and other continuous models from the assembled system while preserving physical connection semantics. |
| **Mechanics and environment** | **Rigid-body physics + terrain** | Exercise vehicle motion, contacts, route geometry, and environmental interactions in the same study. |
| **Mission policy** | **Rhai scenarios** | Express objectives, phases, autonomy, observations, and event responses without moving continuous control math into ad hoc scripts. |
| **Automation and control** | **HTTP/MCP/API boundaries** | Let engineers, scripts, and AI agents inspect state, issue commands, run scenarios, and compare outcomes through the same runtime boundary. |
| **Reproducible implementation** | **Rust + open source** | Inspect the runtime, authored assets, scenarios, tests, and generated-model path in one public repository. |

---

## 🛠 Key Capabilities

- **System-level co-simulation**: Connect specialized participants around one mission question instead of treating every subsystem result as an isolated answer.
- **Large-frame spatial precision**: Use an f64 spatial foundation for vehicle-scale geometry and large mission frames in one scene.
- **AI-ready operation**: Agents and scripts can inspect state, issue typed commands, run scenarios, and compare outcomes through the same boundary used by engineers.
- **Scriptable mission policy**: Attach authored Rhai scenarios for lifecycle hooks, sensing, objectives, and event-driven behavior. Continuous control laws remain in Modelica and engine kernels; see the **[Scripting Guide](docs/scripting-guide.md)**.
- **Inspectability**: Keep OpenUSD composition, generated Modelica source, runtime telemetry, diagnostics, and mission evidence connected to the study.

---

## 🏁 Fast Track

### ▶ Download and run locally
Use the **[latest release](https://github.com/LunCoSim/lunco-sim/releases/latest)** for a packaged build. The website’s **[download guide](https://lunco.space/download)** explains how to choose a release, start with a mission question, and trace the resulting study.

### 💻 Run locally

```bash
git clone https://github.com/LunCoSim/lunco-sim.git
cd lunco-sim
```

Then launch the entry point that fits your goal:

### 1. LunCoSim — the mission simulator
The production simulator loads composed USD scenes and provides ground physics,
rover/mobility tools, scene editing, mission operation, and the embedded
Modelica workbench.

```bash
cargo build -p lunco-luncosim --bin luncosim
target/debug/luncosim
```

After building, use `target/debug/luncosim` directly for launches, validation,
and scene tests. The former sandbox executable name is retired.

### 2. Lunica — the engineering workbench
Focus on Modelica modeling, schematic diagramming, and subsystem analysis.

```bash
cargo run --bin lunica
```

> **Driving it from code or an AI agent?** Launch any app with `--api` and drive it over HTTP/MCP — see the **[AI Agent Guide](AGENTS.md)** and the task-oriented **[skills](skills/)**.

---

## 🏗 Ecosystem & Governance

- **[Documentation Hub](docs/README.md)** — Usage guides and architectural deep-dives.
- **[Scripting Guide](docs/scripting-guide.md)** — Write hot-reloadable rhai scenarios & mission timelines.
- **[AI Agent Guide](AGENTS.md)** & **[Skills](skills/)** — Drive and extend LunCoSim from code or an AI agent.
- **[Crates Index](docs/crates-index.md)** — A map of our 60+ specialized crates.
- **[Principles](docs/principles.md)** — Our non-negotiable mandates: TDD-First, Headless-First, and Tunability.

---

## 🗺️ Planned integrations

The following items describe future integration work, not current website or runtime claims.

| Milestone | Status | Description |
|---|---|---|
| **System-Level Core** | ✅ Foundation | Multi-domain co-simulation (USD + Modelica + Avian3D) with f64 precision. |
| **Real-world Validation** | 📝 Planned | **HIL/SIL Integration** (Spec 027) for Hardware-in-the-loop validation. |
| **Industrial Interop** | 📝 Planned | **NASA GMAT** (Spec 022) for orbital mechanics and **ROS2** for robotics control. |
| **Advanced Physics** | 📝 Planned | **PINN-based Terramechanics** (Spec 025) for high-fidelity regolith interaction. |
| **Autonomous Missions** | 📝 Planned | **Agent-Driven Sim** (Spec 033) and **Mission Replay/Audit** (Spec 020). |

---

## 🤝 Community & Vision

LunCo is built by a global community of engineers and researchers making professional space engineering tools accessible to everyone.

- [**Discord**](https://discord.gg/A6U3GdvQum) | [**Twitter**](https://twitter.com/LunCoSim) | [**LinkedIn**](https://www.linkedin.com/company/luncosim/) | [**YouTube**](https://www.youtube.com/@LunCoSim)

**Want to join the mission?** [**Apply to the core team**](https://tally.so/r/3jX6aE).
