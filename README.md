
# LunCoSim: Everyone can do Space

LunCoSim is an open-source, collaborative space simulation platform designed for planning lunar & and space missions, engineering complex systems, and training future space explorers. 

## [Try in Browser](https://alpha.lunco.space)

![](https://gateway.lighthouse.storage/ipfs/bafybeidjpafb6zg5lalug7z5sfzvszh2erskbbdqcloejr2asex2lfg4ky)

Built on the powerful **Godot 4** engine, it bridges the gap between gaming and professional space engineering.


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

## 🛠 Installation

0. The development is done on Linux Mate, so there could be issues running on Windows and MacOs. Please reach us

1. Install [Godot 4.5](https://godotengine.org/download/)

2. Install [git lfs](https://github.com/git-lfs/git-lfs#getting-started). It handles large files in the repository. Use git-cmd if you are on Windows.

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

7. Now open project and wait till intenal conent management downloads all the files. LunCoSim Content Manager (new system, gradually being adopted):
   1. Will be installed automatically with other addons
   2. After installation, you'll see a "Content" button in the editor toolbar
   3. Use it to download missing content files when needed

8. Wait till all the files are downloaded. You'll see the message in the Output tab.

9. Restart editor and enjoy!


### Content Management Notes
- Some large files are still managed by git-lfs
- Newer content will use `.content` files for external storage
- If you see missing files:
  1. First try git-lfs: `git lfs pull`
  2. Then use the Content Manager in the editor toolbar
  3. If issues persist, please reach out on Discord


## 🌐 Community & Support

Join our vibrant community and stay updated on the latest developments:

- [Discord Server](https://discord.gg/A6U3GdvQum)
- [Twitter](https://twitter.com/LunCoSim)
- [Website](https://lunco.space/)
- [LinkedIn](https://www.linkedin.com/company/luncosim/)
- [YouTube Channel](https://www.youtube.com/@LunCoSim)

## 💖 Support Us

Support development on [JuiceBox](https://juicebox.money/v2/p/763)!

## Want to contribute? Apply [here](https://tally.so/r/3jX6aE)