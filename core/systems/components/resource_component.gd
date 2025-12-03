class_name LCResourceComponent
extends RefCounted

## Base class for physical resource components (Tanks, Pipes, Pumps)
## Acts as the "Brain" of the component, handling physics logic.
## Wraps LCSolverNode(s) and LCSolverEdge(s).

# Reference to the solver graph this component belongs to
var graph: LCSolverGraph

# Dictionary of named ports (String -> LCSolverNode)
# Example: {"inlet": node1, "outlet": node2}
var ports: Dictionary = {}

func _init(p_graph: LCSolverGraph):
	graph = p_graph

## Called every physics frame to update internal state (e.g. integrate mass)
## and update solver parameters (e.g. set pressure based on level)
func update(delta: float):
	pass

## Helper to create a port
func _create_port(name: String, initial_pressure: float = 0.0) -> LCSolverNode:
	var node = graph.add_node(initial_pressure)
	ports[name] = node
	return node

## Get a port by name
func get_port(name: String) -> LCSolverNode:
	return ports.get(name)
