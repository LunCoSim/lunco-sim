# LunCoSim

[![Discord](https://img.shields.io/discord/979381990220513320?style=flat-square&label=Discord&logo=discord&logoColor=white&color=5865F2)](https://discord.gg/A6U3GdvQum)
[![X](https://img.shields.io/badge/Follow-%40LunCoSim-000000?style=flat-square&logo=x&logoColor=white)](https://twitter.com/LunCoSim)
[![LinkedIn](https://img.shields.io/badge/LinkedIn-LunCoSim-0A66C2?style=flat-square&logo=linkedin&logoColor=white)](https://www.linkedin.com/company/luncosim/)
[![YouTube](https://img.shields.io/badge/YouTube-Subscribe-FF0000?style=flat-square&logo=youtube&logoColor=white)](https://www.youtube.com/@LunCoSim)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-brightgreen?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)

### Build a space mission. See how it behaves.

LunCoSim is an open-source simulation workbench for space engineers,
researchers, and students. Assemble vehicles and environments, connect physical
models, and explore how a system behaves throughout an operation.

A rover's suspension, its controller, and the terrain all affect the same
traverse. LunCoSim brings those interactions into one scene, with models you
can inspect, parameters you can change, and telemetry you can follow.

[**Download LunCoSim**](https://github.com/LunCoSim/lunco-sim/releases)
· [Tutorials](docs/tutorials/README.md)
· [Watch demos](https://www.youtube.com/@LunCoSim)
· [Join the community](https://discord.gg/A6U3GdvQum)

## What can you do with it?

- **Build and drive a rover.** Assemble a vehicle, explore wheel and suspension
  behavior, and observe its interaction with terrain.
- **Connect equations to motion.** Wire a Modelica controller to a lander's
  rigid-body physics and inspect the feedback loop.
- **Explore subsystem behavior.** Edit electrical, thermal, and mechanical
  models in the embedded Modelica workbench, run parameter studies, and compare
  plots.
- **Script an operation.** Define mission phases, waypoints, and responses to
  events, then inspect the resulting state through the UI or API.

Start with the [lander and rover walkthrough](docs/tutorials/01-lander-rover-mission.md)
or see [how a Modelica model drives a physical vehicle](docs/tutorials/03-cosim.md).

## Your mission is an editable project

A **Twin** is a project folder containing the scene, models, scripts, and
configuration. OpenUSD describes the assembly and its connections; Modelica
describes continuous behavior; Rhai scripts describe the operation. You can
reuse components and change authored models and scenarios without rebuilding
the Rust engine.

The same command interface serves the workbench, scripts, and AI agents, so
you can move from interactive exploration to automated studies.

[Create your first Twin](docs/tutorials/00-create-a-twin.md)
· [Explore the component library](docs/component-index.md)
· [Automate through the API](docs/apps/README.md#talking-to-a-running-app--http-api--mcp)

## Try it

1. Download the installer for your platform from [GitHub Releases](https://github.com/LunCoSim/lunco-sim/releases).
   See the [installation guide](docs/apps/luncosim/README.md#desktop-updates) for package details.
2. Open the app's Tutorials menu. Start with **View, Build & Lunica** to find
   your way around, then **First Drive** to try a rover.
3. Follow a [walkthrough](docs/tutorials/README.md#authoring-walkthroughs) to build your own project.

LunCoSim is under active development. Model coverage and maturity vary;
engineering conclusions need validation for their intended use. Check the
[feature status](specs/README.md) and [known limitations](docs/reviews/README.md).

## Build with us

Have a mission to model, a component to contribute, or a result to compare?
[Bring it to Discord](https://discord.gg/A6U3GdvQum).
For bugs, [open an issue](https://github.com/LunCoSim/lunco-sim/issues) with the
release version, steps to reproduce, and expected behavior.

[Build from source](docs/apps/README.md#build-from-source)
· [Documentation](docs/README.md)
· [Contributor and agent guide](AGENTS.md)
· [Roadmap and integrations](docs/architecture/engineering-backlog-and-standards.md)

LunCoSim is licensed under [Apache 2.0](LICENSE).
[Website](https://lunco.space/)
· [LinkedIn](https://www.linkedin.com/company/luncosim/)
· [X](https://twitter.com/LunCoSim)
