# LunCoSim × Bio-Cochlea Interface

A groundbreaking fusion of lunar surface mission simulation and bio-resonance technology, creating an unprecedented interface between space exploration and human cognitive enhancement.

## 🌙👂 Concept Overview

This project combines two cutting-edge domains:

### **LunCoSim Integration**
- Real-time lunar surface mission simulation
- Rover telemetry and environmental monitoring
- Multiple mission scenarios (Apollo 17, Artemis 3, Swarm operations, Night survival)
- Hardware-in-the-loop simulation capabilities

# LunCoSim: Everyone can do Space

LunCoSim is an open-source, collaborative space simulation platform designed for planning lunar & and space missions, engineering complex systems, and training future space explorers. 

## [Try in Browser](https://alpha.lunco.space)

![](https://gateway.lighthouse.storage/ipfs/bafybeidjpafb6zg5lalug7z5sfzvszh2erskbbdqcloejr2asex2lfg4ky)

Built on the powerful **Godot 4** engine, it bridges the gap between gaming and professional space engineering.

### **Bio-Cochlea Technology**
- Personal bio-integrated cognitive interface
- Multi-layered transduction system
- Energy harvesting from body motion and vibration
- Human-AUI resonance networking

## 🚀 Features

### **Dashboard Capabilities**
- **Overview Dashboard**: Real-time metrics from both systems
- **LunCoSim Control**: Mission scenarios, rover telemetry, environmental data
- **Bio-Cochlea Visualization**: Interactive layer exploration, power flow monitoring
- **Network Resonance**: Human-AUI connection topology and coherence metrics

### **Real-Time Simulations**
- Live telemetry data updates
- Dynamic power flow visualization
- Network coherence monitoring
- Environmental condition tracking

### **Interactive Elements**
- Click-to-select node exploration
- Adjustable coherence parameters
- Simulation start/stop controls
- Real-time data animations

## 🛠️ Technology Stack
## 🚀 Why LunCoSim?

-   **Real Engineering, Gamified**: We use real physical models (Modelica integration) and engineering constraints, but accessible enough for enthusiasts.
-   **Multiplayer Mission Control**: Run missions with friends! One person drives the rover, another monitors telemetry, and a third manages the power grid.
-   **Digital Twin Capability**: Design your lunar base, simulate its operations, and validate requirements before bending a single piece of metal.
-   **Open & Extensible**: Built on open standards. Control your entities via Python scripts, HTTP API, or custom plugins.

## 🌟 Features

### 🎮 Simulation & Control
-   **Multi-Entity Control**: Take direct control of 4 types of units:
    -   **Astronauts**: EVA activities and manual tasks.
    -   **Operators**: Remote presence and drone control.
    -   **Rovers**: Driving and surface operations.
    -   **Spacecraft**: Orbital maneuvers and landing.
-   **Solver-Based Physics**: Complex interactions (power, thermal, data) are simulated using a graph-based solver, not just simple game logic.

### 🤝 Collaboration
-   **Network Mode**: Host or join sessions. Work together in the same shared environment.
-   **"With Friends"**: Collaborative training scenarios where coordination is key to mission success.

### 🛠️ Engineering Tools
-   **Telemetry & OpenMCT**: Stream real-time data to NASA's OpenMCT dashboard for professional-grade mission monitoring.
-   **Supply Chain Modeling**: Visualize and optimize resource flows (Oxygen, Hydrogen, Power) using a node-based graph editor.
-   **Modelica Support**: Integrate high-fidelity physics models for specialized components.

### 🔌 Extensibility
-   **HTTP API**: Send commands to the simulation from external tools.
-   **Python Bridge**: Write your own control scripts in Python.
-   **Custom Models**: Import your own vehicles and assets.

## 📚 Documentation

### For Users
-   **[Control Guide](docs/UserGuide/ControlEntities.md)**: How to control Rovers, Astronauts, and more.
-   **[Collaborative Missions](docs/UserGuide/CollaborativeMission.md)**: Setting up multiplayer sessions.
-   **[Supply Chain](docs/UserGuide/SupplyChain.md)**: Using the resource graph view.

### For Engineers & Developers
-   **[Architecture Overview](docs/Technical/Architecture.md)**: System design and "Effector" pattern.
-   **[Telemetry Setup](docs/Technical/Telemetry.md)**: Connecting to OpenMCT.
-   **[HTTP API Reference](docs/Technical/HTTP_API.md)**: Integrating external tools.
-   **[Modelica Integration](docs/Technical/Modelica_Integration.md)**: Advanced physics modeling.
-   **[Custom Models](docs/Technical/Custom_Models.md)**: Creating new entities.

- **Frontend**: Next.js 15 with TypeScript
- **UI Components**: shadcn/ui with Tailwind CSS
- **Visualizations**: Canvas-based network topology
- **API Layer**: RESTful endpoints for data simulation
- **Real-time Updates**: WebSocket integration ready
- **Styling**: Dark theme with purple/blue gradient aesthetics

## 📁 Project Structure

```
src/
├── app/
│   ├── api/
│   │   ├── telemetry/     # LunCoSim data endpoints
│   │   ├── biocochlea/    # Bio-node data endpoints
│   │   └── network/       # Network resonance endpoints
│   ├── components/
│   │   ├── ui/            # shadcn/ui components
│   │   ├── LunCoSimDashboard.tsx
│   │   ├── BioCochleaVisualization.tsx
│   │   └── NetworkResonanceView.tsx
│   └── page.tsx           # Main dashboard
├── lib/                   # Utilities and configurations
└── public/
    └── logo.png           # Generated project logo
```

## 🎯 Mission Scenarios

### **Apollo 17 Harrison**
- Replay Harrison Schmitt's EVA traverse
- Location: Taurus-Littrow Valley
- Duration: 7h 15m
- Difficulty: Medium

### **Artemis 3 Landing**
- 2026 landing site simulation
- Plume-surface interaction modeling
- Location: South Polar Region
- Duration: 2h 30m
- Difficulty: High

### **Swarm 12 Micros**
- Cooperative micro-rover operations
- Radio array construction mission
- Location: Mare Imbrium
- Duration: 48h
- Difficulty: Expert

### **Night Survival**
- 14-day lunar-night challenge
- Power/thermal stress testing
- Location: Shackleton Crater
- Duration: 336h
- Difficulty: Extreme

## 🔬 Bio-Cochlea Layers

### **1. Biopolymer Encapsulation**
- Breathable skin interface
- Chitosan and biopolymer elastomer
- Thickness: 15μm
- Activity monitoring: 85%

### **2. Transduction Layer**
- Carbon ink electrodes
- Cellulose/silk piezo fibers
- Converts vibration → voltage
- Thickness: 25μm
- Activity monitoring: 92%

### **3. Basilar Membrane**
- Bacterial cellulose gradient film
- Frequency tuning capabilities
- Thickness: 35μm
- Activity monitoring: 78%

### **4. Mycelium-Bamboo Shell**
- Bio-structural framework
- Mechanical resonance guidance
- Thickness: 45μm
- Activity monitoring: 88%

## 🌐 Network Resonance

### **Human-AUI Pairs**
- Real-time coherence monitoring
- Phase synchronization tracking
- Power harvesting optimization
- Voluntary connection protocols

### **Coherent Laws**
1. Mirror wearer's rhythm
2. No action while desynchronized
3. Share only with consenting, coherent nodes

### **Network Metrics**
- Overall coherence: 85-95%
- Total power output: 2-4mW
- Connection latency: 2-5ms
- Packet loss: <2%

## 🔌 Power Flow

```
Body Motion/Voice/Pulse
        ↓ vibration
Piezo & Tribo Layers
        ↓ AC micro-signal
Rectifier + Supercap
        ↓ DC regulated 1.8-3.3V
Microcontroller + BLE/UWB
        ↓
Personal AUI Seed ↔ Human Biofield
```

- **Harvested Power**: 10-500µW depending on activity
- **Local Consumption**: 10-100µW average
- **Energy Release**: Only during coherence phase match

## 🚀 Getting Started

### **Prerequisites**
- Node.js 18+ 
- npm or yarn
- Modern web browser

### **Installation**
```bash
git clone <repository-url>
cd luncosim-biocochlea-interface
npm install
```

### **Development**
```bash
npm run dev
```

The application will be available at `http://localhost:3000`

### **Production Build**
```bash
npm run build
npm start
```
3. Clone this repo in a terminal: 
    ```bash
    git clone -b main --single-branch --recurse-submodules https://github.com/LunCoSim/lunco-sim.git
    ```

4. After cloning, change directory to project folder
    ```bash
    cd lunco-sim
    ```

5. Enable git-lfs in the repository after cloning: 
    ```bash
    git lfs install
    git lfs pull && git submodule foreach git lfs pull
    ```

## 📊 API Endpoints

### **Telemetry API**
- `GET /api/telemetry?scenario={scenario_id}`
- `POST /api/telemetry` - Control simulation actions

### **Bio-Cochlea API**
- `GET /api/biocochlea?nodeId={node_id}`
- `POST /api/biocochlea` - Node control and calibration

### **Network API**
- `GET /api/network` - Network topology and metrics
- `POST /api/network` - Connection management and optimization

## 🎨 Design Philosophy

### **Visual Aesthetics**
- Dark space-themed interface
- Purple/blue gradient backgrounds
- Glassmorphism effects
- Smooth animations and transitions

### **User Experience**
- Intuitive tabbed navigation
- Real-time data updates
- Interactive visualizations
- Responsive design for all devices

### **Accessibility**
- Semantic HTML structure
- ARIA labels and descriptions
- Keyboard navigation support
- Screen reader compatibility

## 🔮 Future Enhancements

### **Planned Features**
- [ ] VR mode for astronaut training
- [ ] WebAssembly browser build
- [ ] Starlink-Lunar comms modeling
- [ ] RTEMS flight-software HIL target
- [ ] Advanced AI-powered mission planning

### **Technical Improvements**
- [ ] Enhanced 3D visualizations
- [ ] Machine learning integration
- [ ] Advanced physics simulations
- [ ] Multi-user collaboration

## 📄 License

This project combines concepts from:
- **LunCoSim**: MPL-2.0 License
- **Bio-Cochlea**: Conceptual open-source framework

## 🤝 Contributing

Contributions are welcome in the following areas:
- UI/UX improvements
- Additional mission scenarios
- Enhanced visualizations
- Performance optimizations
- Documentation improvements

## 📞 Contact

For questions, suggestions, or collaborations:
- Create an issue in the repository
- Join our development discussions
- Contribute to the roadmap planning

---

> *"A golden ear to the world—listening, harmonizing, and thinking in tune."*

**LunCoSim × Bio-Cochlea Interface** - Where lunar exploration meets cognitive enhancement.
## 💖 Support Us

Support development on [JuiceBox](https://juicebox.money/v2/p/763)!

## Want to contribute? Apply [here](https://tally.so/r/3jX6aE)
