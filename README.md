# LunCo: virtual universe to design real space missions 🌎🚀🌚

[![Discord](https://img.shields.io/discord/1078754516390158416?color=7289da&label=Discord&logo=discord&logoColor=fff)](https://discord.gg/A6U3GdvQum)
[![Twitter](https://img.shields.io/twitter/follow/LunCoSim?style=social)](https://twitter.com/LunCoSim)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org/)

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
| **Robot Control** | **HIL / SIL / ROS2** | Native Hardware/Software-in-the-Loop support. Bridging logical intent to physical actuators. |
| **Mission Control** | **XTCE / MAVLink** | Standardized telemetry compatible with NASA OpenMCT, YAMCS, and professional ground stations. |

---

## 🛠 Key Capabilities

- **System-Level Co-Simulation**: Orchestrate multiple specialized engines (Modelica, Avian3D, GMAT) into a single cohesive mission scenario.
- **Planetary Scale Precision**: Built on a specialized **f64 (double precision)** spatial math foundation, ensuring absolute stability from millimetre-scale parts to lunar orbits.
- **Native Multi-User**: Architecture built from the ground up for collaboration. Every edit, command, and telemetry stream is replicated across the network.
- **Headless-First & AI-Ready**: Designed for automation. Scalable for massive parallel Monte Carlo analysis and end-to-end AI agent training.
- **Composition over Simulation**: We use OpenUSD to **compose** complex scenes from modular parts, then attach simulation behaviors via our multi-engine backend.

---

## 🏁 Fast Track

### 1. The Physics Sandbox
Validate robotics, suspension, and environment interactions in our collaborative 3D scene.

```bash
git clone https://github.com/LunCoSim/lunco-sim.git
cd lunco-sim
cargo run --release -p lunco-client --bin sandbox
```

### 2. Lunica (Engineering Workbench)
Focus on Modelica modeling, schematic diagramming, and subsystem analysis.

```bash
cargo run --bin lunica
```

---

## 🏗 Ecosystem & Governance

- **[Documentation Hub](docs/README.md)** — Usage guides and architectural deep-dives.
- **[Crates Index](docs/crates-index.md)** — A map of our 30+ specialized crates.
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
