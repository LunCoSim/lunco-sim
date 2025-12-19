# Collaborative Training Use Case

## Mission Context

Commercial Lunar Payload Services (CLPS) missions involve lunar landers (e.g., Astrobotic, Intuitive Machines, ispace, Firefly) delivering multiple commercial payloads to the lunar surface.

## Scenario

Multiple commercial payloads operate on the lunar surface:
- Rovers
- Cameras
- Scientific instruments
- Other mobile/stationary equipment

## Key Challenges

### Resource Constraints
- Limited power budget
- Restricted connectivity bandwidth
- Shared computational resources

### Coordination Requirements
- **Collision avoidance**: Rovers must not interfere with each other
- **Conflicting operations**: Payloads have individual ConOps that may conflict
- **Provider control**: Lander provider manages resource allocation

### Command Authorization
- Payloads must request permission from provider before executing commands
- Commands classified by hazard levels:
  - Low hazard: routine operations
  - Medium hazard: resource-intensive operations
  - High hazard: mission-critical operations
- Permission requirements vary by mission phase (early mission: strict control → later mission: relaxed control)

## Simulation Objectives

Implement collaborative training scenarios where:

1. **Multiple operators** control different payloads simultaneously
2. **Resource arbitration** enforces power/bandwidth limits
3. **Permission system** validates commands based on hazard level and mission phase
4. **Collision detection** prevents physical interference between payloads
5. **ConOps validation** ensures operations align with approved procedures

## Training Scenarios

- Multi-rover coordination in shared workspace
- Resource-constrained operations (limited power/bandwidth)
- Emergency procedures with escalating command permissions
- Provider-payload communication protocols
- Mission phase transitions (commissioning → nominal → extended operations)
