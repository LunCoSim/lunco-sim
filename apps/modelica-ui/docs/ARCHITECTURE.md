# LunSim Technical Architecture

## Overview

LunSim is built using Godot 4.4 with GDScript, leveraging Godot's GraphElement component for the node-based building system and implementing a simplified simulation engine with a path toward potential Modelica integration in the future.

## Implementation Strategy

The development follows a progressive implementation approach:

1. **Proof of Concept (POC)**: Create a minimal viable implementation with core functionality
2. **Iterative Enhancement**: Add features incrementally while maintaining a working system
3. **Feature Expansion**: Expand to include the full feature set once the core is solid

## Core POC Systems

### 1. Component System

**Core Classes:**
- `BaseComponent`: Extends GraphElement, represents a building component
- `ResourcePort`: Connection points for resource flows
- `PortConnection`: Handles connections between components

**Implementation:**
```gdscript
# Example BaseComponent class
class_name BaseComponent
extends GraphElement

var id: String = ""
var component_type: String = ""
var inputs = {}  # Resource input ports
var outputs = {}  # Resource output ports
var is_active: bool = true

func _ready():
    # Basic GraphElement setup
    resizable = false
    show_close = true
    title = component_type
    
func setup_ports():
    # Will be implemented by child components
    pass

func process_simulation(delta):
    # Basic simulation step - to be overridden
    pass
```

### 2. Resource System (POC Version)

**Core Implementation:**
- Simple static resource type definitions to start
- Basic resource properties (name, unit, color)
- Extension points for adding more complex behaviors later

```gdscript
# Simple resource type definition
class_name ResourceType
extends Resource

# Define core resource types as constants
const ELECTRICITY = "electricity"
const OXYGEN = "oxygen"  
const WATER = "water"

# Static resource properties
static var properties = {
    ELECTRICITY: {
        "name": "Electricity",
        "unit": "kW",
        "color": Color(1.0, 0.9, 0.0)
    },
    # Other resources...
}
```

### 3. Simulation Engine (POC Version)

**Components:**
- `SimulationManager`: Controls time and simulation steps using Godot's `_physics_process`
- Simple resource transfer logic between components

**Simulation Loop:**
1. Process all components (production/consumption)
2. Process all connections (resource transfers)
3. Update visualizations

```gdscript
# Basic simulation manager
class_name SimulationManager
extends Node

var components = []
var connections = []
var simulation_running = true
var simulation_speed = 1.0

func _physics_process(delta):
    if simulation_running:
        process_simulation(delta * simulation_speed)
        
func process_simulation(delta):
    # Process components and connections
    # ...
```

### 4. User Interface (POC Version)

**Main Elements:**
- `BuildingController`: Manages component placement and connection creation
- Simple component palette with basic components
- Basic time controls (play/pause, speed)

## Progressive Enhancement Path

After completing the POC, the system can be enhanced in the following order:

1. **Resource System Enhancement**
   - Move to a more flexible resource management system
   - Add resource manager singleton
   - Create individual resource type classes

2. **Component System Expansion**
   - Add component state management
   - Implement component properties system
   - Create more specialized components

3. **Simulation Depth**
   - Add environmental influences
   - Implement more complex resource transformations
   - Create event system for challenges

4. **Advanced UI & Visualization**
   - Improve resource flow visualization
   - Add component status indicators
   - Implement component property editors

5. **Save/Load System**
   - Add serialization to all core elements
   - Implement save file management
   - Add autosave functionality

## Future Technical Expansions (Post-POC)

- More complex equation systems (still in GDScript)
- Optional native modules for performance-critical code
- Optional Modelica integration (long-term)
- Advanced visualization modes (thermal, power, etc.)
- Serialization for colony sharing
- Mobile-friendly UI mode

## POC Implementation Steps

1. Create project structure and base classes
2. Implement core simulation manager
3. Create basic component types (Solar Panel, Battery, Habitat)
4. Implement connection system
5. Add UI for component placement
6. Create demo scenario with working resource flow

## POC Technical Requirements

- Godot 4.4 with GDScript
- GraphElement-based component system
- Simple resource definitions
- Basic UI for interaction
- Resource flow visualization

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