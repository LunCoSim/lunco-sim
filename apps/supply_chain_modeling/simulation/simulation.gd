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

var paused: bool = true
var simulation_time: float = 0.0
var time_scale: float = 1.0
var time_unit: float = 60.0

var resource_manager: ResourceRegistry = ResourceRegistry.get_instance()

# === Functions ===
func add_node(node: SimulationNode) -> void:
	add_child(node)
	node.name = node.name.validate_node_name()
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
	
	var connection = {
		"from_node": from_node,
		"from_port": from_port,
		"to_node": to_node,
		"to_port": to_port
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
