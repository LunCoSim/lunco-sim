# LunCoSim: The Collaborative Digital Twin for Space Systems

[![Discord](https://img.shields.io/discord/1078754516390158416?color=7289da&label=Discord&logo=discord&logoColor=fff)](https://discord.gg/A6U3GdvQum)
[![Twitter](https://img.shields.io/twitter/follow/LunCoSim?style=social)](https://twitter.com/LunCoSim)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org/)

**LunCoSim** is an open-source, high-fidelity **robotics co-simulation platform** designed to bridge the gap between systems engineering, behavioral modeling, and 3D operations. While its foundation is a universal robotics hub, its mission is focused on the unique challenges of the **Space Frontier**—planetary exploration, lunar infrastructure, and orbital assembly.

[**Website**](https://lunco.space/) | [**Documentation Hub**](docs/README.md) | [**Join Discord**](https://discord.gg/A6U3GdvQum)

---

## 🛰 The Challenge: Siloed Space Robotics

Modern space missions are stalled by fragmented workflows. Systems engineers work in SysML, control engineers in Modelica, robotics software teams in ROS, and 3D artists in USD. Round-tripping between these domains is lossy, manual, and expensive. There is no "Shared Source of Truth" for complex robotic agents.

## 🚀 The Solution: The Unified Co-Simulation Stack

LunCoSim unifies these industry standards into a single, real-time environment. We don't just "show" a rover; we orchestrate a **multi-engine co-simulation** of its **mathematical truth** (Modelica), its **structural requirement** (SysML v2), its **software stack** (ROS/FSW), and its **physical presence** (OpenUSD) in a collaborative workspace.

### Core Strategic Pillars

| Pillar | Industry Standard | The Value Proposition |
|---|---|---|
| **Structural Truth** | **SysML v2** | Definitive architectural blueprints that serve as the authoritative "Source of Truth" for every entity. |
| **Behavioral Rigor** | **Modelica** | Interpretable mathematical models for power, thermal, and life support—guaranteeing physics integrity. |
| **Visual Interop** | **OpenUSD** | Industrial-grade 3D composition compatible with NVIDIA Omniverse, Blender, and Pixar tools. |
| **Collaborative Ops** | **WebTransport** | Low-latency, multi-user engineering where teams design, test, and operate in the same authoritative scene. |
| **Standardized Telemetry** | **XTCE / MAVLink** | Mission-ready data streams compatible with NASA OpenMCT, YAMCS, and professional ground stations. |

---

## 🛠 Key Capabilities

- **Mathematical Integrity**: Native integration of the Modelica language via our `rumoca` engine—simulate complex subsystems with DAE-solver precision.
- **Planetary Scale Precision**: Built on a specialized **f64 (double precision)** spatial math foundation, ensuring absolute stability from millimetre-scale rover parts to the vastness of the lunar surface.
- **Autonomous & AI-Ready**: A **headless-first architecture** with a comprehensive HTTP/JSON API. Scalable for massive parallel Monte Carlo analysis, AI agent training, and automated verification.
- **Cross-Platform Delivery**: High-performance native execution for engineering workstations and WebGPU-powered browser builds for stakeholders.
- **Hotswappable Architecture**: A hotswappable plugin system allowing the swap of flight software, physics integrators, or environment models without a simulation restart.

---

## 🏁 Fast Track to Simulation

### 1. Run the Physics Sandbox
Validate rover chassis, suspension, and environment interactions in our USD-based sandbox.

```bash
git clone https://github.com/LunCoSim/lunco-sim.git
cd lunco-sim
cargo run --release -p lunco-client --bin sandbox
```

### 2. Launch Lunica (Engineering Workbench)
Focus entirely on Modelica modeling, schematic diagramming, and subsystem analysis.

```bash
cargo run --bin lunica
```

---

## 🏗 Ecosystem & Governance

LunCoSim is more than an application; it is a modular multi-crate ecosystem designed for long-term architectural continuity.

- **[Documentation Hub](docs/README.md)** — Authoritative guides on architecture, documents, and simulation layers.
- **[Crates Index](docs/crates-index.md)** — A map of our 30+ specialized crates (Core, Celestial, Mobility, FSW, USD).
- **[Principles](docs/principles.md)** — Our non-negotiable mandates: TDD-First, Headless-First, and Tunability.
- **[Technical Specifications](specs/)** — Granular implementation plans for the roadmap ahead.

---

## 🤝 Community & Vision

LunCoSim is built by a global community of engineers, researchers, and space enthusiasts. We are dedicated to making professional-grade space engineering tools accessible to everyone.

- [**Discord**](https://discord.gg/A6U3GdvQum) | [**Twitter**](https://twitter.com/LunCoSim) | [**LinkedIn**](https://www.linkedin.com/company/luncosim/) | [**YouTube**](https://www.youtube.com/@LunCoSim)

**Want to join the mission?** We follow a strict TDD and Documentation mandate to ensure simulation reliability. [**Apply to the core team**](https://tally.so/r/3jX6aE).
