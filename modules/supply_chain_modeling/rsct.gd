extends Node2D

@onready var graph_edit: GraphEdit = $GraphEdit

# Save file path for the current graph
var save_file_path: String = "user://current_graph.save"
var autosave_timer: float = 0.0
const AUTOSAVE_INTERVAL: float = 60.0  # Autosave every 60 seconds

func _ready():
	# Connect signals for handling connections
	graph_edit.connect("connection_request", _on_connection_request)
	graph_edit.connect("disconnection_request", _on_disconnection_request)
	
	# Enable snapping and minimap for better UX
	graph_edit.snapping_distance = 20
	#graph_edit.show_minimap = true
	#graph_edit.minimap_enabled = true

func _on_connection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int):
	# Create new connection between nodes
	graph_edit.connect_node(from_node, from_port, to_node, to_port)
	
	# You can add custom logic here for handling the resource flow
	print("Connected: ", from_node, "(", from_port, ") -> ", to_node, "(", to_port, ")")

func _on_disconnection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int):
	# Remove connection between nodes
	graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
	
	# You can add custom cleanup logic here
	print("Disconnected: ", from_node, "(", from_port, ") -> ", to_node, "(", to_port, ")")

func _process(delta: float) -> void:
	autosave_timer += delta
	if autosave_timer >= AUTOSAVE_INTERVAL:
		autosave_timer = 0.0
		save_graph()

func save_graph() -> void:
	var save_data := {
		"nodes": {},
		"connections": []
	}
	
	# Save all node data
	for node in graph_edit.get_children():
		if node is GraphNode:
			save_data["nodes"][node.name] = {
				"position": node.position_offset,
				"size": node.size,
				"type": node.scene_file_path
			}
	
	# Save all connections
	for connection in graph_edit.get_connection_list():
		save_data["connections"].append({
			"from_node": connection["from_node"],
			"from_port": connection["from_port"],
			"to_node": connection["to_node"],
			"to_port": connection["to_port"]
		})
	
	# Save to file
	var file = FileAccess.open(save_file_path, FileAccess.WRITE)
	if file:
		file.store_var(save_data)
		print("Graph autosaved successfully")
