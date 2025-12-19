extends Control

# Node references
@onready var graph_edit: GraphView = %GraphEdit
@onready var tab_container: TabContainer = %TabContainer

@onready var button_container: VBoxContainer = %NewNodesMenu
@onready var properties: PropertiesEditor = %Properties

@onready var save_dialog: FileDialog = %SaveDialog
@onready var load_dialog: FileDialog = %LoadDialog

@onready var simulation: SimulationManager = %Simulation

# Dependencies
var Web3Interface
var Utils

# State variables
var DEFAULT_SAVE_PATH: String = "user://supply_chain_graph.save"

var dragging_new_node: bool = false
var dragging_node_path: String = ""

# === Initialization ===

func _init():
	# Initialize dependencies
	if Engine.has_singleton("Web3Interface"):
		Web3Interface = Engine.get_singleton("Web3Interface")
	else:
		Web3Interface = load("res://apps/supply_chain_modeling/singletons/web3_interface.gd").new()
		add_child(Web3Interface)
	
	if Engine.has_singleton("Utils"):
		Utils = Engine.get_singleton("Utils")
	else:
		Utils = load("res://apps/supply_chain_modeling/singletons/utils.gd").new()
		add_child(Utils)
	
	Utils.initialize_class_map("res://apps/supply_chain_modeling/simulation/resources/")
	Utils.initialize_class_map("res://apps/supply_chain_modeling/simulation/facilities/")
	Utils.initialize_class_map("res://apps/supply_chain_modeling/simulation/other/")

func _ready():
	pause_simulation()
	_connect_signals()

	load_graph()
	save_graph() # Hack to fix the bug that after loading form file info is deleted
	
func _connect_signals() -> void:
	# Connect menu signals
	var menu_bar = $UI/MenuContainer/MenuBar
	
	# Only connect signals if they're not already connected
	if !menu_bar.new_graph_requested.is_connected(new_graph):
		menu_bar.new_graph_requested.connect(new_graph)
	if !menu_bar.save_requested.is_connected(save_graph):
		menu_bar.save_requested.connect(func(): save_graph())
	if !menu_bar.load_requested.is_connected(load_graph):
		menu_bar.load_requested.connect(func(): load_graph())
	if !menu_bar.save_to_file_requested.is_connected(_on_save_to_file_requested):
		menu_bar.save_to_file_requested.connect(_on_save_to_file_requested)
	if !menu_bar.load_from_file_requested.is_connected(_on_load_from_file_requested):
		menu_bar.load_from_file_requested.connect(_on_load_from_file_requested)
	if !menu_bar.return_to_launcher_requested.is_connected(_on_return_to_launcher_requested):
		menu_bar.return_to_launcher_requested.connect(_on_return_to_launcher_requested)
	if !menu_bar.switch_tab_requested.is_connected(_on_switch_tab_requested):
		menu_bar.switch_tab_requested.connect(_on_switch_tab_requested)
	
	Web3Interface.connect("wallet_connected", _on_wallet_connected)
	Web3Interface.connect("wallet_disconnected", _on_wallet_disconnected)
	Web3Interface.connect("nft_minted", _on_nft_minted)
	Web3Interface.connect("nft_load_complete", _on_nft_load_complete)

# === Core Processing ===
func _handle_autosave() -> void:
	save_graph()

func _input(event: InputEvent) -> void:
	if event is InputEventMouseMotion and dragging_new_node:
		# Handle preview or visual feedback while dragging
		get_viewport().set_input_as_handled()

# === Simulation Control ===
func set_time_scale(new_scale: float) -> void:
	simulation.set_time_scale(new_scale)

func toggle_simulation() -> void:
	simulation.toggle_simulation()

func pause_simulation() -> void:
	simulation.pause_simulation()

func resume_simulation() -> void:
	simulation.resume_simulation()

func set_simulation_status(_paused: bool):
	simulation.set_simulation_status(_paused)

# === Graph Management ===
func new_graph() -> void:
	# Step 1: Clear simulation first
	simulation.new_simulation()
	
	# Step 2: Clear UI nodes
	graph_edit.clear_graph()
	
	# Step 3: Reset simulation state and view
	graph_edit.scroll_offset = Vector2.ZERO
	graph_edit.zoom = 1.0
	save_graph()

# === Node Management ===
func add_node_from_path(path: String, _position: Vector2 = Vector2.ZERO):
	var sim_node = simulation.add_node_from_path(path)
	graph_edit.add_ui_for_node(sim_node, _position)
	save_graph()

# === UI Management ===
func show_message(text: String) -> void:
	var dialog = AcceptDialog.new()
	dialog.dialog_text = text
	add_child(dialog)
	dialog.popup_centered()

# === Save/Load ===
func graph_to_save_data() -> Dictionary:
	var save_data := {
		"simulation": simulation.save_state(),
		"ui": graph_edit.get_ui_state(),
		"view": graph_edit.get_view_state()
	}
	
	return save_data

func graph_from_save_data(save_data: Dictionary) -> void:
	# Clear existing graph
	new_graph()
	
	# Load simulation state first (this creates the simulation nodes)
	simulation.load_state(save_data["simulation"])
	
	# Create UI nodes for each simulation node
	for node_name in save_data["simulation"]["nodes"]:
		var sim_node = simulation.get_node_or_null(NodePath(node_name))
		
		if sim_node:
			graph_edit.create_ui_node(sim_node)
			
	if "ui" in save_data:
		for node_name in save_data["ui"]:
			var ui_node = graph_edit.get_node_or_null(NodePath(node_name))
			if ui_node:
				# Set UI properties
				ui_node.position_offset = Vector2(save_data["ui"][node_name]["position"][0], 
												save_data["ui"][node_name]["position"][1])
				
				if "size" in save_data["ui"][node_name]:
					ui_node.size = Vector2(save_data["ui"][node_name]["size"][0], 
										save_data["ui"][node_name]["size"][1])
	
	# Recreate connections
	if "connections" in save_data["simulation"]:
		for connection in save_data["simulation"]["connections"]:
			graph_edit.connect_node(
				connection["from_node"],
				connection["from_port"],
				connection["to_node"],
				connection["to_port"]
			)
	
	# Restore view state
	if "view" in save_data:
		graph_edit.call_deferred("set_scroll_offset", 
			Vector2(save_data["view"]["scroll_offset"][0], 
				   save_data["view"]["scroll_offset"][1]))
		if "zoom" in save_data["view"]:
			graph_edit.zoom = save_data["view"]["zoom"]
	
	pause_simulation()

# === File Operations ===
func save_graph(save_path: String = DEFAULT_SAVE_PATH) -> void:
	var file = FileAccess.open(save_path, FileAccess.WRITE)
	if not file:
		show_message("Error: Could not save file")
		return
	
	var save_data = graph_to_save_data()
	file.store_string(JSON.stringify(save_data))
	print("Graph saved successfully")

func load_graph(load_file_path: String = DEFAULT_SAVE_PATH) -> void:
	if not FileAccess.file_exists(load_file_path):
		print("No save file exists")
		return
	
	var file = FileAccess.open(load_file_path, FileAccess.READ)
	if not file:
		show_message("Error: Could not open file")
		return
	
	var json = JSON.new()
	if json.parse(file.get_as_text()) != OK:    
		show_message("Error: Could not parse file")
		return
	
	var save_data = json.data
	if not save_data:
		return
	
	graph_from_save_data(save_data)
	print("Load complete")

# === NFT Operations ===
func save_as_nft() -> void:
	print('save_as_nft')
	var save_data = graph_to_save_data()
	Web3Interface.mint_design(save_data)

func load_from_nft(token_id: int) -> void:
	Web3Interface.load_design(token_id)

# === Signal Handlers ===
# -- Graph Edit Signals --
func _on_connection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> void:
	var result = simulation.connect_nodes(from_node, from_port, to_node, to_port)
	if result.success:
		graph_edit.connect_node(from_node, from_port, to_node, to_port)
	else:
		print("RSCT: Connection rejected: %s" % result.message)
	save_graph()

func _on_disconnection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> void:
	if simulation.disconnect_nodes(from_node, from_port, to_node, to_port):
		graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
	save_graph()

func _on_node_moved() -> void:
	save_graph()

func _on_delete_nodes_request(nodes: Array) -> void:
	for node_name in nodes:
		simulation.remove_node(NodePath(node_name))
		var node = graph_edit.get_node(NodePath(node_name))
		if node:
			node.queue_free()
	save_graph()

func _on_node_selected(node: Node) -> void:
	var sim_node = simulation.get_node(NodePath(node.name))
	if sim_node:
		properties.update_properties(sim_node)

func _on_node_deselected(node: Node) -> void:
	properties.clear_properties()

# -- Button Signals --
func _on_button_down(path: String) -> void:
	dragging_new_node = true
	dragging_node_path = path

func _on_button_up() -> void:
	if dragging_new_node:
		var mouse_pos = graph_edit.get_local_mouse_position()
		if graph_edit.get_rect().has_point(mouse_pos):
			var graph_pos = (mouse_pos + graph_edit.scroll_offset) / graph_edit.zoom
			add_node_from_path(dragging_node_path, graph_pos)
		else:
			# If not dropped on graph, create at center
			add_node_from_path(dragging_node_path)
	
	dragging_new_node = false
	dragging_node_path = ""

# -- Web3 Interface Signals --
func _on_wallet_connected() -> void:
	print("Wallet connected")

func _on_wallet_disconnected() -> void:
	print("Wallet disconnected")

func _on_nft_minted(_token_id: int) -> void:
	print("NFT minted successfully")

func _on_nft_load_complete(token_data: Dictionary) -> void:
	if "metadata" in token_data and "graph_data" in token_data["metadata"]:
		graph_from_save_data(token_data["metadata"]["graph_data"])

# -- Dialog Signals --
func _on_save_dialog_file_selected(path: String) -> void:
	save_graph(path)

func _on_load_dialog_file_selected(path: String) -> void:
	load_graph(path)

func _on_save_to_file_requested() -> void:
	save_dialog.popup_centered(Vector2(800, 600))

func _on_load_from_file_requested() -> void:
	load_dialog.popup_centered(Vector2(800, 600))

func _on_return_to_launcher_requested() -> void:
	get_tree().change_scene_to_file("res://apps/launcher/launcher.tscn")

# === Tab Management ===
func _on_switch_tab_requested(tab_index: int) -> void:
	if tab_container and tab_index >= 0 and tab_index < tab_container.get_tab_count():
		tab_container.current_tab = tab_index

# === External Graph Inspection ===

## Inspect an external solver graph (e.g., from a spacecraft)
func inspect_graph(graph: LCSolverGraph):
	if not graph:
		return
	
	# Check if nodes are ready
	if not graph_edit or not simulation:
		push_warning("RSCT: graph_edit or simulation not ready, cannot inspect graph")
		return
	
	# Clear current simulation/graph (only if simulation exists)
	if simulation and simulation.has_method("new_simulation"):
		simulation.new_simulation()
	
	# Clear UI nodes
	if graph_edit and graph_edit.has_method("clear_graph"):
		graph_edit.clear_graph()
	
	# Load the external graph into the view
	if graph_edit and graph_edit.has_method("load_from_solver_graph"):
		graph_edit.load_from_solver_graph(graph)
	
	# Disable simulation controls since we're viewing an external simulation
	if simulation:
		simulation.paused = true
	
	print("RSCT: Inspecting external solver graph with ", graph.nodes.size(), " nodes")
