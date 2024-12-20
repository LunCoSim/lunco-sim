class_name SimulationManager
extends Node

signal node_added(node: SimulationNode)
signal node_removed(node_id: String)
signal connection_added(from_id, from_port, to_id, port)
signal connection_removed(from_id, from_port, to_id, port)

var connections: Array[Dictionary] = []  # Dictionary of connections [from_id, from_port, to_id, port]

var paused: bool = true
var simulation_time: float = 0.0
var time_scale: float = 1.0
var time_unit: float = 60.0

var resource_manager: ResourceManager = ResourceManager.get_instance()

func add_node(node: SimulationNode) -> void:
	add_child(node)
	emit_signal("node_added", node)

func remove_node(node_id: NodePath) -> void:
	var node = get_node(node_id)	
	if node:
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
				"type": child.get_script().resource_path,
				"properties": child.properties
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
		var node_scene = load(node_data["type"])
		if node_scene:
			var node = Node.new()
			node.script = node_scene

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

func connect_nodes(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> bool:
	var source = get_node_or_null(NodePath(from_node))
	var target = get_node_or_null(NodePath(to_node))
	
	if source and target:
		var connection = {
			"from_node": from_node,
			"from_port": from_port,
			"to_node": to_node,
			"to_port": to_port
		}
		connections.append(connection)

		emit_signal("connection_added", from_node, from_port, to_node, to_port)
		return true
	return false

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
	
func set_simulation_status(_paused: bool):
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

func new_simulation():
	clear_simulation()
	reset_simulation()
	pause_simulation()
