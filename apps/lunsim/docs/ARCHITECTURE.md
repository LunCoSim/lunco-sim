# LunSim Technical Architecture

## Overview

LunSim is built using Godot 4.4 with GDScript, leveraging Godot's GraphElement component for the node-based building system and implementing a simplified simulation engine with a path toward potential Modelica integration in the future.

## Core Systems

### 1. Node-Based Building System

**Core Classes:**
- `BuildingNode`: Extends GraphElement, represents a building component
- `ResourcePort`: Connection points for resource flows
- `ConnectionManager`: Handles connection validation and creation
- `VisualFeedback`: Provides visual feedback for connections and operations

**Implementation:**
```gdscript
# Example BuildingNode class
class_name BuildingNode
extends GraphElement

var inputs = {}
var outputs = {}
var properties = {}
var simulation_data = {}
var visual_effects = {}

func _init():
    resizable = false
    show_close = true
    
func setup_ports():
    # Create input/output ports based on component type
    
func process_simulation(delta):
    # Process inputs, modify state, generate outputs
    
func update_visual_feedback():
    # Update visual effects based on current state
```

### 2. Resource System

**Core Classes:**
- `Resource`: Base class for all resources
- `ResourceFlow`: Handles transfer between components
- `ResourceContainer`: Manages resource storage
- `ResourceEffects`: Manages visual representation of resources

**Resource Types:**
- Material resources (physical items)
- Fluid resources (gases, liquids)
- Energy resources (electricity, heat)

### 3. Simulation Engine

**Components:**
- `SimulationManager`: Controls time and simulation steps
- `SimpleSolver`: Handles basic resource transfers and transformations
- `StateTracker`: Maintains and updates system state
- `FeedbackSystem`: Provides gameplay feedback on simulation

**Simulation Loop:**
1. Update inputs for all components
2. Process component logic with simple equations
3. Update outputs and flows
4. Generate visual and audio feedback
5. Advance simulation time

### 4. User Interface

**Main Elements:**
- `BuildingEditor`: Main GraphElement-based building interface
- `ComponentPalette`: Available components for placement
- `ResourceMonitor`: Displays resource levels and flows
- `TimeControls`: Controls for simulation speed
- `FeedbackPanel`: Provides gameplay guidance and feedback

### 5. Future Extensibility for Modelica

**Initial Architecture Considerations:**
- Use abstract interfaces for simulation components
- Implement a plugin system for simulation solvers
- Design resource system to be compatible with equation-based modeling
- Document connection points and interfaces for future integration

**Integration Path:**
1. Start with simple GDScript simulations
2. Add more complex equation-based models still in GDScript
3. Implement native module interfaces (optional future step)
4. Add Modelica parser and integration (optional future step)

## Data Flow

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│  User Interface │    │ Component Logic  │    │  Visualization  │
│                 │◄───┤                 │◄───┤                 │
│  - Input        │    │  - Simulation   │    │  - Rendering    │
│  - Controls     │───►│  - State        │───►│  - Feedback     │
└─────────────────┘    └─────────────────┘    └─────────────────┘
                                 ▲
                        ┌────────┴────────┐
                        │ Gameplay Systems│
                        │ - Progression   │
                        │ - Challenges    │
                        └─────────────────┘
```

## File Structure

```
/scenes
  /main.tscn            # Main game scene
  /building_editor.tscn # Node editing interface
  /simulation_view.tscn # Simulation visualization
  /ui/                  # UI scenes
    /component_palette.tscn
    /resource_monitor.tscn
    /feedback_panel.tscn

/scripts
  /core                 # Core simulation systems
    /simulation.gd      # Simulation manager
    /resource.gd        # Resource base class
    /simple_solver.gd   # Simple equation solver
    /gameplay.gd        # Gameplay systems manager
    /recording.gd       # Gameplay recording system
  
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
    /visual_effects.gd  # Visual effects manager

/resources              # Game resources
  /components/          # Component definitions
  /icons/               # UI icons
  /themes/              # UI themes
  /effects/             # Visual and audio effects
  /tutorials/           # Tutorial resources
```

## Initial Implementation Steps

1. Create base component system with GraphElement
2. Implement resource flow with visual feedback
3. Add simple simulation loop with basic resource transformations
4. Create initial set of power and life support components
5. Implement time controls and engaging UI with feedback
6. Add resource tracking and monitoring with visual feedback
7. Create sandbox mode for free building and experimentation
8. Implement gameplay recording for validation and sharing

## Performance Considerations

- Use Godot's signal system for loose coupling
- Optimize simulation updates for larger colonies
- Implement component update priority for critical systems
- Use visual instancing for resource flows
- Consider simpler models for mobile platforms

## Future Technical Expansions

- More complex equation systems (still in GDScript)
- Optional native modules for performance-critical code
- Optional Modelica integration (long-term)
- Multiplayer colony sharing
- Advanced visualization modes (thermal, power, etc.)
- Serialization for colony sharing
- Mobile-friendly UI mode 