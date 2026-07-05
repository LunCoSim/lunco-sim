# LunCo: virtual universe to design real space missions 🌎🚀🌚

[![Discord](https://img.shields.io/discord/979381990220513320?style=flat-square&label=Discord&logo=discord&logoColor=white&color=5865F2)](https://discord.gg/A6U3GdvQum)
[![X](https://img.shields.io/badge/Follow-%40LunCoSim-000000?style=flat-square&logo=x&logoColor=white)](https://twitter.com/LunCoSim)
[![LinkedIn](https://img.shields.io/badge/LinkedIn-LunCoSim-0A66C2?style=flat-square&logo=linkedin&logoColor=white)](https://www.linkedin.com/company/luncosim/)
[![YouTube](https://img.shields.io/badge/YouTube-Subscribe-FF0000?style=flat-square&logo=youtube&logoColor=white)](https://www.youtube.com/@LunCoSim)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-brightgreen?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)

**LunCo** is an open-source, high-fidelity **robotics co-simulation platform** built for **System-Level Engineering** and **Concept of Operations (CONOPS)**. It bridges the gap between systems architecture, behavioral modeling, and real-time operations, providing the digital substrate for the next generation of space exploration.

[**Website**](https://lunco.space/) | [**Documentation Hub**](docs/README.md) | [**Join Discord**](https://discord.gg/A6U3GdvQum)

---

## 🛰 The Mission: Orchestrating CONOPS

Most simulators focus on isolated physics. **LunCo** focuses on the **System-of-Systems**. We simulate not just how a rover drives, but how it interacts with the power grid, obeys the flight software, adheres to the SysML blueprint, and contributes to the overall mission timeline.

### Strategic Product Pillars

| Pillar | Technology | The Value Proposition |
|---|---|---|
| **System Identity** | **SysML v2** | Definitive blueprints serving as the "Source of Truth" for system structure and requirements. |
| **Native Collaboration** | **WebTransport** | Built-in, multi-user engineering. Design, test, and operate in the same scene simultaneously. |
| **Scene Composition** | **OpenUSD** | Industrial-grade 3D interop with NVIDIA Omniverse. USD is our **world format**, not a sim engine. |
| **Behavioral Rigor** | **Modelica / ROM** | Multi-domain behavioral simulation (power, thermal, robotics) with high-fidelity dynamics. |
| **Mission Autonomy** | **rhai Scenarios** | Hot-reloadable per-entity flight software & declarative mission timelines — sense and command the world through the same API as the UI. |
| **Robot Control** | **HIL / SIL / ROS2** | Native Hardware/Software-in-the-Loop support. Bridging logical intent to physical actuators. |
| **Mission Control** | **XTCE / MAVLink** | Standardized telemetry compatible with NASA OpenMCT, YAMCS, and professional ground stations. |

---

## 🛠 Key Capabilities

- **System-Level Co-Simulation**: Orchestrate multiple specialized engines (Modelica, Avian3D, GMAT) into a single cohesive mission scenario.
- **Planetary Scale Precision**: Built on a specialized **f64 (double precision)** spatial math foundation, ensuring absolute stability from millimetre-scale parts to lunar orbits.
- **Native Multi-User**: Architecture built from the ground up for collaboration. Every edit, command, and telemetry stream is replicated across the network.
- **Headless-First & AI-Ready**: Designed for automation. Scalable for massive parallel Monte Carlo analysis and end-to-end AI agent training.
- **Scriptable Autonomy**: Attach hot-reloadable **rhai scenarios** to any entity — lifecycle hooks, sensing, and declarative mission timelines drive behavior with no recompile, through the same command/query API the UI and AI agents use. See the **[Scripting Guide](docs/scripting-guide.md)**.
- **Composition over Simulation**: We use OpenUSD to **compose** complex scenes from modular parts, then attach simulation behaviors via our multi-engine backend.

---

## 🏁 Fast Track

### ▶ Try it live — no install
Both windowed apps also run in your browser. These are **early preview builds — expect rough edges and missing features**:

- **[lunica.lunco.space](https://lunica.lunco.space)** — the Modelica engineering workbench
- **[sandbox.lunco.space](https://sandbox.lunco.space)** — the physics sandbox

### 💻 Run locally

```bash
git clone https://github.com/LunCoSim/lunco-sim.git
cd lunco-sim
```

Then launch the entry point that fits your goal (each also builds for the browser via `scripts/build_web.sh`):

### 1. LunCoSim — the Full Mission Simulator
The flagship: celestial bodies, ephemeris, solar-system-scale precision, and the complete flight-software / robotics / avatar stack.

```bash
cargo run --release -p luncosim
```

### 2. The Physics Sandbox
Validate robotics, suspension, and environment interactions in a collaborative 3D scene (windowed, or headless with `--no-ui`).

```bash
cargo run --release -p lunco-sandbox --bin sandbox
```

### 3. Lunica — the Engineering Workbench
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

## 🗺️ Strategic Roadmap

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
