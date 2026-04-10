# LunCoSim: Collaborative Space Engineering for Everyone

LunCoSim is an open-source, collaborative digital twin of the solar system. Built for high-fidelity space mission planning, engineering, and training, it enables multiple participants to design and operate complex systems in a shared 3D environment.

[**Join our Discord**](https://discord.gg/A6U3GdvQum) | [**Follow us on Twitter**](https://twitter.com/LunCoSim) | [**Website**](https://lunco.space/)

---

## 🚀 The 5 Pillars of Our Digital Twin

Empowering everyone to architect the future of space, LunCoSim delivers professional-grade simulation fidelity at every scale—from individual rovers to entire lunar cities—through five core technological pillars:

### 1. 🤝 Shared 3D Workspaces
Real-time collaborative engineering where multiple participants interact in the same high-fidelity environment. Whether you are driving a rover, monitoring telemetry, or managing orbital maneuvers, the simulation remains synchronized and authoritative.

### 2. 🧱 Modular Digital Twins (USD)
We leverage **Universal Scene Description (USD)** for 3D world composition. This ensures industrial-grade interoperability with Pixar USD and NVIDIA Isaac Sim, allowing you to author once and simulate everywhere.

### 3. 📐 Mathematical Rigor (Modelica)
Native integration of **Modelica** provides high-fidelity 1D physics for critical subsystems. Calculate power draw, thermal rejection, and life support levels with professional-grade mathematical certainty.

### 4. 🔗 Structural Truth (SysML v2)
We use the next-generation **SysML v2** standard as our structural blueprint. Every entity and its state is defined by an engineering model, serving as the ultimate "Source of Truth" for the simulation.

### 5. 📡 Standardized Mission Control (XTCE)
Monitor your missions using the **XML Telemetry and Command Exchange (XTCE)** standard. Compatible with professional tools like YAMCS and NASA OpenMCT, LunCoSim provides real-time, standardized hardware telemetry.

---

## 🛠 Features & Capabilities

- **Desktop & Browser Support**: High-performance native execution on Linux/Windows and accessible via web browsers (links coming soon).
- **Headless-First Architecture**: Core simulation logic is decoupled from rendering, enabling high-speed automated validation and massive parallel Monte Carlo analysis.
- **Planetary Precision (f64)**: All spatial math and physics use double-precision floating point (f64) for absolute stability across the scales of a lunar base or the entire solar system.
- **Hotswappable Plugins**: A highly dynamic architecture where every feature — from flight software to physics integrators — is a modular plugin that can be swapped without a restart.

---

## 🚦 Getting Started

### Prerequisites
- [Rust Toolchain](https://rustup.rs/) (Stable)
- Git

### Fast Track
Clone the repository and run the simulation sandbox:

```bash
git clone https://github.com/LunCoSim/lunco-sim.git
cd lunco-sim
cargo run --release -p lunco-client --bin rover_sandbox
```

---

## 🏗 Project Architecture

LunCoSim is built as a modular multi-crate workspace:

- `lunco-core`: Headless simulation core and base traits.
- `lunco-celestial`: Planetary mechanics, SOI handling, and environments.
- `lunco-physics`: Precision f64 physics integration via Avian3D.
- `lunco-telemetry`: XTCE-based monitoring and signaling.
- `lunco-fsw`: Flight software and subsystem logic.
- `lunco-client`: Visual client for Desktop and Browser (WASM).
- `lunco-avatar`: User presence, perspective, and authority management.

---

## 🗺️ Roadmap & Detailed Specifications

For those interested in the deep technical details, future architecture, and granular implementation plans, we maintain a comprehensive set of specifications in the [**specs/**](specs/) directory.

---


## 📜 Legacy Support
The original Godot 4 implementation of LunCoSim is still available on the [**main-godot4**](https://github.com/LunCoSim/lunco-sim/tree/main-godot4) branch. 

---

## 🌐 Community & Links

-   [Discord Server](https://discord.gg/A6U3GdvQum)
-   [Twitter](https://twitter.com/LunCoSim)
-   [LinkedIn](https://www.linkedin.com/company/luncosim/)
-   [YouTube Channel](https://www.youtube.com/@LunCoSim)

**Want to contribute?** We follow a strict TDD and Documentation mandate. [Apply here](https://tally.so/r/3jX6aE) to join our team! Check our [Constitution](.specify/memory/constitution.md) and [Detailed Specifications](specs/) to understand our core principles and roadmap.
