class_name SimulationManager
extends Node

signal simulation_step_completed
signal node_added(node: SimulationNode)
signal node_removed(node_id: String)
signal connection_added(from_id, from_port, to_id, port)
signal connection_removed(from_id, from_port, to_id, port)

var nodes: Dictionary = {}
var paused: bool = true
var simulation_time: float = 0.0
var time_scale: float = 1.0

func add_node(node: SimulationNode) -> void:
	nodes[node.node_id] = node
	emit_signal("node_added", node)

func remove_node(node_id: String) -> void:
	if nodes.has(node_id):
		nodes.erase(node_id)
		emit_signal("node_removed", node_id)

func _physics_process(delta: float) -> void:
	if paused:
		return
		
	simulation_time += delta * time_scale
	process_simulation_step(delta)
	
func process_simulation_step(delta: float) -> void:
	for node in nodes.values():
		if node.has_method("process_step"):
			node.process_step(delta)
	emit_signal("simulation_step_completed")

func save_state() -> Dictionary:
	var state = {
		"time": simulation_time,
		"nodes": {},
		"connections": []
	}
	
	for node in nodes.values():
		state.nodes[node.node_id] = node.to_dict()
		
	return state

func load_state(state: Dictionary) -> void:
	simulation_time = state.get("time", 0.0)
	nodes.clear()
	
	for node_id in state.nodes:
		var node_data = state.nodes[node_id]
		#var node = create_node_from_data(node_data)
		#add_node(node)

func connect_nodes(from_id: String, from_port: int, to_id: String, port: int) -> bool:
	if not (nodes.has(from_id) and nodes.has(to_id)):
		return false
		
	var connection = {
		"from": from_id,
		"from_port": from_port, 
		"to": to_id,
		"port": port
	}
	
	nodes[from_id].connections.append(connection)
	emit_signal("connection_added", from_id, from_port, to_id, port)
	return true

func disconnect_nodes(from_id: String, from_port: int, to_id: String, port: int) -> bool:
	# Step 1: Check if both nodes exist
	if not (nodes.has(from_id) and nodes.has(to_id)):
		return false
	
	# Step 2: Create the connection dictionary to match
	var connection_to_remove = {
		"from": from_id,
		"from_port": from_port,
		"to": to_id,
		"port": port
	}
	
	# Step 3: Find and remove the connection
	var source_node = nodes[from_id]
	for connection in source_node.connections:
		if connection.hash() == connection_to_remove.hash():
			# Step 4: Remove the connection
			source_node.connections.erase(connection)
			# Step 5: Emit the signal
			emit_signal("connection_removed", from_id, from_port, to_id, port)
			return true
	
	# Step 6: Return false if connection wasn't found
	return false
