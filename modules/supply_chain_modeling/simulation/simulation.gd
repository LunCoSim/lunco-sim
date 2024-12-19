class_name SimulationManager
extends Node

signal simulation_step_completed
signal node_added(node: SimulationNode)
signal node_removed(node_id: String)
signal connection_added(from_id, from_port, to_id, port)
signal connection_removed(from_id, from_port, to_id, port)

var connections: Dictionary = {} # Dictionary of connections [from_id, from_port, to_id, port]

var paused: bool = true
var simulation_time: float = 0.0
var time_scale: float = 1.0
var time_unit: float = 60.0

func add_node(node: SimulationNode) -> void:
	add_child(node)
	emit_signal("node_added", node)

func remove_node(node_id: String) -> void:
	var node = get_node(node_id)	
	if node:
		remove_child(node) #remove connections as well
		emit_signal("node_removed", node_id)

func _physics_process(delta: float) -> void:
	if paused:
		return
	
	simulation_time += delta * time_scale


func save_state() -> Dictionary:
	var save_data := {
		"simulation_time": simulation_time,
		"nodes": {},
		"connections": [],
		"time_scale": time_scale
	}
	
	# Save nodes
	for node in get_children():
		if node is SimulationNode:
			save_data["nodes"][node.name] = {
				"type": node.scene_file_path,
				# Add any other node-specific data needed
				"properties": node.get_save_properties() if node.has_method("get_save_properties") else {}
			}
	
	# Save connections
	for connection in connections.values():
		save_data["connections"].append({
			"from_node": connection["from"],
			"from_port": connection["from_port"],
			"to_node": connection["to"],
			"to_port": connection["port"]
		})
	
	return save_data

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
		var node_scene = load(node_data["type"])
		if node_scene:
			var node = node_scene.instantiate()
			node.name = node_name
			add_child(node)
			# Restore node properties if available
			if "properties" in node_data and node.has_method("load_properties"):
				node.load_properties(node_data["properties"])
	
	# Load connections
	for connection in state["connections"]:
		connect_nodes(
			connection["from_node"],
			connection["from_port"],
			connection["to_node"],
			connection["to_port"]
		)

func connect_nodes(from_id: String, from_port: int, to_id: String, port: int) -> bool:
	if not (get_node(from_id) and get_node(to_id)):
		return false
		
	var connection = {
		"from": from_id,
		"from_port": from_port, 
		"to": to_id,
		"port": port
	}
	
	connections[connection.hash()] = connection	
	
	emit_signal("connection_added", from_id, from_port, to_id, port)
	return true

func disconnect_nodes(from_id: String, from_port: int, to_id: String, port: int) -> bool:
	# Step 1: Check if both nodes exist
	if not (get_node(from_id) and get_node(to_id)):
		return false
	
	# Step 2: Create the connection dictionary to match
	var connection_to_remove = {
		"from": from_id,
		"from_port": from_port,
		"to": to_id,
		"port": port
	}
	
	# Step 3: Find and remove the connection
	var source_node = get_node(from_id)
	for connection in source_node.connections:
		if connection.hash() == connection_to_remove.hash():
			# Step 4: Remove the connection
			source_node.connections.erase(connection)
			# Step 5: Emit the signal
			emit_signal("connection_removed", from_id, from_port, to_id, port)
			return true
	
	# Step 6: Return false if connection wasn't found
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
	
func set_simulation_status(_paused: bool):
	for node in get_children():
		if node is Node:
			node.set_physics_process(not _paused)

func get_simulation_time() -> float:
	return simulation_time

func get_simulation_time_scaled() -> float	:
	return simulation_time * time_unit
