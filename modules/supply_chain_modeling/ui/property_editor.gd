extends VBoxContainer

var current_node: Node = null

func clear_properties():
	for child in get_children():
		child.queue_free()

func update_properties(node: Node):
	clear_properties()
	current_node = node
	
	if not node:
		return
		
	# Add node name field
	add_string_property("name", "Node Name", node.name)
	
	# Get list of properties
	var property_list = node.get_property_list()
	
	# Filter and display editable properties
	for property in property_list:
		# Skip built-in properties we don't want to edit
		if property["usage"] & PROPERTY_USAGE_SCRIPT_VARIABLE == 0:
			continue
			
		match property["type"]:
			TYPE_FLOAT:
				add_float_property(property["name"], property["name"], node.get(property["name"]))
			TYPE_INT:
				add_int_property(property["name"], property["name"], node.get(property["name"]))
			TYPE_STRING:
				add_string_property(property["name"], property["name"], node.get(property["name"]))
			TYPE_BOOL:
				add_bool_property(property["name"], property["name"], node.get(property["name"]))

func add_float_property(property_name: String, display_name: String, value: float):
	var container = HBoxContainer.new()
	
	var label = Label.new()
	label.text = display_name
	container.add_child(label)
	
	var spinbox = SpinBox.new()
	spinbox.min_value = -999999
	spinbox.max_value = 999999
	spinbox.step = 0.1
	spinbox.value = value
	spinbox.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	spinbox.connect("value_changed", _on_property_changed.bind(property_name))
	container.add_child(spinbox)
	
	add_child(container)

func add_int_property(property_name: String, display_name: String, value: int):
	var container = HBoxContainer.new()
	
	var label = Label.new()
	label.text = display_name
	container.add_child(label)
	
	var spinbox = SpinBox.new()
	spinbox.min_value = -999999
	spinbox.max_value = 999999
	spinbox.step = 1
	spinbox.value = value
	spinbox.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	spinbox.connect("value_changed", _on_property_changed.bind(property_name))
	container.add_child(spinbox)
	
	add_child(container)

func add_string_property(property_name: String, display_name: String, value: String):
	var container = HBoxContainer.new()
	
	var label = Label.new()
	label.text = display_name
	container.add_child(label)
	
	var line_edit = LineEdit.new()
	line_edit.text = value
	line_edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	line_edit.connect("text_changed", _on_property_changed.bind(property_name))
	container.add_child(line_edit)
	
	add_child(container)

func add_bool_property(property_name: String, display_name: String, value: bool):
	var container = HBoxContainer.new()
	
	var label = Label.new()
	label.text = display_name
	container.add_child(label)
	
	var checkbox = CheckBox.new()
	checkbox.button_pressed = value
	checkbox.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	checkbox.connect("toggled", _on_property_changed.bind(property_name))
	container.add_child(checkbox)
	
	add_child(container)

func _on_property_changed(value, property_name: String):
	if current_node:
		current_node.set(property_name, value) 
