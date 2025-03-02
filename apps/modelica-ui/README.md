# Lunar Colony Simulator (LunSim)

A game-first approach to lunar colony simulation built with Godot 4.4 and GDScript, with a path toward potential Modelica integration in the future.

## Overview

LunSim combines engaging gameplay mechanics inspired by factory-building games like Factorio with an intuitive node-based building system. Build, manage, and optimize a sustainable lunar colony while learning about the challenges of extraterrestrial habitation.

## Development Strategy

We're using a phased approach, starting with a focused Proof of Concept:

1. **POC Phase (Current)**: Create a minimal functioning prototype demonstrating core mechanics
2. **MVP Phase**: Expand to a fuller gameplay experience with more components
3. **Feature Expansion**: Progressively add depth and complexity
4. **Full Experience**: Complete game with campaign, challenges, and comprehensive systems

## Core Philosophy

- **Game-First Experience**: We focus on creating fun, engaging gameplay before adding simulation complexity
- **Visual Feedback**: All actions provide immediate, satisfying visual and audio feedback
- **Progressive Complexity**: Start simple and gradually unlock more challenging systems
- **Intuitive Building**: Using Godot 4.4's GraphElement for a smooth building experience
- **Early Validation**: Gameplay testing ensures mechanics are enjoyable

## Current Focus (POC)

- **Basic Building System**: Place and connect simple components
- **Resource Flows**: Visualize electricity, oxygen, and water moving between components
- **Core Components**: Solar panels, batteries, and habitat modules
- **Simple Simulation**: Resource production, storage, and consumption

## Implementation Plan

1. Create core component framework with GraphElement
2. Implement basic resource system
3. Add connection functionality
4. Create visual feedback for resource flows
5. Implement simple time controls
6. Balance initial components for demonstration

## Documentation

- [Roadmap](docs/ROADMAP.md): Development phases and timelines
- [Architecture](docs/ARCHITECTURE.md): Technical design and implementation
- [Gameplay](docs/GAMEPLAY.md): Core mechanics and player experience
- [Components](docs/COMPONENTS.md): Building block specifications
- [Design](docs/DESIGN.md): Visual and UX design principles
- [Gameplay Validation](docs/GAMEPLAY_VALIDATION.md): How we validate gameplay mechanics
- [Modelica Integration](docs/MODELICA_INTEGRATION.md): Long-term integration strategy

## Getting Started

*Detailed instructions for setup and running will be provided once POC is completed*

## Contributing

We welcome contributions! Please refer to our development strategy and focus on the current POC priorities.

## License

*To be determined*

# Modelica UI

A user interface for working with Modelica models, built using Godot Engine.

## Features

- Load Modelica files (.mo) and their dependencies
- Show all loaded Modelica files in a file tree
- Edit, save, and create new Modelica files with syntax highlighting
- Run simulations with configurable parameters
- View simulation results in a table
- Export simulation results to CSV

## Getting Started

### Prerequisites

- Godot Engine 4.x

### Running the UI

1. Open the project in Godot Engine
2. Run the project (F5)
3. Use the UI to load, edit, and simulate Modelica models

## Usage

### Loading a Model

1. Click the "Load" button in the top-left corner
2. Select a Modelica (.mo) file to load
3. The file and its dependencies will be loaded and shown in the file tree

### Creating a New Model

1. Click the "New" button in the top-left corner
2. Choose a location and name for your new file
3. A template model will be created and opened in the editor

### Editing a Model

1. Select a file from the file tree to open it in the editor
2. Make your changes in the code editor
3. Click "Save" to save your changes

### Running a Simulation

1. Open the model you want to simulate
2. Configure the simulation parameters (start time, end time, step size)
3. Click "Run Simulation" to start the simulation
4. Results will appear in the table below

### Exporting Results

1. After running a simulation, click "Export CSV"
2. Choose a location and name for your CSV file
3. The simulation results will be exported to the CSV file

## Project Structure

- `scenes/` - Contains the UI scene definitions
- `scripts/` - Contains the scripts for the UI functionality
  - `ui/` - UI-specific scripts
  - `core/` - Core functionality scripts
- `resources/` - Resources used by the UI

## Development

The Modelica UI uses the Modelica core functionality from the `apps/modelica` directory.

### Key Components

- `modelica_ui_controller.gd` - Main UI controller script
- `modelica_syntax_highlighter.gd` - Syntax highlighting for Modelica files
- `modelica_simulator.gd` - Handles simulation and results

## Roadmap

Planned features for future versions:

- Graphical representation of simulation results
- Better error reporting and visualization
- Auto-completion in the code editor
- Model debugging capabilities
- Support for more complex Modelica features 