class_name Connection
extends Node2D

var start_component: Node2D
var end_component: Node2D
var start_point: String  # "left" or "right"
var end_point: String    # "left" or "right"
var line: Line2D
var click_area: Control
var is_hovered: bool = false
var is_selected: bool = false

signal connection_clicked
signal connection_selected
signal connection_deleted

const NORMAL_COLOR = Color(0.7, 0.7, 0.7)  # Gray
const HOVER_COLOR = Color(1.0, 0.8, 0.0)   # Gold
const SELECTED_COLOR = Color(0.0, 1.0, 0.0) # Green
const LINE_WIDTH = 2.0
const CLICK_AREA_PADDING = 5.0

func _init(from_component: Node2D, to_component: Node2D, from_point: String, to_point: String):
	start_component = from_component
	end_component = to_component
	start_point = from_point
	end_point = to_point
	
	# Create visual line
	line = Line2D.new()
	line.width = LINE_WIDTH
	line.default_color = NORMAL_COLOR
	add_child(line)
	
	# Create clickable area
	click_area = Control.new()
	click_area.mouse_filter = Control.MOUSE_FILTER_STOP
	click_area.mouse_entered.connect(_on_mouse_entered)
	click_area.mouse_exited.connect(_on_mouse_exited)
	click_area.gui_input.connect(_on_gui_input)
	add_child(click_area)
	
	# Initial update of line position
	_update_line_position()

func _process(_delta):
	if not is_instance_valid(start_component) or not is_instance_valid(end_component):
		queue_free()
		return
		
	_update_line_position()
	_update_line_color()

func _update_line_position():
	var start_pos = _get_connection_point_position(start_component, start_point)
	var end_pos = _get_connection_point_position(end_component, end_point)
	
	line.clear_points()
	line.add_point(start_pos)
	line.add_point(end_pos)
	
	# Update clickable area with padding for easier selection
	var padding = 10
	var min_x = min(start_pos.x, end_pos.x) - padding
	var min_y = min(start_pos.y, end_pos.y) - padding
	var max_x = max(start_pos.x, end_pos.x) + padding
	var max_y = max(start_pos.y, end_pos.y) + padding
	
	click_area.position = Vector2(min_x, min_y)
	click_area.size = Vector2(max_x - min_x, max_y - min_y)

func _get_connection_point_position(component: Node2D, point: String) -> Vector2:
	var offset = Vector2.ZERO
	match point:
		"left":
			offset = Vector2(-20, 0)
		"right":
			offset = Vector2(20, 0)
	return component.global_position + offset

func _on_mouse_entered():
	is_hovered = true

func _on_mouse_exited():
	is_hovered = false

func _on_gui_input(event: InputEvent):
	if event is InputEventMouseButton and event.pressed:
		match event.button_index:
			MOUSE_BUTTON_LEFT:
				is_selected = !is_selected
				connection_selected.emit()
			MOUSE_BUTTON_RIGHT:
				connection_deleted.emit()
			MOUSE_BUTTON_MIDDLE:
				connection_clicked.emit()

# Validation methods
static func can_connect(start_comp: Node2D, end_comp: Node2D, start_p: String, end_p: String) -> bool:
	# Don't connect a component to itself
	if start_comp == end_comp:
		return false
		
	# Don't connect if components are already connected
	for child in start_comp.get_parent().get_children():
		if child is Connection:
			var conn = child as Connection
			if (conn.start_component == start_comp and conn.end_component == end_comp) or \
			   (conn.start_component == end_comp and conn.end_component == start_comp):
				return false
	
	# Don't connect same sides (left-left or right-right)
	if start_p == end_p:
		return false
		
	# Get component types (assuming they're stored in the name for now)
	var start_type = start_comp.name
	var end_type = end_comp.name
	
	# Validate based on component types
	match start_type:
		"VoltageSource":
			return end_type in ["Resistor", "Capacitor", "Inductor"]
		"Resistor":
			return end_type in ["VoltageSource", "Resistor", "Capacitor", "Inductor"]
		_:
			return true  # Allow other connections for now

func get_connection_data() -> Dictionary:
	return {
		"start_component": start_component.name,
		"end_component": end_component.name,
		"start_point": start_point,
		"end_point": end_point
	}

func delete():
	connection_deleted.emit()
	queue_free() 

func _update_line_color():
	if is_selected:
		line.default_color = SELECTED_COLOR
	elif is_hovered:
		line.default_color = HOVER_COLOR
	else:
		line.default_color = NORMAL_COLOR
