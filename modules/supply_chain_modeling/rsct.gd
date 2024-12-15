@tool
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
var save_file_path: String = "user://current_graph.save"
var autosave_timer: float = 0.0
var sim_time : float = 0.0
var time_scale: float = 1.0  # Default time scale
var time_unit: float = 60.0  # Default time unit
var paused: bool = true  # Simulation paused state
var dragging_new_node: bool = false
var dragging_node_path: String = ""

# === Initialization ===
func _ready():
	_connect_signals()
	load_graph()
	update_sim_time_label()
	pause_simulation()
	create_buttons()

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
	
	%FileMenu.id_pressed.connect(_on_file_menu_pressed)
	%NFTMenu.id_pressed.connect(_on_nft_menu_pressed)
	
	save_dialog.file_selected.connect(_on_save_dialog_file_selected)
	load_dialog.file_selected.connect(_on_load_dialog_file_selected)

# === Core Processing ===
func _process(delta: float) -> void:
	_handle_autosave(delta)
	update_sim_time_label()

func _physics_process(delta: float) -> void:
	if not paused:
		sim_time += delta * time_scale

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
	time_scale = new_scale

func pause_simulation() -> void:
	paused = true
	for node in graph_edit.get_children():
		if node is GraphNode:
			node.set_physics_process(false)

func resume_simulation() -> void:
	paused = false
	for node in graph_edit.get_children():
		if node is GraphNode:
			node.set_physics_process(true)

# === Graph Management ===
func new_graph() -> void:
	for node in graph_edit.get_children():
		if node is GraphNode:
			node.free()
	
	pause_simulation()
	sim_time = 0.0
	update_sim_time_label()
	graph_edit.scroll_offset = Vector2.ZERO
	save_graph()

# === Node Management ===
func add_node_from_path(path: String, position: Vector2 = Vector2.ZERO):
	var node_scene = load(path)
	if node_scene:
		var node = node_scene.instantiate()
		graph_edit.add_child(node)
		node.set_owner(null)
		node.set_physics_process(!paused)
		
		if position == Vector2.ZERO:
			var viewport_size = graph_edit.size
			var scroll_offset = graph_edit.scroll_offset
			var zoom = graph_edit.zoom
			var center_x = (scroll_offset.x + viewport_size.x / 2) / zoom
			var center_y = (scroll_offset.y + viewport_size.y / 2) / zoom
			node.position_offset = Vector2(center_x - node.size.x / 2, center_y - node.size.y / 2)
		else:
			node.position_offset = position - node.size / 2
		
		save_graph()

# === UI Management ===
func create_buttons() -> void:
	var resource_paths = Utils.get_scene_paths("res://ui/resources/")
	var facility_paths = Utils.get_scene_paths("res://ui/facilities/")
	
	for path in resource_paths + facility_paths:
		var button = Button.new()
		button.text = path.get_file().get_basename()
		button.connect("button_down", func(): _on_button_down(path))
		button.connect("button_up", _on_button_up)
		button_container.add_child(button)

func update_sim_time_label() -> void:
	var sim_time_minutes = round(sim_time * time_unit)
	sim_time_label.text = "Sim Time: " + str(sim_time_minutes) + " minutes"

func show_message(text: String) -> void:
	var dialog = AcceptDialog.new()
	dialog.dialog_text = text
	add_child(dialog)
	dialog.popup_centered()

# === File Operations ===
func save_graph() -> void:
	var file = FileAccess.open(save_file_path, FileAccess.WRITE)
	if not file:
		show_message("Error: Could not save file")
		return
	
	var save_data := {
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
				"position": node.position_offset,
				"size": node.size,
				"type": node.scene_file_path
			}
	
	for connection in graph_edit.get_connection_list():
		save_data["connections"].append({
			"from_node": connection["from_node"],
			"from_port": connection["from_port"],
			"to_node": connection["to_node"],
			"to_port": connection["to_port"]
		})
	
	file.store_var(save_data)
	print("Graph autosaved successfully")

func load_graph() -> void:
	if not FileAccess.file_exists(save_file_path):
		print("No save file exists")
		return
	
	var file = FileAccess.open(save_file_path, FileAccess.READ)
	if not file:
		show_message("Error: Could not open file")
		return
	
	var save_data = file.get_var()
	if not save_data:
		return
	
	new_graph()
	
	for node_name in save_data["nodes"]:
		var node_data = save_data["nodes"][node_name]
		var node_scene = load(node_data["type"])
		if node_scene:
			var node = node_scene.instantiate()
			node.name = node_name
			node.position_offset = node_data["position"]
			node.size = node_data["size"]
			graph_edit.add_child(node)
			node.set_owner(null)
	
	for connection in save_data["connections"]:
		graph_edit.connect_node(
			connection["from_node"],
			connection["from_port"],
			connection["to_node"],
			connection["to_port"]
		)
	
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
	
	var web3 = get_node("/root/Web3Interface")
	web3.mint_design(save_data)

func load_from_nft(token_id: int) -> void:
	var web3 = get_node("/root/Web3Interface")
	web3.load_design(token_id)

# === Signal Handlers ===
# -- Graph Edit Signals --
func _on_connection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> void:
	if simulation.connect_nodes(from_node, to_node, from_port):
		graph_edit.connect_node(from_node, from_port, to_node, to_port)

func _on_disconnection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> void:
	simulation.disconnect_nodes(from_node, to_node)
	graph_edit.disconnect_node(from_node, from_port, to_node, to_port)

func _on_node_moved() -> void:
	save_graph()

func _on_delete_nodes_request(nodes: Array) -> void:
	for node_name in nodes:
		var node = graph_edit.get_node(node_name)
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

# -- Menu Signals --
func _on_file_menu_pressed(id: int) -> void:
	match id:
		0:  # New
			new_graph()
		1:  # Save
			save_dialog.popup_centered()
		2:  # Load
			load_dialog.popup_centered()

func _on_nft_menu_pressed(id: int) -> void:
	match id:
		0:  # Save as NFT
			save_as_nft()
		1:  # Load from NFT
			# Implement NFT loading dialog
			pass

# -- Web3 Interface Signals --
func _on_wallet_connected() -> void:
	print("Wallet connected")

func _on_wallet_disconnected() -> void:
	print("Wallet disconnected")

func _on_nft_minted(token_id: int) -> void:
	show_message("Design saved as NFT #" + str(token_id))

func _on_nft_load_complete(design_data: Dictionary) -> void:
	new_graph()
	
	for node_name in design_data["nodes"]:
		var node_data = design_data["nodes"][node_name]
		var node_scene = load("res://scenes/" + node_data["type"])
		if node_scene:
			var node = node_scene.instantiate()
			node.name = node_name
			node.position_offset = Vector2(node_data["pos"][0], node_data["pos"][1])
			graph_edit.add_child(node)
			node.set_owner(null)
	
	for connection in design_data["connections"]:
		graph_edit.connect_node(
			connection[0],
			connection[1],
			connection[2],
			connection[3]
		)

# -- Dialog Signals --
func _on_save_dialog_file_selected(path: String) -> void:
	save_file_path = path
	save_graph()

func _on_load_dialog_file_selected(path: String) -> void:
	save_file_path = path
	load_graph()



	
