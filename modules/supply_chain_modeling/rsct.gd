@tool
extends Control

@onready var graph_edit: GraphEdit = %GraphEdit
@onready var sim_time_label: Label = %SimTimeLabel

# Save file path for the current graph
var save_file_path: String = "user://current_graph.save"
var autosave_timer: float = 0.0
const AUTOSAVE_INTERVAL: float = 60000.0  # Autosave every 60 seconds

var sim_time : float = 0.0
var time_scale: float = 1.0  # Default time scale (1 second real time = 1 minute simulation time)
var time_unit: float = 60.0  # Default time unit (1 second real time = 1 minute simulation time)
var paused: bool = false  # Simulation paused state

func _ready():
	# Connect signals for handling connections
	graph_edit.connect("connection_request", _on_connection_request)
	graph_edit.connect("disconnection_request", _on_disconnection_request)
	graph_edit.connect("end_node_move", _on_node_moved)
	
	# Enable snapping and minimap for better UX
	graph_edit.snapping_distance = 20
	#graph_edit.show_minimap = true
	#graph_edit.minimap_enabled = true
	
	
	load_graph()
	update_sim_time_label()

func _process(delta: float) -> void:
	if not paused:
		sim_time += delta * time_scale
		update_sim_time_label()
		# Update objects based on sim_time here

	autosave_timer += delta
	if autosave_timer >= AUTOSAVE_INTERVAL:
		autosave_timer = 0.0
		save_graph()

func save_graph() -> void:
	
	var save_data := {
		"nodes": {},
		"connections": [],
		"view": {
			"scroll_offset": graph_edit.scroll_offset
		}
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

func load_graph() -> void:
	if not FileAccess.file_exists(save_file_path):
		return
		
	var file = FileAccess.open(save_file_path, FileAccess.READ)
	if not file:
		return
		
	var save_data = file.get_var()
	if not save_data:
		return
	
	# Clear existing graph
	for node in graph_edit.get_children():
		if node is GraphNode:
			node.free()
	
	# Load nodes
	for node_name in save_data["nodes"]:
		var node_data = save_data["nodes"][node_name]
		var node_scene = load(node_data["type"])
		if node_scene:
			var node = node_scene.instantiate()
			node.name = node_name
			node.position_offset = node_data["position"]
			node.size = node_data["size"]
			graph_edit.add_child(node)
			node.set_owner(null) # Ensure node isn't saved with scene
	
	# Load connections
	for connection in save_data["connections"]:
		graph_edit.connect_node(
			connection["from_node"],
			connection["from_port"],
			connection["to_node"],
			connection["to_port"]
		)
		
		# Load view settings
	if "view" in save_data:
		graph_edit.call_deferred("set_scroll_offset", save_data["view"]["scroll_offset"]) 
		print("Load: ", save_data["view"]["scroll_offset"])
	
	print("Load complete")

func _on_connection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int):
	# Create new connection between nodes
	graph_edit.connect_node(from_node, from_port, to_node, to_port)
	
	# You can add custom logic here for handling the resource flow
	print("Connected: ", from_node, "(", from_port, ") -> ", to_node, "(", to_port, ")")
	
	save_graph()

func _on_disconnection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int):
	# Remove connection between nodes
	graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
	
	# You can add custom cleanup logic here
	print("Disconnected: ", from_node, "(", from_port, ") -> ", to_node, "(", to_port, ")")
	
	save_graph()

func _on_node_moved():
	save_graph()

# Add functions to control simulation
func set_time_scale(new_scale: float) -> void:
	time_scale = new_scale

func pause_simulation() -> void:
	paused = true

func resume_simulation() -> void:
	paused = false


func add_node_from_path(path: String):
	var node_scene = load(path)
	if node_scene:
		var node = node_scene.instantiate()
		graph_edit.add_child(node)
		node.set_owner(null) # Ensure node isn't saved with scene
		save_graph()

func _on_button_3_pressed() -> void:
	add_node_from_path("res://modules/supply_chain_modeling/resources/resource_o_2.tscn")

func _on_button_4_pressed() -> void:
	add_node_from_path("res://modules/supply_chain_modeling/resources/resource_h_2.tscn")

func _on_button_5_pressed() -> void:
	add_node_from_path("res://modules/supply_chain_modeling/facilities/object_factory.tscn")


func _on_button_10_pressed() -> void:
	add_node_from_path("res://modules/supply_chain_modeling/facilities/solar_power_plant.tscn")
	
	

func update_sim_time_label() -> void:
	var sim_time_minutes = round(sim_time * time_unit)
	sim_time_label.text = "Sim Time: " + str(sim_time_minutes) + " minutes"

func _on_button_6_pressed() -> void:
	if paused:
		resume_simulation()
	else:
		pause_simulation()

func _on_button_7_pressed() -> void:
	set_time_scale(max(0.1, time_scale - 0.1))  # Decrease time scale, minimum 0.1
	print("Time scale decreased to: ", time_scale)

func _on_button_8_pressed() -> void:
	set_time_scale(time_scale + 0.1)  # Increase time scale
	print("Time scale increased to: ", time_scale)

func _on_button_11_pressed() -> void:
	add_node_from_path("res://modules/supply_chain_modeling/facilities/storage.tscn")

func _on_button_12_pressed() -> void:
	add_node_from_path("res://modules/supply_chain_modeling/resources/resource_h2o.tscn")


	
