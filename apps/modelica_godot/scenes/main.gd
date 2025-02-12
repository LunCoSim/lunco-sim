extends Node2D

var loader: MOLoader
var components: Dictionary = {}
var connections: Array[Connection] = []
var dragging_component: Node2D = null
var connecting_from: Dictionary = {}  # Stores component and point when starting connection
var hovered_point: Dictionary = {}  # Stores currently hovered connection point
@onready var component_area = $ComponentArea
@onready var camera = $Camera2D
@onready var status_label = $CanvasLayer/UI/Toolbar/HBoxContainer/StatusLabel
@onready var grid = $Grid

# Camera control variables
var camera_drag = false
var camera_drag_start = Vector2()
var camera_start_position = Vector2()
var zoom_speed = 0.1
var min_zoom = 0.1
var max_zoom = 5.0

# Grid settings
const GRID_SIZE = 20
const GRID_COLOR = Color(0.2, 0.2, 0.2)
const GRID_ALPHA = 0.5

func _ready():
	print("Starting main scene")
	loader = MOLoader.new()
	add_child(loader)
	
	# Connect UI signals
	_connect_ui_signals()
	
	# Set initial camera zoom
	camera.zoom = Vector2(1, 1)
	
	# Draw initial grid
	queue_redraw()
	
	# Load mechanical components
	_load_mechanical_components()
	
	# Load electrical components
	_load_electrical_components()
	
	# Create example connections
	_create_example_circuit()

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
		var button = get_node("CanvasLayer/UI/ComponentPanel/VBoxContainer/" + btn_name)
		if button:
			button.pressed.connect(_on_component_button_pressed.bind(component_buttons[btn_name]))
	
	# Connect toolbar buttons
	var simulate_btn = $CanvasLayer/UI/Toolbar/HBoxContainer/SimulateBtn
	var stop_btn = $CanvasLayer/UI/Toolbar/HBoxContainer/StopBtn
	simulate_btn.pressed.connect(_on_simulate_pressed)
	stop_btn.pressed.connect(_on_stop_pressed)

func _on_component_button_pressed(component_type: String):
	var component = _create_component_from_package(
		"res://apps/modelica_godot/components/Electrical/Components.mo",
		component_type
	)
	if component:
		# Position at center of camera view
		var camera_center = camera.get_screen_center_position()
		component.position = camera_center
		_make_component_interactive(component)
		component_area.add_child(component)
		components[component.name + str(components.size())] = component
		status_label.text = "Added " + component_type

func _load_mechanical_components():
	print("Loading mechanical components")
	# Load spring
	var spring = loader.load_component("res://apps/modelica_godot/components/mechanical/Spring.mo")
	if spring:
		print("Spring loaded successfully")
		spring.position = Vector2(200, 200)
		_make_component_interactive(spring)
		component_area.add_child(spring)
		components["spring"] = spring
	else:
		push_error("Failed to load spring component")
	
	# Load mass (once we restore it)
	# var mass = loader.load_component("res://apps/modelica_godot/components/mechanical/Mass.mo")
	# if mass:
	#     mass.position = Vector2(300, 200)
	#     component_area.add_child(mass)
	#     components["mass"] = mass

func _load_electrical_components():
	print("Loading electrical components")
	# Load components from Electrical/Components.mo
	var resistor = _create_component_from_package(
		"res://apps/modelica_godot/components/Electrical/Components.mo",
		"Resistor"
	)
	if resistor:
		resistor.position = Vector2(200, 300)
		_make_component_interactive(resistor)
		component_area.add_child(resistor)
		components["resistor"] = resistor
	
	var voltage_source = _create_component_from_package(
		"res://apps/modelica_godot/components/Electrical/Components.mo",
		"VoltageSource"
	)
	if voltage_source:
		voltage_source.position = Vector2(100, 300)
		_make_component_interactive(voltage_source)
		component_area.add_child(voltage_source)
		components["voltage_source"] = voltage_source

func _make_component_interactive(component: Node2D):
	# Make the component clickable
	for child in component.get_children():
		if child is ColorRect:
			child.mouse_filter = Control.MOUSE_FILTER_STOP
			
			# Connect signals
			child.gui_input.connect(_on_component_gui_input.bind(component, child))
			child.mouse_entered.connect(_on_area_mouse_entered.bind(component, child))
			child.mouse_exited.connect(_on_area_mouse_exited.bind(component, child))

func _on_area_mouse_entered(_component: Node2D, area: ColorRect):
	if area.position.x < -15:  # Left connection point
		hovered_point = {"area": area, "side": "left"}
		area.color = Color.YELLOW
	elif area.position.x > 15:  # Right connection point
		hovered_point = {"area": area, "side": "right"}
		area.color = Color.YELLOW

func _on_area_mouse_exited(_component: Node2D, area: ColorRect):
	if hovered_point.get("area") == area:
		hovered_point.clear()
	area.color = Color.WHITE

func _on_component_gui_input(event: InputEvent, component: Node2D, area: ColorRect):
	if event is InputEventMouseButton:
		if event.button_index == MOUSE_BUTTON_LEFT:
			if event.pressed:
				if area.position.x < -15:  # Left connection point
					_start_connection(component, "left")
				elif area.position.x > 15:  # Right connection point
					_start_connection(component, "right")
				else:  # Main body
					dragging_component = component
			else:
				if dragging_component == component:
					dragging_component = null
				elif not connecting_from.is_empty():
					if area.position.x < -15:  # Left connection point
						_try_complete_connection(component, "left")
					elif area.position.x > 15:  # Right connection point
						_try_complete_connection(component, "right")

func _start_connection(component: Node2D, point: String):
	connecting_from = {
		"component": component,
		"point": point
	}

func _try_complete_connection(end_component: Node2D, end_point: String):
	if connecting_from.is_empty():
		return
		
	var start_component = connecting_from.component
	var start_point = connecting_from.point
	
	# Validate connection
	if Connection.can_connect(start_component, end_component, start_point, end_point):
		# Create new connection
		var connection = Connection.new(start_component, end_component, start_point, end_point)
		connection.connection_clicked.connect(_on_connection_clicked.bind(connection))
		component_area.add_child(connection)
		connections.append(connection)
	
	# Clear connecting state
	connecting_from.clear()

func _on_connection_clicked(connection: Connection):
	# Remove connection
	connections.erase(connection)
	connection.queue_free()

func _process(_delta):
	if dragging_component:
		var mouse_pos = get_viewport().get_mouse_position()
		var snapped_pos = _snap_to_grid(mouse_pos)
		dragging_component.position = snapped_pos
	
	# Queue redraw for grid and connection lines
	queue_redraw()

func _draw():
	# Draw grid
	var view_size = get_viewport_rect().size
	var left = -5000
	var right = 5000
	var top = -5000
	var bottom = 5000
	
	# Draw vertical lines
	for x in range(left, right, GRID_SIZE):
		draw_line(Vector2(x, top), Vector2(x, bottom), GRID_COLOR, 1.0, true)
	
	# Draw horizontal lines
	for y in range(top, bottom, GRID_SIZE):
		draw_line(Vector2(left, y), Vector2(right, y), GRID_COLOR, 1.0, true)
	
	# Draw temporary connection line if connecting
	if not connecting_from.is_empty():
		var start_pos = _get_connection_point_position(
			connecting_from.component,
			connecting_from.point
		)
		var end_pos = get_viewport().get_mouse_position()
		
		# Change color based on whether the current hover would make a valid connection
		var color = Color.WHITE
		if not hovered_point.is_empty():
			var can_connect = Connection.can_connect(
				connecting_from.component,
				get_component_from_area(hovered_point.area),
				connecting_from.point,
				hovered_point.side
			)
			color = Color.GREEN if can_connect else Color.RED
		
		draw_line(start_pos, end_pos, color, 2.0)

func get_component_from_area(area: ColorRect) -> Node2D:
	return area.get_parent() as Node2D

func _get_connection_point_position(component: Node2D, point: String) -> Vector2:
	var offset = Vector2.ZERO
	match point:
		"left":
			offset = Vector2(-20, 0)
		"right":
			offset = Vector2(20, 0)
	return component.global_position + offset

func _create_example_circuit():
	# Create a simple voltage source -> resistor connection
	if components.has("voltage_source") and components.has("resistor"):
		var connection = Connection.new(
			components.voltage_source,
			components.resistor,
			"right",
			"left"
		)
		component_area.add_child(connection)
		connections.append(connection)

func _create_component_from_package(package_path: String, component_name: String) -> Node:
	print("Creating component from package: ", package_path, " component: ", component_name)
	var node = Node2D.new()
	node.name = component_name
	
	match component_name:
		"VoltageSource":
			# Create circle for voltage source
			var circle = ColorRect.new()
			circle.size = Vector2(40, 40)
			circle.position = Vector2(-20, -20)
			circle.color = Color(0.2, 0.6, 1.0)  # Light blue
			node.add_child(circle)
			
			# Add V symbol
			var label = Label.new()
			label.text = "V"
			label.position = Vector2(-5, -10)
			node.add_child(label)
			
			# Add connection points
			var left_point = ColorRect.new()
			left_point.size = Vector2(10, 10)
			left_point.position = Vector2(-25, -5)
			left_point.color = Color(1, 0.8, 0)  # Gold
			node.add_child(left_point)
			
			var right_point = ColorRect.new()
			right_point.size = Vector2(10, 10)
			right_point.position = Vector2(15, -5)
			right_point.color = Color(1, 0.8, 0)  # Gold
			node.add_child(right_point)
			
		"Resistor":
			# Create rectangle for resistor
			var rect = ColorRect.new()
			rect.size = Vector2(40, 20)
			rect.position = Vector2(-20, -10)
			rect.color = Color(0.8, 0.2, 0.2)  # Red
			node.add_child(rect)
			
			# Add R symbol
			var label = Label.new()
			label.text = "R"
			label.position = Vector2(-5, -10)
			node.add_child(label)
			
			# Add connection points
			var left_point = ColorRect.new()
			left_point.size = Vector2(10, 10)
			left_point.position = Vector2(-25, -5)
			left_point.color = Color(1, 0.8, 0)  # Gold
			node.add_child(left_point)
			
			var right_point = ColorRect.new()
			right_point.size = Vector2(10, 10)
			right_point.position = Vector2(15, -5)
			right_point.color = Color(1, 0.8, 0)  # Gold
			node.add_child(right_point)
			
		"Capacitor":
			# Create capacitor symbol
			var rect = ColorRect.new()
			rect.size = Vector2(40, 20)
			rect.position = Vector2(-20, -10)
			rect.color = Color(0.2, 0.8, 0.2)  # Green
			node.add_child(rect)
			
			var label = Label.new()
			label.text = "C"
			label.position = Vector2(-5, -10)
			node.add_child(label)
			
			var left_point = ColorRect.new()
			left_point.size = Vector2(10, 10)
			left_point.position = Vector2(-25, -5)
			left_point.color = Color(1, 0.8, 0)
			node.add_child(left_point)
			
			var right_point = ColorRect.new()
			right_point.size = Vector2(10, 10)
			right_point.position = Vector2(15, -5)
			right_point.color = Color(1, 0.8, 0)
			node.add_child(right_point)
			
		"Inductor":
			# Create inductor symbol
			var rect = ColorRect.new()
			rect.size = Vector2(40, 20)
			rect.position = Vector2(-20, -10)
			rect.color = Color(0.8, 0.2, 0.8)  # Purple
			node.add_child(rect)
			
			var label = Label.new()
			label.text = "L"
			label.position = Vector2(-5, -10)
			node.add_child(label)
			
			var left_point = ColorRect.new()
			left_point.size = Vector2(10, 10)
			left_point.position = Vector2(-25, -5)
			left_point.color = Color(1, 0.8, 0)
			node.add_child(left_point)
			
			var right_point = ColorRect.new()
			right_point.size = Vector2(10, 10)
			right_point.position = Vector2(15, -5)
			right_point.color = Color(1, 0.8, 0)
			node.add_child(right_point)
			
		_:
			# Default representation for unknown components
			var rect = ColorRect.new()
			rect.size = Vector2(50, 50)
			rect.position = Vector2(-25, -25)
			rect.color = Color(0.7, 0.7, 0.7)  # Gray
			node.add_child(rect)
			
			var label = Label.new()
			label.text = component_name
			label.position = Vector2(-25, 30)
			node.add_child(label)
	
	return node

func _unhandled_input(event):
	# Camera pan
	if event is InputEventMouseButton:
		if event.button_index == MOUSE_BUTTON_MIDDLE:
			if event.pressed:
				camera_drag = true
				camera_drag_start = event.position
				camera_start_position = camera.position
			else:
				camera_drag = false
		# Zoom
		elif event.button_index == MOUSE_BUTTON_WHEEL_UP:
			_zoom_camera(1 + zoom_speed)
		elif event.button_index == MOUSE_BUTTON_WHEEL_DOWN:
			_zoom_camera(1 - zoom_speed)
	
	# Camera drag
	elif event is InputEventMouseMotion and camera_drag:
		camera.position = camera_start_position + (camera_drag_start - event.position) / camera.zoom.x

func _zoom_camera(factor):
	var new_zoom = camera.zoom * factor
	new_zoom = new_zoom.clamp(Vector2(min_zoom, min_zoom), Vector2(max_zoom, max_zoom))
	camera.zoom = new_zoom

func _on_simulate_pressed():
	status_label.text = "Simulating..."
	# TODO: Implement simulation

func _on_stop_pressed():
	status_label.text = "Stopped"
	# TODO: Implement simulation stop

func _snap_to_grid(pos: Vector2) -> Vector2:
	var snapped = Vector2()
	snapped.x = round(pos.x / GRID_SIZE) * GRID_SIZE
	snapped.y = round(pos.y / GRID_SIZE) * GRID_SIZE
	return snapped
