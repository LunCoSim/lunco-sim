class_name LCSolverEdge
extends RefCounted

## Edge in the Linear Graph Solver
## Represents a flow path between two nodes.
## Analogous to an Electrical Resistor or Modelica Connection.

# Unique ID
var id: int = -1

# The two nodes this edge connects
var node_a: LCSolverNode
var node_b: LCSolverNode

# Conductance (G = 1/R)
# Flow = G * (Pressure_A - Pressure_B + Pressure_Source)
# Units: kg / (s * Pa)
var conductance: float = 0.0

# Pressure Source (Active Element like a Pump)
# Adds pressure "push" from A to B
var pressure_source: float = 0.0

# Calculated Flow Rate (kg/s)
# Positive means flow A -> B
var flow_rate: float = 0.0

func _init(p_id: int, p_node_a: LCSolverNode, p_node_b: LCSolverNode, p_conductance: float = 1.0):
	id = p_id
	node_a = p_node_a
	node_b = p_node_b
	conductance = p_conductance

## Calculate flow based on current node pressures
func update_flow():
	var delta_p = node_a.pressure - node_b.pressure + pressure_source
	flow_rate = delta_p * conductance

## Get the other node connected to this edge
func get_other_node(node: LCSolverNode) -> LCSolverNode:
	if node == node_a:
		return node_b
	elif node == node_b:
		return node_a
	return null
