class_name SolverSimulationNode
extends SimulationNode

## Base class for simulation components that use the LCSolverGraph
## Manages a collection of Ports (LCSolverNodes) for multi-domain modeling.
## 
## Example: An ElectricPump component has:
##   - fluid_in (Port -> LCSolverNode, Domain: Fluid)
##   - fluid_out (Port -> LCSolverNode, Domain: Fluid)
##   - power_in (Port -> LCSolverNode, Domain: Electrical)

# Reference to the solver graph (set by SimulationManager)
var solver_graph: LCSolverGraph = null

# Ports: Dictionary of port_name -> LCSolverNode
var ports: Dictionary = {}

# Internal edges (connections between ports within this component)
var internal_edges: Array[LCSolverEdge] = []

## Called by SimulationManager to register this component with the solver
func register_with_solver(graph: LCSolverGraph):
	solver_graph = graph
	_create_ports()
	_create_internal_edges()

## Override this to create ports for your component
## Example:
##   ports["inlet"] = solver_graph.add_node(0.0, false, "Fluid")
##   ports["outlet"] = solver_graph.add_node(0.0, false, "Fluid")
func _create_ports():
	pass

## Override this to create internal edges between ports
## Example:
##   var edge = solver_graph.connect_nodes(ports["inlet"], ports["outlet"], 1.0, "Fluid")
##   internal_edges.append(edge)
func _create_internal_edges():
	pass

## Called before solver.solve() - update solver parameters from component state
## Example: Update pump pressure_source based on power availability
func update_solver_state():
	pass

## Called after solver.solve() - sync component state from solver results
## Example: Update current_amount from port.flow_accumulation
func update_from_solver():
	pass

## Get a port by name
func get_port(port_name: String) -> LCSolverNode:
	return ports.get(port_name)

## Cleanup
func _exit_tree():
	# Remove internal edges
	for edge in internal_edges:
		if solver_graph:
			solver_graph.remove_edge(edge)
	
	# Remove ports
	for port_name in ports:
		if solver_graph:
			solver_graph.remove_node(ports[port_name])
