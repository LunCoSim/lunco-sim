extends Control

@onready var loader: MOLoader
@onready var status_label = $UI/Toolbar/HBoxContainer/StatusLabel
@onready var graph_edit = $UI/GraphEdit

var component_count = 0

func _ready():
	print("Starting main scene")
	loader = MOLoader.new()
	add_child(loader)
	
	# Wait one frame for loader to initialize
	await get_tree().process_frame
	
	# Connect UI signals
	_connect_ui_signals()
	
	# Connect GraphEdit signals
	graph_edit.connection_request.connect(_on_connection_request)
	graph_edit.disconnection_request.connect(_on_disconnection_request)
	
	# Set GraphEdit properties
	graph_edit.snapping_enabled = true
	graph_edit.snapping_distance = 20
	graph_edit.show_grid = true

func _connect_ui_signals():
	# Connect component buttons
	var component_buttons = {
		"VoltageSourceBtn": "VoltageSource",
		"ResistorBtn": "Resistor",
		"CapacitorBtn": "Capacitor",
		"InductorBtn": "Inductor",
		"GroundBtn": "Ground",
		"SpringBtn": "Spring",
		"MassBtn": "Mass",
		"DamperBtn": "Damper",
		"GroundMechBtn": "GroundMech"
	}
	
	for btn_name in component_buttons:
		var button = get_node_or_null(NodePath("UI/ComponentPanel/VBoxContainer/" + btn_name))
		if button:
			button.pressed.connect(_on_component_button_pressed.bind(component_buttons[btn_name]))
	
	# Connect toolbar buttons
	var simulate_btn = $UI/Toolbar/HBoxContainer/SimulateBtn
	var stop_btn = $UI/Toolbar/HBoxContainer/StopBtn
	simulate_btn.pressed.connect(_on_simulate_pressed)
	stop_btn.pressed.connect(_on_stop_pressed)

func _on_component_button_pressed(component_type: String):
	var node = _create_component_node(component_type)
	if node:
		graph_edit.add_child(node)
		status_label.text = "Added " + component_type

func _create_component_node(component_type: String) -> GraphNode:
	var node = GraphNode.new()
	var unique_name = component_type + str(component_count)
	component_count += 1
	
	# Set up the node
	node.name = unique_name
	node.title = component_type
	node.position_offset = Vector2(200, 200)  # Default position
	node.draggable = true
	node.resizable = false

	
	# Create the main container
	var container = VBoxContainer.new()
	container.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	container.size_flags_vertical = Control.SIZE_EXPAND_FILL
	container.mouse_filter = Control.MOUSE_FILTER_IGNORE
	node.add_child(container)
	
	# Add the main body
	var body = ColorRect.new()
	body.custom_minimum_size = Vector2(100, 50)
	body.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	body.size_flags_vertical = Control.SIZE_EXPAND_FILL
	body.mouse_filter = Control.MOUSE_FILTER_IGNORE  # Allow clicks to pass through
	match component_type:
		"VoltageSource":
			body.color = Color(0.2, 0.6, 1.0)  # Light blue
		"Resistor":
			body.color = Color(0.8, 0.2, 0.2)  # Red
		"Capacitor":
			body.color = Color(0.2, 0.8, 0.2)  # Green
		"Inductor":
			body.color = Color(0.8, 0.2, 0.8)  # Purple
		_:
			body.color = Color(0.7, 0.7, 0.7)  # Gray
	
	container.add_child(body)
	
	# Set up connection slots
	node.set_slot(0,  # Slot index
		true,         # Enable left slot
		0,           # Left slot type
		Color.GOLD,  # Left slot color
		true,        # Enable right slot
		0,           # Right slot type
		Color.GOLD)  # Right slot color
	
	return node

func _on_connection_request(from_node: StringName, from_port: int, 
						  to_node: StringName, to_port: int):
	# Check if connection is valid
	if _can_connect(from_node, to_node):
		graph_edit.connect_node(from_node, from_port, to_node, to_port)
		status_label.text = "Connected components"
	else:
		status_label.text = "Invalid connection"

func _on_disconnection_request(from_node: StringName, from_port: int,
							 to_node: StringName, to_port: int):
	graph_edit.disconnect_node(from_node, from_port, to_node, to_port)
	status_label.text = "Disconnected components"

func _can_connect(from_node: StringName, to_node: StringName) -> bool:
	# Get the actual nodes
	var from = graph_edit.get_node_or_null(NodePath(from_node))
	var to = graph_edit.get_node_or_null(NodePath(to_node))
	
	if not from or not to:
		return false
	
	# Don't connect a node to itself
	if from == to:
		return false
	
	# Add more connection validation logic here
	# For example, check component compatibility
	
	return true

func _on_simulate_pressed():
	status_label.text = "Simulating..."
	# TODO: Implement simulation

func _on_stop_pressed():
	status_label.text = "Stopped"
	# TODO: Implement simulation stop
