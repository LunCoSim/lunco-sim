class_name LCPumpComponent
extends LCResourceComponent

## Pump Component
## Physics: Voltage Source
## Adds pressure head to the flow path.

# Parameters
var max_pressure: float = 100000.0 # Pa (Head)
var max_flow: float = 10.0 # kg/s (Not strictly enforced by linear model, but implies resistance)
var power: float = 0.0 # 0.0 to 1.0 (Throttle) - starts closed

# Internal
var conductance: float = 1.0
var edge: LCSolverEdge

func _init(p_graph: LCSolverGraph, p_max_pressure: float = 100000.0):
	super._init(p_graph)
	max_pressure = p_max_pressure
	
	# Assume some internal resistance
	conductance = 0.1 # Arbitrary for now

## Connect two existing nodes with this pump
func connect_nodes(node_in: LCSolverNode, node_out: LCSolverNode):
	if edge:
		graph.remove_edge(edge)
		
	# Pump pushes from In -> Out (Liquid domain for propellant)
	edge = graph.connect_nodes(node_in, node_out, conductance, "Liquid")

func set_power(p_power: float):
	power = clamp(p_power, 0.0, 1.0)

func update(delta: float):
	if edge:
		# Apply pressure source based on power
		edge.potential_source = max_pressure * power
		
		# CRITICAL: Conductance must be 0 when pump is off
		# Otherwise flow happens even without pressure source
		if power > 0.01:
			edge.conductance = conductance
		else:
			edge.conductance = 0.0
