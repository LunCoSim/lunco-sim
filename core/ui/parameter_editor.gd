class_name LCParameterEditor
extends VBoxContainer

## Universal editor for LCSpaceSystem parameters.
## Scans a target node hierarchy for components with "Parameters" metadata
## and generates a UI to edit them in real-time.

@export var target_path: NodePath
var target_node: Node

var content_container: VBoxContainer

func _init():
	# This IS the content container
	content_container = self
	size_flags_horizontal = Control.SIZE_EXPAND_FILL

func _ready():
	if not target_path.is_empty():
		set_target(get_node(target_path))

func set_target(node: Node):
	print("DEBUG: ParameterEditor set_target: ", node)
	target_node = node
	refresh()

func refresh():
	# Clear existing
	for child in content_container.get_children():
		child.queue_free()
	
	if not target_node:
		print("DEBUG: ParameterEditor refresh - no target node")
		return
	
	# Scan hierarchy
	print("DEBUG: ParameterEditor scanning hierarchy of: ", target_node.name)
	_scan_and_build(target_node)

func _scan_and_build(node: Node):
	# Check if node has Parameters dictionary (duck typing)
	if "Parameters" in node and node.Parameters is Dictionary and not node.Parameters.is_empty():
		print("DEBUG: Found component with parameters: ", node.name)
		_create_component_section(node)
	
	# Recurse
	for child in node.get_children():
		_scan_and_build(child)

func _create_component_section(component: Object):
	# Header
	var header = Label.new()
	header.text = component.name
	header.add_theme_font_size_override("font_size", 16)
	header.add_theme_color_override("font_color", Color(0.6, 0.8, 1.0))
	content_container.add_child(header)
	
	var section = VBoxContainer.new()
	content_container.add_child(section)
	
	# Add separator
	var sep = HSeparator.new()
	section.add_child(sep)
	
	# Create controls for each parameter
	for param_key in component.Parameters:
		var metadata = component.Parameters[param_key]
		_create_parameter_control(section, component, param_key, metadata)
	
	# Spacer
	var spacer = Control.new()
	spacer.custom_minimum_size.y = 10
	content_container.add_child(spacer)

func _create_parameter_control(parent: Control, component: Object, param_key: String, metadata: Dictionary):
	var row = HBoxContainer.new()
	parent.add_child(row)
	
	var label = Label.new()
	label.text = param_key.capitalize()
	label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	row.add_child(label)
	
	var property_path = metadata.get("path", "")
	if property_path.is_empty():
		return # Invalid metadata
		
	var current_value = component.get(property_path)
	var type = metadata.get("type", "float")
	var is_readonly = metadata.get("readonly", false)
	var use_text_field = metadata.get("text_field", false)
	
	if type == "float" or type == "int":
		if is_readonly:
			# Read-only: just show the value, update it every frame
			var value_label = Label.new()
			value_label.text = _format_value(current_value, type)
			value_label.custom_minimum_size.x = 100
			value_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
			row.add_child(value_label)
			
			# Store reference for real-time updates
			value_label.set_meta("component", component)
			value_label.set_meta("property_path", property_path)
			value_label.set_meta("type", type)
		elif use_text_field:
			# Text field for direct input
			var line_edit = LineEdit.new()
			line_edit.text = _format_value(current_value, type)
			line_edit.custom_minimum_size.x = 120
			line_edit.alignment = HORIZONTAL_ALIGNMENT_RIGHT
			
			# Store references for real-time updates
			line_edit.set_meta("component", component)
			line_edit.set_meta("property_path", property_path)
			line_edit.set_meta("type", type)
			
			line_edit.text_submitted.connect(func(text):
				var val = float(text.replace(",", ""))
				component.set(property_path, val)
				line_edit.text = _format_value(val, type)
			)
			
			line_edit.focus_exited.connect(func():
				var val = float(line_edit.text.replace(",", ""))
				component.set(property_path, val)
				line_edit.text = _format_value(val, type)
			)
			
			row.add_child(line_edit)
		else:
			# Editable: slider + value label
			var slider = HSlider.new()
			slider.size_flags_horizontal = Control.SIZE_EXPAND_FILL
			slider.min_value = metadata.get("min", 0.0)
			slider.max_value = metadata.get("max", 100.0)
			slider.step = metadata.get("step", 0.1 if type == "float" else 1.0)
			slider.value = current_value
			
			var value_label = Label.new()
			value_label.text = _format_value(current_value, type)
			value_label.custom_minimum_size.x = 70
			value_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
			
			# Store references for real-time updates
			slider.set_meta("component", component)
			slider.set_meta("property_path", property_path)
			slider.set_meta("value_label", value_label)
			slider.set_meta("type", type)
			
			slider.value_changed.connect(func(val):
				component.set(property_path, val)
				value_label.text = _format_value(val, type)
				# Optional: Trigger update if component needs it
				if component.has_method("_update_parameters"):
					component._update_parameters()
			)
			
			row.add_child(slider)
			row.add_child(value_label)
		
	elif type == "bool":
		var checkbox = CheckBox.new()
		checkbox.button_pressed = current_value
		checkbox.toggled.connect(func(val):
			component.set(property_path, val)
		)
		row.add_child(checkbox)

func _format_value(value, type: String) -> String:
	if type == "int":
		return str(int(value))
	elif type == "float":
		return "%.2f" % value
	return str(value)

func _process(_delta):
	# Real-time update of parameter displays (only when not focused)
	if not target_node:
		return
	
	for child in content_container.get_children():
		_update_controls_recursive(child)

func _update_controls_recursive(node: Node):
	# Update readonly labels
	if node is Label and node.has_meta("component"):
		var component = node.get_meta("component")
		var property_path = node.get_meta("property_path")
		var type = node.get_meta("type")
		if component and is_instance_valid(component):
			var current_value = component.get(property_path)
			node.text = _format_value(current_value, type)
	
	# Update text fields (only if not focused)
	elif node is LineEdit and node.has_meta("component"):
		if not node.has_focus():
			var component = node.get_meta("component")
			var property_path = node.get_meta("property_path")
			var type = node.get_meta("type")
			if component and is_instance_valid(component):
				var current_value = component.get(property_path)
				node.text = _format_value(current_value, type)
	
	# Update sliders (only if not being dragged)
	elif node is HSlider and node.has_meta("component"):
		# Only update if slider doesn't have focus (user not interacting)
		if not node.has_focus():
			var component = node.get_meta("component")
			var property_path = node.get_meta("property_path")
			var value_label = node.get_meta("value_label")
			var type = node.get_meta("type")
			if component and is_instance_valid(component):
				var current_value = component.get(property_path)
				node.value = current_value
				if value_label:
					value_label.text = _format_value(current_value, type)
	
	# Recurse through children
	for child in node.get_children():
		_update_controls_recursive(child)
