extends Control

# Node references
@onready var graph_edit: GraphEdit = %GraphEdit
@onready var sim_time_label: Label = %SimTimeLabel
@onready var button_container: VBoxContainer = %ButtonContainer
@onready var properties: VBoxContainer = %Properties
@onready var save_dialog: FileDialog = %SaveDialog
@onready var load_dialog: FileDialog = %LoadDialog
@onready var simulation: SimulationManager = %Simulation

# Constants
const AUTOSAVE_INTERVAL: float = 60000.0  # Autosave every 60 seconds

# State variables

var DEFAULT_SAVE_PATH: String = "user://current_graph.save"
var autosave_timer: float = 0.0

var dragging_new_node: bool = false
var dragging_node_path: String = ""

# === Initialization ===
func _ready():
	pause_simulation()
	_connect_signals()
	create_buttons()
	
	# load_graph()
	# save_graph() # Hack to fix the bug that after loading form file info is deleted

	for node in simulation.get_children():
		#print(node, node.get_class())
		#if node is SimulationNode:
		var ui_node
		
		if node is BaseFacility:
			print("facility")
		elif node is ResourceH2:
			print("resourceh2")
		else:
			ui_node = UISimulationNode.new()
		
		if ui_node:
			graph_edit.add_child(ui_node)
		

func _connect_signals():
	Web3Interface.connect("wallet_connected", _on_wallet_connected)
	Web3Interface.connect("wallet_disconnected", _on_wallet_disconnected)
	Web3Interface.connect("nft_minted", _on_nft_minted)
	Web3Interface.connect("nft_load_complete", _on_nft_load_complete)
	
	graph_edit.connect("connection_request", _on_connection_request)
	graph_edit.connect("disconnection_request", _on_disconnection_request)
	graph_edit.connect("end_node_move", _on_node_moved)
	graph_edit.connect("delete_nodes_request", _on_delete_nodes_request)
	graph_edit.connect("node_selected", _on_node_selected)
	graph_edit.connect("node_deselected", _on_node_deselected)
	
	
	save_dialog.file_selected.connect(_on_save_dialog_file_selected)
	load_dialog.file_selected.connect(_on_load_dialog_file_selected)

# === Core Processing ===
func _process(delta: float) -> void:
	_handle_autosave(delta)
	update_sim_time_label()

func _handle_autosave(delta: float) -> void:
	autosave_timer += delta
	if autosave_timer >= AUTOSAVE_INTERVAL:
		autosave_timer = 0.0
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
	for node in graph_edit.get_children():
		if node is GraphNode:
			node.free()
	
	pause_simulation()
	
	graph_edit.scroll_offset = Vector2.ZERO
	save_graph()

# === Node Management ===
func add_node_from_path(path: String, position: Vector2 = Vector2.ZERO):
	var node_script = load(path)
	if node_script:

		var sim_node = Node.new()
		sim_node.set_script(node_script)
		

		var ui_node = UISimulationNode.new()

		sim_node.set_owner(null)
		ui_node.set_owner(null)

		simulation.add_child(sim_node)
		graph_edit.add_child(ui_node)

		ui_node.set_physics_process(false)
		
		if position == Vector2.ZERO:
			var viewport_size = graph_edit.size
			var scroll_offset = graph_edit.scroll_offset
			var zoom = graph_edit.zoom
			var center_x = (scroll_offset.x + viewport_size.x / 2) / zoom
			var center_y = (scroll_offset.y + viewport_size.y / 2) / zoom
			ui_node.position_offset = Vector2(center_x - ui_node.size.x / 2, center_y - ui_node.size.y / 2)
		else:
			ui_node.position_offset = position - ui_node.size / 2
		
		save_graph()

# === UI Management ===
func create_buttons() -> void:
	var resource_paths = Utils.get_paths("res://simulation/resources/")
	var facility_paths = Utils.get_paths("res://simulation/facilities/")
	
	for path in resource_paths + facility_paths:
		var button = Button.new()
		button.text = path.get_file().get_basename()
		button.connect("button_down", func(): _on_button_down(path))
		button.connect("button_up", _on_button_up)
		button_container.add_child(button)

func update_sim_time_label() -> void:
	sim_time_label.text = "Sim Time: " + str(simulation.get_simulation_time_scaled()) + " minutes"

func show_message(text: String) -> void:
	var dialog = AcceptDialog.new()
	dialog.dialog_text = text
	add_child(dialog)
	dialog.popup_centered()

# === File Operations ===
func save_graph(save_path: String = DEFAULT_SAVE_PATH) -> void:
	var file = FileAccess.open(save_path, FileAccess.WRITE)
	if not file:
		show_message("Error: Could not save file")
		return
	
	var save_data := {
		"simulation": simulation.save_state(),
		"view": {
			"scroll_offset": graph_edit.scroll_offset,
			"zoom": graph_edit.zoom
		}
	}
	
	# Save UI node positions
	for node in graph_edit.get_children():
		if node is GraphNode:
			if node.name in save_data["simulation"]["nodes"]:
				save_data["simulation"]["nodes"][node.name]["ui"] = {
					"position": node.position_offset,
					"size": node.size
				}
	
	file.store_var(save_data)
	print("Graph saved successfully")

func load_graph(load_file_path: String = DEFAULT_SAVE_PATH) -> void:
	if not FileAccess.file_exists(load_file_path):
		print("No save file exists")
		return
	
	var file = FileAccess.open(load_file_path, FileAccess.READ)
	if not file:
		show_message("Error: Could not open file")
		return
	
	var save_data = file.get_var()
	if not save_data:
		return
	
	# Clear existing graph
	new_graph()
	
	# Load simulation state
	simulation.load_state(save_data["simulation"])
	
	# Create UI nodes for simulation nodes
	for node_name in save_data["simulation"]["nodes"]:
		var node_data = save_data["simulation"]["nodes"][node_name]
		var ui_node = UISimulationNode.new()
		ui_node.name = node_name
		
		# Set UI properties if available
		if "ui" in node_data:
			ui_node.position_offset = node_data["ui"]["position"]
			ui_node.size = node_data["ui"]["size"]
		
		graph_edit.add_child(ui_node)
	
	# Restore view state
	if "view" in save_data:
		graph_edit.call_deferred("set_scroll_offset", save_data["view"]["scroll_offset"])
		if "zoom" in save_data["view"]:
			graph_edit.zoom = save_data["view"]["zoom"]
	
	pause_simulation()
	print("Load complete")

# === NFT Operations ===
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
	
	for node in graph_edit.get_children():
		if node is GraphNode:
			save_data["nodes"][node.name] = {
				"pos": [node.position_offset.x, node.position_offset.y],
				"type": node.scene_file_path.get_file()
			}
	
	for connection in graph_edit.get_connection_list():
		save_data["connections"].append([
			connection["from_node"],
			connection["from_port"],
			connection["to_node"],
			connection["to_port"]
		])
	
	Web3Interface.mint_design(save_data)

func load_from_nft(token_id: int) -> void:
	Web3Interface.load_design(token_id)

# === Signal Handlers ===
# -- Graph Edit Signals --
func _on_connection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> void:
	if simulation.connect_nodes(from_node, from_port, to_node, from_port):
		graph_edit.connect_node(from_node, from_port, to_node, to_port)
	save_graph()

func _on_disconnection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> void:
	if simulation.disconnect_nodes(from_node, from_port, to_node, to_port):
		graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
	save_graph()

func _on_node_moved() -> void:
	save_graph()

func _on_delete_nodes_request(nodes: Array) -> void:
	for node_name in nodes:
		var node = graph_edit.get_node(NodePath(node_name))
		if node:
			node.queue_free()
	save_graph()

func _on_node_selected(node: Node) -> void:
	if node.has_method("show_properties"):
		node.show_properties(properties)

func _on_node_deselected(node: Node) -> void:
	if node.has_method("hide_properties"):
		node.hide_properties(properties)

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

func _on_nft_minted(token_id: int) -> void:
	show_message("Design saved as NFT #" + str(token_id))

func _on_nft_load_complete(design_data: Dictionary) -> void:
	new_graph()
	# TBD Load from dics

# -- Dialog Signals --
func _on_save_dialog_file_selected(path: String) -> void:
	save_graph(path)

func _on_load_dialog_file_selected(path: String) -> void:
	load_graph(path)

func _on_save_to_file_requested() -> void:
	save_dialog.popup_centered(Vector2(800, 600))

func _on_load_from_file_requested() -> void:
	load_dialog.popup_centered(Vector2(800, 600))
	

	
