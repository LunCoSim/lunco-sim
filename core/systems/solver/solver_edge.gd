class_name LCSolverEdge
extends RefCounted

## Edge in the Linear Graph Solver
## Represents a flow path between two nodes.
## Analogous to an Electrical Resistor or Modelica Connection.

# Unique ID
var id: int = -1

# Domain of this edge
var domain: StringName = "Fluid"

# The two nodes this edge connects
var node_a # LCSolverNode
var node_b # LCSolverNode

# Conductance (G = 1/R)
# Flow = G * (Potential_A - Potential_B + Potential_Source)
# Fluid: kg / (s * Pa)
# Electrical: Siemens (1/Ohm)
var conductance: float = 0.0

# Potential Source (Active Element like a Pump/Battery)
# Adds potential "push" from A to B
var potential_source: float = 0.0

# Calculated Flow Rate
# Positive means flow A -> B
# Fluid: kg/s
# Electrical: Amperes (C/s)
var flow_rate: float = 0.0

# Unidirectional Flow (Check Valve / Diode)
# If true, flow can only be positive (A -> B).
var is_unidirectional: bool = false

# Allowed Resource Types (Optional, for Fluid domain)
# If set, only allows flow if nodes have compatible resources.
var allowed_resource_types: Array[StringName] = []

func _init(p_id: int, p_node_a, p_node_b, p_conductance: float = 1.0, p_domain: StringName = "Fluid"):
	id = p_id
	node_a = p_node_a
	node_b = p_node_b
	conductance = p_conductance
	domain = p_domain

## Calculate flow based on current node potentials
func update_flow():
	var delta_p = node_a.potential - node_b.potential + potential_source
	
	# Unidirectional check (Diode/Check Valve)
	if is_unidirectional and delta_p < 0:
		flow_rate = 0.0
		return
		
	# Resource Compatibility Check (Simple)
	# If strict checking is needed, we can add it here.
	# For now, we assume the graph builder ensures valid connections.
	
	flow_rate = delta_p * conductance

## Get the other node connected to this edge
func get_other_node(node) :
	if node == node_a:
		return node_b
	elif node == node_b:
		return node_a
	return null
