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

### 3. 📐 Mathematics & Engineering Physics Rigor (Modelica)
Native integration of **Modelica** provides high-fidelity 2D and 3D engineering physics emulation for critical subsystems. Compute the power draw, thermal rejection, and life support levels with professional-grade, rigorous and realistic mathematical models.

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
- **In-Scene Editing**: Spawn rovers, props, and terrain directly in the running simulation. Transform and inspect objects with gizmo tools.

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

### USD Rover Sandbox (with Editing Tools)

The USD-based rover sandbox loads the entire scene — rovers, terrain, and camera — from declarative `.usda` files. It includes an in-scene editing toolkit:

```bash
cargo run --release -p lunco-client --bin rover_sandbox_usd
```

**Editing Tools:**
- **Spawn Palette** — Click or drag rovers, balls, ramps, and walls into the scene
- **Transform Gizmo** — Select objects and use **G** (translate) / **R** (rotate) to manipulate them
- **Inspector Panel** — View entity parameters (position, mass, physics)
- **Undo** — **Ctrl+Z** to revert spawns and moves
- **Escape** — Cancel current operation

See [USD System Documentation](docs/USD_SYSTEM.md) for rover definitions, scene composition, wheel types, and the full editing tools architecture.

---

## 🏗 Project Architecture

LunCoSim is built as a modular multi-crate workspace:

- **`lunco-core`**: Headless simulation core, CommandMessage architecture, and base traits.
- **`lunco-celestial`**: Planetary mechanics, SOI handling, and environments.
- **`lunco-mobility`**: Rover locomotion — differential drive, Ackermann steering, raycast suspension.
- **`lunco-fsw`**: Flight software — digital ports, wires, and subsystem logic.
- **`lunco-avatar`**: User presence, camera modes (freeflight, orbit, spring arm), and possession.
- **`lunco-controller`**: Input translation — keyboard/intent to CommandMessage.
- **`lunco-sandbox-edit`**: In-scene editing — spawn palette, transform gizmo, inspector, undo.
- **`lunco-usd`**: USD integration — 3-plugin pipeline for visual, physics, and simulation mapping.
- **`lunco-client`**: Visual desktop client — combines all subsystems into the sandbox binary.
- **`lunco-modelica`**: Modelica simulation — AST-based parsing, component diagrams, workbench UI.
- **`lunco-ui`**: Reusable UI mechanisms — WidgetSystem, node graphs, 3D world-space UI.
- **`lunco-attributes`**: Reflection-based attribute system for SysML v2 alignment.

---

## 🎨 UI Architecture

All UI panels are **entity viewers** — they watch a selected entity and render its data. The same panel works in a standalone workbench, a 3D overlay, or a mission dashboard.

```
                    Entity (ModelicaModel, FswConfig, etc.)
                              │
           ┌──────────────────┼──────────────────┐
           ▼                  ▼                  ▼
     DiagramPanel      CodeEditorPanel    TelemetryPanel
     (egui-snarl)      (text editor)      (params/inputs)
```

`WorkbenchState.selected_entity` is the **selection bridge** — any context (library browser, 3D viewport click, colony tree) can set it to open the editor for any entity.

### Panel Layout

Panels are dockable, tabbable, resizable, and persist across sessions:

| Panel | Position | Purpose |
|-------|----------|---------|
| Library Browser | Left dock | File navigation, drag `.mo` files |
| Code Editor | Center tab | Source code editing, compile & run |
| Diagram | Center tab | Component block diagram (egui-snarl) |
| Telemetry | Right dock | Parameters, inputs, variable toggles |
| Graphs | Bottom dock | Time-series plots |

See [UI/UX Architecture Research](docs/research-ui-ux-architecture.md) for the full analysis of professional tools and our design decisions.

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
