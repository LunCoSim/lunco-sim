class_name LCSolverNode
extends RefCounted

## Node in the Linear Graph Solver
## Represents a point of potential (Pressure).
## Analogous to an Electrical Node (Voltage) or Modelica FluidPort.

# Unique ID for the solver
var id: int = -1

# State Variable: Pressure (Pascals)
# In electrical analogy: Voltage (Volts)
var pressure: float = 0.0

# If true, this node is a reference node (Ground/Atmosphere)
# Its pressure is fixed and will not be solved for.
var is_ground: bool = false

# Connected edges (for graph traversal if needed)
var edges: Array[LCSolverEdge] = []

func _init(p_id: int, p_initial_pressure: float = 0.0, p_is_ground: bool = false):
	id = p_id
	pressure = p_initial_pressure
	is_ground = p_is_ground

func add_edge(edge: LCSolverEdge):
	if not edge in edges:
		edges.append(edge)

func remove_edge(edge: LCSolverEdge):
	edges.erase(edge)
