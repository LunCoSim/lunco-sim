# LunSim Technical Architecture

## Overview

LunSim is built using Godot 4 with GDScript, leveraging Godot's GraphEdit component for the node-based building system and incorporating a simplified Modelica-inspired simulation engine.

## Core Systems

### 1. Node-Based Building System

**Core Classes:**
- `BuildingNode`: Extends GraphNode, represents a building component
- `ResourcePort`: Connection points for resource flows
- `ConnectionManager`: Handles connection validation and creation

**Implementation:**
```gdscript
# Example BuildingNode class
class_name BuildingNode
extends GraphNode

var inputs = {}
var outputs = {}
var properties = {}
var simulation_data = {}

func _init():
    resizable = false
    show_close = true
    
func setup_ports():
    # Create input/output ports based on component type
    
func process_simulation(delta):
    # Process inputs, modify state, generate outputs
```

### 2. Resource System

**Core Classes:**
- `Resource`: Base class for all resources
- `ResourceFlow`: Handles transfer between components
- `ResourceContainer`: Manages resource storage

**Resource Types:**
- Material resources (physical items)
- Fluid resources (gases, liquids)
- Energy resources (electricity, heat)

### 3. Simulation Engine

**Components:**
- `SimulationManager`: Controls time and simulation steps
- `EquationSolver`: Simplified solver for component interactions
- `StateTracker`: Maintains and updates system state

**Simulation Loop:**
1. Update inputs for all components
2. Solve equations for current state
3. Update outputs and flows
4. Advance simulation time

### 4. User Interface

**Main Elements:**
- `BuildingEditor`: Main GraphEdit-based building interface
- `ComponentPalette`: Available components for placement
- `ResourceMonitor`: Displays resource levels and flows
- `TimeControls`: Controls for simulation speed

### 5. Modelica Integration (Future)

**Simplified Phase:**
- Basic equation-based components
- Simplified solvers for component interactions
- Limited variable types and connections

**Advanced Phase:**
- Full Modelica parser integration
- Differential equation solver
- Modelica Standard Library compatibility

## Data Flow

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│  User Interface │    │ Component Logic  │    │  Visualization  │
│                 │◄───┤                 │◄───┤                 │
│  - Input        │    │  - Simulation   │    │  - Rendering    │
│  - Controls     │───►│  - State        │───►│  - Feedback     │
└─────────────────┘    └─────────────────┘    └─────────────────┘
```

## File Structure

```
/scenes
  /main.tscn            # Main game scene
  /building_editor.tscn # Node editing interface
  /simulation_view.tscn # Simulation visualization

/scripts
  /core                 # Core simulation systems
    /simulation.gd      # Simulation manager
    /resource.gd        # Resource base class
    /equation_solver.gd # Simplified equation solver
  
  /components           # Building components
    /base_component.gd  # Base component class
    /power/             # Power system components
    /life_support/      # Life support components
    /habitat/           # Habitat components
    /production/        # Production components
  
  /ui                   # User interface scripts
    /component_palette.gd
    /resource_monitor.gd
    /time_controls.gd

/resources              # Game resources
  /components/          # Component definitions
  /icons/               # UI icons
  /themes/              # UI themes
```

## Initial Implementation Steps

1. Create base component system with GraphEdit
2. Implement resource flow visualization
3. Add simple simulation loop with basic equation solving
4. Create initial set of power and life support components
5. Implement time controls and basic UI
6. Add resource tracking and monitoring
7. Create sandbox mode for free building

## Performance Considerations

- Use Godot's signal system for loose coupling
- Optimize simulation updates for larger colonies
- Implement component update priority for critical systems
- Consider threading for complex equation solving (future)

## Future Technical Expansions

- Full Modelica language support
- Differential equation solvers
- Multiplayer colony sharing
- Advanced visualization modes (thermal, power, etc.)
- Serialization for colony sharing
- Mobile platform considerations 