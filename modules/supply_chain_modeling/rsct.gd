@tool
extends Control

@onready var graph_edit: GraphEdit = %GraphEdit
@onready var sim_time_label: Label = %SimTimeLabel
@onready var button_container: VBoxContainer = %ButtonContainer
@onready var properties: VBoxContainer = %Properties
@onready var save_dialog: FileDialog = %SaveDialog
@onready var load_dialog: FileDialog = %LoadDialog
@onready var simulation: SimulationManager = %Simulation

# Save file path for the current graph
var save_file_path: String = "user://current_graph.save"
var autosave_timer: float = 0.0
const AUTOSAVE_INTERVAL: float = 60000.0  # Autosave every 60 seconds

var sim_time : float = 0.0
var time_scale: float = 1.0  # Default time scale (1 second real time = 1 minute simulation time)
var time_unit: float = 60.0  # Default time unit (1 second real time = 1 minute simulation time)
var paused: bool = true  # Simulation paused state

var dragging_new_node: bool = false
var dragging_node_path: String = ""


func _ready():
	#%Simulation.connect("node_added", _on_simulation_node_added)
	#%Simulation.connect("node_removed", _on_simulation_node_removed)

	# Connect signals
	Web3Interface.connect("wallet_connected", _on_wallet_connected)
	Web3Interface.connect("wallet_disconnected", _on_wallet_disconnected)
	
	Web3Interface.connect("nft_minted", _on_nft_minted)
	Web3Interface.connect("nft_load_complete", _on_nft_load_complete)
	
	# Connect signals for handling connections
	# For graph_edit it's ok to do in code to make it easier to have several views
	graph_edit.connect("connection_request", _on_connection_request)
	graph_edit.connect("disconnection_request", _on_disconnection_request)
	graph_edit.connect("end_node_move", _on_node_moved)
	graph_edit.connect("delete_nodes_request", _on_delete_nodes_request)
	graph_edit.connect("node_selected", _on_node_selected)
	graph_edit.connect("node_deselected", _on_node_deselected)
	
	load_graph()
	update_sim_time_label()
	pause_simulation()
	
	# UI initialization
	create_buttons()
	
	# Connect FileMenu signals
	%FileMenu.id_pressed.connect(_on_file_menu_pressed)
	
	# Connect NFTMenu signals
	%NFTMenu.id_pressed.connect(_on_nft_menu_pressed)

func _process(delta: float) -> void:
	# Keep autosave in process cycle since it's not physics-dependent
	autosave_timer += delta
	if autosave_timer >= AUTOSAVE_INTERVAL:
		autosave_timer = 0.0
		save_graph()
		
	# Update UI elements
	update_sim_time_label()

func _physics_process(delta: float) -> void:
	if not paused:
		# Update simulation time
		sim_time += delta * time_scale

func save_graph() -> void:
	
	var save_data := {
		"nodes": {},
		"connections": [],
		"view": {
			"scroll_offset": graph_edit.scroll_offset,
			"zoom": graph_edit.zoom
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
	new_graph()
	
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
		if "zoom" in save_data["view"]:
			graph_edit.zoom = save_data["view"]["zoom"]
		#print("Load: ", save_data["view"]["scroll_offset"], " zoom: ", save_data["view"]["zoom"])
	
	pause_simulation()
	print("Load complete")

func _on_connection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> void:
	if simulation.connect_nodes(from_node, to_node, from_port):
		graph_edit.connect_node(from_node, from_port, to_node, to_port)

func _on_disconnection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int):
	# Remove connection between nodes
	graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
	print("Disconnected: ", from_node, "(", from_port, ") -> ", to_node, "(", to_port, ")")
	save_graph()

func _on_node_moved():
	save_graph()

# Add functions to control simulation
func set_time_scale(new_scale: float) -> void:
	time_scale = new_scale

func pause_simulation() -> void:
	paused = true
	# Disable physics processing for all nodes
	for node in graph_edit.get_children():
		if node is GraphNode:
			node.set_physics_process(false)

func resume_simulation() -> void:
	paused = false
	# Enable physics processing for all nodes
	for node in graph_edit.get_children():
		if node is GraphNode:
			node.set_physics_process(true)

func add_node_from_path(path: String, position: Vector2 = Vector2.ZERO):
	var node_scene = load(path)
	if node_scene:
		var node = node_scene.instantiate()
		graph_edit.add_child(node)
		node.set_owner(null)
		
		# Set initial physics processing state based on simulation state
		node.set_physics_process(!paused)
		
		if position == Vector2.ZERO:
			# Use old center positioning if no position specified
			var viewport_size = graph_edit.size
			var scroll_offset = graph_edit.scroll_offset
			var zoom = graph_edit.zoom
			var center_x = (scroll_offset.x + viewport_size.x / 2) / zoom
			var center_y = (scroll_offset.y + viewport_size.y / 2) / zoom
			node.position_offset = Vector2(center_x - node.size.x / 2, center_y - node.size.y / 2)
		else:
			node.position_offset = position - node.size / 2
		
		save_graph()

# Unified function for handling button release
func _handle_button_release() -> void:
	if dragging_new_node:
		# Get the button that's being released
		var button = get_viewport().gui_get_focus_owner()
		if button is Button and button.get_global_rect().has_point(get_viewport().get_mouse_position()):
			# If released while still over button, create in center
			add_node_from_path(dragging_node_path)
		else:
			# If released elsewhere, create at mouse position
			var mouse_pos = graph_edit.get_local_mouse_position()
			if graph_edit.get_rect().has_point(mouse_pos):
				var graph_pos = (mouse_pos + graph_edit.scroll_offset) / graph_edit.zoom
				add_node_from_path(dragging_node_path, graph_pos)
		dragging_new_node = false
		dragging_node_path = ""


func _on_button_6_pressed() -> void:
	print('_on_button_6_pressed')
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

func new_graph() -> void:
	# Clear existing graph
	for node in graph_edit.get_children():
		if node is GraphNode:
			node.free()
	
	# Reset simulation time
	pause_simulation()
	sim_time = 0.0
	update_sim_time_label()
	
	# Reset view
	graph_edit.scroll_offset = Vector2.ZERO
	
	# Save the empty state
	save_graph()

func _on_delete_nodes_request() -> void:
	# Get selected nodes
	var selected_nodes = []
	for node in graph_edit.get_children():
		if node is GraphNode and node.selected:
			selected_nodes.append(node)
	
	# Delete selected nodes
	for node in selected_nodes:
		# Remove all connections to/from this node
		var connections = graph_edit.get_connection_list()
		for connection in connections:
			if connection["from_node"] == node.name or connection["to_node"] == node.name:
				graph_edit.disconnect_node(
					connection["from_node"],
					connection["from_port"],
					connection["to_node"],
					connection["to_port"]
				)
		# Delete the node
		node.queue_free()
	
	# Save graph after deletion
	save_graph()

func _unhandled_key_input(event: InputEvent) -> void:
	if event is InputEventKey:
		if event.keycode == KEY_DELETE:
			_on_delete_nodes_request()
			get_viewport().set_input_as_handled()

func _input(event: InputEvent) -> void:
	if event is InputEventMouseButton:
		if event.button_index == MOUSE_BUTTON_LEFT and not event.pressed:
			_handle_button_release()

func update_sim_time_label() -> void:
	var sim_time_minutes = round(sim_time * time_unit)
	sim_time_label.text = "Sim Time: " + str(sim_time_minutes) + " minutes"

func create_buttons() -> void:
	
	var resource_paths = Utils.get_scene_paths("res://ui/resources/")
	var facility_paths = Utils.get_scene_paths("res://ui/facilities/")
	
	print(resource_paths, facility_paths)
	for path in resource_paths + facility_paths:
		print(path)
		var button = Button.new()
		button.text = path.get_file().get_basename()
		button.connect("button_down", func(): _on_button_down(path))
		button.connect("button_up", _on_button_up)
		button_container.add_child(button)

func _on_button_down(path: String) -> void:
	dragging_node_path = path
	dragging_new_node = true

func _on_button_up() -> void:
	if dragging_new_node:
		var mouse_pos = graph_edit.get_local_mouse_position()
		if graph_edit.get_rect().has_point(mouse_pos):
			var graph_pos = (mouse_pos + graph_edit.scroll_offset) / graph_edit.zoom
			add_node_from_path(dragging_node_path, graph_pos)
		dragging_new_node = false
	dragging_node_path = ""

func _on_node_selected(node: Node):
	properties.update_properties(node)

func _on_node_deselected(node: Node):
	if properties.current_node == node:
		properties.clear_properties()

func save_as_nft() -> void:
	print('save_as_nft')
	var save_data = {
		"nodes": {},
		"connections": [],
		"view": {
			"scroll_offset": graph_edit.scroll_offset,
			"zoom": graph_edit.zoom
		}
	}
	
	# Save minimal node data for testing
	for node in graph_edit.get_children():
		if node is GraphNode:
			save_data["nodes"][node.name] = {
				"pos": [node.position_offset.x, node.position_offset.y],
				"type": node.scene_file_path.get_file()
			}
	
	# Save minimal connection data
	for connection in graph_edit.get_connection_list():
		save_data["connections"].append([
			connection["from_node"],
			connection["from_port"],
			connection["to_node"],
			connection["to_port"]
		])
	
	var web3 = get_node("/root/Web3Interface")
	web3.mint_design(save_data)



func _on_nft_minted(token_id: int) -> void:
	print("Design saved as NFT with token ID: ", token_id)

func load_from_nft(token_id: int) -> void:
	var web3 = get_node("/root/Web3Interface")
	web3.load_design(token_id)

func _on_nft_load_complete(design_data: Dictionary) -> void:
	# Clear current graph
	new_graph()
	
	# Load nodes
	for node_name in design_data.nodes:
		var node_data = design_data.nodes[node_name]
		var node = load(node_data.type).instantiate()
		node.name = node_name
		node.position_offset = Vector2(node_data.pos[0],node_data.pos[1])

		if node_data.has("properties"):
			node.load_facility_data(node_data.properties)
		graph_edit.add_child(node)
	
	# Load connections
	for connection in design_data.connections:
		graph_edit.connect_node(
			connection[0],
			int(connection[1]),
			connection[2],
			int(connection[3])
		)
	pause_simulation()

func _on_save_nft_pressed() -> void:
	print('save_nft_pressed')
	# Check if wallet is connected
	#if not nft_manager.web3_interface.is_connected():
		#show_message("Please connect your wallet first")
		#return
	
	save_as_nft()

func _on_load_nft_pressed() -> void:
	# Check if wallet is connected
	#if not nft_manager.web3_interface.is_connected():
		#show_message("Please connect your wallet first")
		#return
		
	## Show token ID input dialog
	#var dialog = load("res://modules/supply_chain_modeling/ui/token_id_dialog.tscn").instantiate()
	#add_child(dialog)
	#dialog.connect("token_id_entered", func(token_id): load_from_nft(int(token_id)))
	#dialog.popup_centered()
	#
	load_from_nft(int(1))

func _on_view_nfts_pressed() -> void:
	# Check if wallet is connected
	if not Web3Interface.is_connected:
		show_message("Please connect your wallet first")
		return
		
	# Show NFT gallery dialog
	var gallery = load("res://modules/supply_chain_modeling/ui/nft_gallery.tscn").instantiate()
	add_child(gallery)
	gallery.connect("nft_selected", func(token_id): load_from_nft(token_id))
	gallery.popup_centered()

func show_message(text: String) -> void:
	var dialog = AcceptDialog.new()
	dialog.dialog_text = text
	add_child(dialog)
	dialog.popup_centered()

func _on_wallet_connected(address: String):
	print("Wallet connected: ", address)
	# Enable NFT-related features
	# You might want to enable certain UI elements or functionality here

func _on_wallet_disconnected():
	print("Wallet disconnected")
	# Disable NFT-related features
	# You might want to disable certain UI elements or functionality here

func _on_save_file_selected(path: String) -> void:
	var graph_data = {
		"nodes": [],
		"connections": []
	}
	
	# Save all nodes
	for node in %GraphEdit.get_children():
		if node is GraphNode:
			var node_data = {
				"name": node.name,
				"type": node.get_class(),
				"position": [node.position_offset.x, node.position_offset.y],
				"properties": node.get_facility_data() if "get_facility_data" in node else {}
			}
			graph_data["nodes"].append(node_data)
	
	# Save all connections
	for connection in %GraphEdit.get_connection_list():
		graph_data["connections"].append({
			"from_node": connection["from_node"],
			"from_port": connection["from_port"],
			"to_node": connection["to_node"],
			"to_port": connection["to_port"]
		})
	
	# Save to file
	var file = FileAccess.open(path, FileAccess.WRITE)
	file.store_string(JSON.stringify(graph_data, "  "))
	file.close()

func _on_load_file_selected(path: String) -> void:
	# Clear existing graph
	new_graph()
	
	# Load file
	var file = FileAccess.open(path, FileAccess.READ)
	var json_string = file.get_as_text()
	file.close()
	
	var json = JSON.new()
	var parse_result = json.parse(json_string)
	if parse_result != OK:
		print("JSON Parse Error: ", json.get_error_message())
		return
		
	var graph_data = json.get_data()
	
	# Create nodes
	for node_data in graph_data["nodes"]:
		var node_type = node_data["type"]
		var node_scene = load("res://facilities/" + node_type.to_lower() + ".tscn")
		if node_scene:
			var node = node_scene.instantiate()
			node.name = node_data["name"]
			node.position_offset = Vector2(node_data["position"][0], node_data["position"][1])
			if "properties" in node_data and "load_facility_data" in node:
				node.load_facility_data(node_data["properties"])
			%GraphEdit.add_child(node)
	
	# Restore connections
	for connection in graph_data["connections"]:
		%GraphEdit.connect_node(
			connection["from_node"],
			connection["from_port"],
			connection["to_node"],
			connection["to_port"]
		)

# Add these functions to handle button presses
func _on_save_button_pressed():
	save_dialog.popup_centered(Vector2(800, 600))

func _on_load_button_pressed():
	load_dialog.popup_centered(Vector2(800, 600))

func _on_file_menu_pressed(id: int) -> void:
	match id:
		0:  # New
			new_graph()
		2:  # Save
			save_graph()
		3:  # Load
			load_graph()
		5:  # Save File
			%SaveDialog.popup_centered(Vector2(800, 600))
		6:  # Load File
			%LoadDialog.popup_centered(Vector2(800, 600))

func _on_nft_menu_pressed(id: int) -> void:
	match id:
		0:  # Mint NFT
			save_as_nft()
		1:  # Load NFT
			_on_load_nft_pressed()
		2:  # Show NFTs
			_on_view_nfts_pressed()


	
