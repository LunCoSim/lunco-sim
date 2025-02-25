# LunSim Gameplay Guide

## Core Design Philosophy

- **Game-First Approach**: Focus on fun, engaging mechanics before simulation complexity
- **Visual Feedback**: Immediate, satisfying feedback for all player actions
- **Progressive Complexity**: Start simple, gradually introduce more complex systems
- **Accessible Building**: Intuitive controls for component placement and connections
- **Resource Visualization**: Clear visual representation of resource flows and states

## POC Gameplay Experience

The Proof of Concept (POC) will focus on these core experiences:

1. **Building a Simple Base**: Place and connect basic components
2. **Resource Flow**: Visualize electricity, oxygen, and water moving between components
3. **Basic Simulation**: See how resources are produced, stored, and consumed
4. **Component Interaction**: Create functional systems by connecting compatible components

### POC Core Loop

1. **Place components** from a simple palette onto the graph
2. **Connect components** via intuitive connection lines
3. **Observe resources flowing** between components with visual feedback
4. **Expand the system** by adding more components and connections

### Simplified Controls for POC

- **Component Placement**: Click on component type, then click on graph to place
- **Connection Creation**: Click on output port, then click on compatible input port
- **Time Controls**: Simple play/pause and speed buttons

## Full Gameplay (Post-POC)

### Getting Started

#### Tutorial Mission
New players begin with a guided tutorial mission:
1. Build a basic solar power system with satisfying visual feedback
2. Connect a habitat module and see resources flow through connections
3. Add basic life support with visual and audio feedback
4. Learn about resource management through gameplay, not just text

#### Sandbox Mode
After completing the tutorial, players can:
- Start with predefined scenarios
- Build from scratch with all components unlocked
- Set custom challenge parameters
- Share creations via built-in recording system

### Building Your Colony

#### Component Placement
1. Select a component from the palette
2. Place it on the grid (with satisfying snap/placement effects)
3. Configure its properties via intuitive interface
4. Connect it to other components via ports (with visual connection feedback)

#### Connection Rules
- Components must have compatible ports to connect
- Visual indicators show compatible connection points
- Connection process has visual and audio feedback
- Real-time feedback on connection quality and efficiency

### Resource Management

#### Resource Types
- Each resource has unique visual representation and behaviors
- Resources visibly flow between components
- Resource levels are visualized intuitively
- Critical shortages have clear visual and audio warnings

#### Flow Management
- Monitor resource flows with visual indicators
- Balance production and consumption with intuitive controls
- Create storage buffers for critical resources
- Handle waste and byproducts with visual feedback on environmental impact

### Time System

#### Lunar Day/Night Cycle
- Dynamic lighting represents lunar day/night
- Solar panels visibly react to sunlight levels
- Temperature visualization shows extremes
- Day/night transitions have atmospheric visual effects

#### Simulation Speed
- Pause: For building and detailed planning
- Normal: Standard gameplay
- Fast: For rapid development or waiting
- Time controls have intuitive UI and feedback

## POC Implementation Plan

### Phase 1: Core Building
- Implement GraphElement-based component system
- Create basic component palette
- Implement connection creation logic
- Test basic component placement and connections

### Phase 2: Resource System
- Implement simplified resource types
- Add resource port system
- Create basic resource transfer logic
- Test resource production and consumption

### Phase 3: Visualization
- Add simple resource flow visualization
- Implement basic component status indicators
- Create minimal but functional UI
- Test the visual feedback system

### Phase 4: Integration
- Connect all systems together
- Create a simple test scenario
- Balance resource production/consumption
- Finalize the POC demonstration

## POC Testing Priorities

1. **Usability**: Is component placement and connection intuitive?
2. **Visual Clarity**: Are resource flows easy to understand?
3. **Performance**: Does the system perform well with 10-20 components?
4. **Stability**: Is the simulation stable and predictable?
5. **Engagement**: Is the core building loop enjoyable?

## Next Steps (Post-POC)

- Add more component types
- Implement day/night cycle
- Add resource storage and management
- Create crisis events and challenges
- Implement save/load functionality
- Add more complex resource transformations

## Core Gameplay

### Building Your Colony

#### Component Placement
1. Select a component from the palette
2. Place it on the grid
3. Configure its properties
4. Connect it to other components via ports

#### Connection Rules
- Components must have compatible ports to connect
- Some connections require specific resource types
- Distance limitations may apply for certain components

### Resource Management

#### Resource Types
- Each resource has unique properties and behaviors
- Resources are produced, stored, consumed, and transformed
- Resource balancing is key to colony survival

#### Flow Management
- Monitor resource flows between components
- Balance production and consumption
- Create storage buffers for critical resources
- Handle waste and byproducts

### Optimization Challenges

#### Efficiency
- Improve component placement for better efficiency
- Upgrade components for better performance
- Minimize resource waste

#### Expansion Planning
- Plan for future growth
- Create modular systems
- Build redundancy for critical systems

### Time System

#### Lunar Day/Night Cycle
- A full lunar day (29.5 Earth days) drives the gameplay cycle
- Solar power only works during lunar daytime
- Temperature extremes between day and night

#### Simulation Speed
- Pause: For building and detailed planning
- Normal: Standard gameplay
- Fast: For rapid development or waiting

### Crisis Management

#### System Failures
- Components may fail or degrade
- Power outages
- Resource shortages

#### Environmental Challenges
- Radiation events
- Meteorite impacts
- Dust storms

## User Interface

### Build Mode
- Graph-based view of components
- Connection visualization
- Component palette
- Property editor

### Monitor Mode
- Resource level displays
- Flow rate indicators
- Efficiency metrics
- Alert notifications

### Time Controls
- Pause/play/fast-forward
- Day/night indicator
- Mission timer

## Progression

### Initial Challenges
1. **Power Management**: Balance day/night power needs
2. **Basic Life Support**: Maintain oxygen and water
3. **Resource Collection**: Establish mining operations

### Intermediate Challenges
1. **Closed Loop Systems**: Create recycling systems
2. **Expansion**: Support more colonists
3. **Manufacturing**: Create advanced components

### Advanced Challenges
1. **Self-Sufficiency**: Eliminate Earth resupply
2. **Research**: Develop new technologies
3. **Colony Specialization**: Focus on specific industries

## Game Modes

### Campaign
- Series of increasingly complex missions
- Structured goals and challenges
- Progressive technology unlocks

### Sandbox
- All components available
- Customizable starting conditions
- No failure conditions

### Challenge Mode
- Specific scenarios with limited resources
- Time pressure
- Scoring system

## Tips for New Players

1. Start small and expand gradually
2. Always build excess power capacity
3. Create redundant life support systems
4. Monitor resource flows constantly
5. Plan for the long lunar night early
6. Build storage buffers for critical resources
7. Pay attention to efficiency metrics 