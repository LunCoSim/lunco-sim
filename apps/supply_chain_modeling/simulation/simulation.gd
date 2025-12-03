class_name SimulationManager
extends Node

const MODULE_PATH = "res://modules/supply_chain_modeling"

# === Signals ===
signal node_added(node: SimulationNode)
signal node_removed(node_id: String)
signal connection_added(from_id, from_port, to_id, port)
signal connection_removed(from_id, from_port, to_id, port)

# === Variables ===
var connections: Array[Dictionary] = []  # Dictionary of connections [from_id, from_port, to_id, port]

# Solver Graph
var solver_graph: LCSolverGraph = LCSolverGraph.new()

var paused: bool = true
var simulation_time: float = 0.0
var time_scale: float = 1.0
var time_unit: float = 60.0

var resource_manager: ResourceRegistry = ResourceRegistry.get_instance()

# === Functions ===
func add_node(node: SimulationNode) -> void:
	add_child(node)
	node.name = node.name.validate_node_name()
	
	# Register with solver if it's a SolverSimulationNode
	if node is SolverSimulationNode:
		node.register_with_solver(solver_graph)
	
	emit_signal("node_added", node)

func remove_node(node_id: NodePath) -> void:
	var node = get_node(node_id)	
	if node:
		for connection in connections:
			if NodePath(connection["from_node"]) == node_id or NodePath(connection["to_node"]) == node_id:
				disconnect_nodes(connection["from_node"], connection["from_port"], connection["to_node"], connection["to_port"])
		remove_child(node) #remove connections as well
		emit_signal("node_removed", node_id)

func _physics_process(delta: float) -> void:
	if paused:
		return
	
	simulation_time += delta * time_scale
	
	# Update solver parameters from component states
	for child in get_children():
		if child is SolverSimulationNode:
			child.update_solver_state()
	
	# Solve the graph
	solver_graph.solve(delta * time_scale)
	
	# Update component states from solver results
	for child in get_children():
		if child is SolverSimulationNode:
			child.update_from_solver()


func save_state() -> Dictionary:
	var state = {
		"simulation_time": simulation_time,
		"time_scale": time_scale,
		"nodes": {},
		"connections": connections
	}
	
	# Save nodes
	for child in get_children():
		if child is SimulationNode:
			state["nodes"][child.name] = {
				"type": Utils.get_custom_class_name(child),
				"state": child.save_state()
			}
	
	return state

func load_state(state: Dictionary) -> void:
	# Clear existing state
	for node in get_children():
		if node is SimulationNode:
			node.queue_free()
	
	connections.clear()
	
	# Load simulation parameters
	simulation_time = state.get("simulation_time", 0.0)
	time_scale = state.get("time_scale", 1.0)
	
	# Load nodes
	for node_name in state["nodes"]:
		var node_data = state["nodes"][node_name]
		var script_path = Utils.get_script_path(node_data["type"])
		if script_path:
			var node_script = load(script_path)
			if node_script:
				var node = node_script.new()
				node.name = node_name
				add_child(node)
				# Restore node properties if available
				node.load_state(node_data["state"])
	
	# Load connections
	for connection in state["connections"]:
		connect_nodes(
			connection["from_node"],
			connection["from_port"],
			connection["to_node"],
			connection["to_port"]
		)

func can_connect(from_node: String, from_port: int, to_node: String, to_port: int) -> Dictionary:
	var response = {
		"success": false,
		"message": ""
	}
	
	# Get the actual nodes
	var source = get_node_or_null(from_node)
	var target = get_node_or_null(to_node)
	
	if not source or not target:
		response.message = "Invalid nodes"
		return response
	
	# Use the existing validation from StorageFacility
	if source is StorageFacility:
		if not source.can_connect_with(target, from_port, to_port):
			response.message = "Resources are not compatible"
			return response
	
	response.success = true
	return response

func connect_nodes(from_node: String, from_port: int, to_node: String, to_port: int) -> Dictionary:
	var validation = can_connect(from_node, from_port, to_node, to_port)
	if not validation.success:
		return validation
	
	# Get the actual component nodes
	var source = get_node_or_null(from_node)
	var target = get_node_or_null(to_node)
	
	if not source or not target:
		validation.success = false
		validation.message = "Invalid nodes"
		return validation
	
	# Create solver edge if both are SolverSimulationNodes
	var solver_edge = null
	if source is SolverSimulationNode and target is SolverSimulationNode:
		# Map port indices to port names (simplified - assumes single port or indexed ports)
		var source_port_name = _get_port_name(source, from_port, true)
		var target_port_name = _get_port_name(target, to_port, false)
		
		if source_port_name and target_port_name:
			var source_port = source.get_port(source_port_name)
			var target_port = target.get_port(target_port_name)
			
			if source_port and target_port:
				# Create edge with default conductance
				solver_edge = solver_graph.connect_nodes(source_port, target_port, 1.0, source_port.domain)
	
	var connection = {
		"from_node": from_node,
		"from_port": from_port,
		"to_node": to_node,
		"to_port": to_port,
		"solver_edge": solver_edge
	}
	
	connections.append(connection)
	emit_signal("connection_added", from_node, from_port, to_node, to_port)
	
	return validation

func disconnect_nodes(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> bool:
	for i in range(connections.size() - 1, -1, -1):
		var connection = connections[i]
		if connection["from_node"] == from_node and \
		   connection["from_port"] == from_port and \
		   connection["to_node"] == to_node and \
		   connection["to_port"] == to_port:
			# Remove solver edge if it exists
			if connection.has("solver_edge") and connection["solver_edge"]:
				solver_graph.remove_edge(connection["solver_edge"])
			
			connections.remove_at(i)
			emit_signal("connection_removed", from_node, from_port, to_node, to_port)
			return true
	return false

# === Simulation Control ===
func set_time_scale(new_scale: float) -> void:
	time_scale = new_scale

func toggle_simulation() -> void:
	paused = !paused
	set_simulation_status(paused)

func pause_simulation() -> void:
	paused = true
	set_simulation_status(paused)

func resume_simulation() -> void:
	paused = false
	set_simulation_status(paused)
	
func set_simulation_status(_paused: bool) -> void:
	for node in get_children():
		if node is Node:
			node.set_physics_process(not _paused)

func get_simulation_time() -> float:
	return simulation_time

func get_simulation_time_scaled() -> float:
	return simulation_time * time_unit

func clear_simulation() -> void:
	# Step 1: Clear all simulation nodes
	for node in get_children():
		node.queue_free()
	
	# Step 2: Clear stored connections
	connections.clear()

func reset_simulation() -> void:
	simulation_time = 0.0

func new_simulation() -> void:
	clear_simulation()
	reset_simulation()
	pause_simulation()

func add_node_from_path(custom_class_name: String) -> SimulationNode:
	var path = Utils.get_script_path(custom_class_name)
	var node_script = load(path)
	if node_script:
		var sim_node = node_script.new()
		add_node(sim_node)
		return sim_node
	return null

## Helper function to map port index to port name
## This is a simplified mapping - components define their own port names
func _get_port_name(component: SolverSimulationNode, port_index: int, is_output: bool) -> String:
	# For StorageFacility: single port "fluid_port"
	if component is StorageFacility:
		return "fluid_port"
	
	# For Pump: inlet (port 0 input) and outlet (port 0 output)
	if component is Pump:
		if is_output:
			return "outlet"
		else:
			return "inlet"
	
	# For ElectrolyticFactory:
	# Inputs: 0=water_in, 1=power_in
	# Outputs: 0=h2_out, 1=o2_out
	if component is ElectrolyticFactory:
		if is_output:
			if port_index == 0:
				return "h2_out"
			elif port_index == 1:
				return "o2_out"
		else:
			if port_index == 0:
				return "water_in"
			elif port_index == 1:
				return "power_in"
	
	# Default fallback
	return ""
