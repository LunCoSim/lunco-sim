class_name ResourcePropertyEditor
extends VBoxContainer

signal property_changed(property: String, value: Variant)

var current_resource: BaseResource

func edit_resource(resource: BaseResource) -> void:
	current_resource = resource
	refresh_properties()

func refresh_properties() -> void:
	# Clear existing properties
	for child in get_children():
		child.queue_free()
		
	if not current_resource:
		return
		
	# Add property fields
	_add_string_property("name", current_resource.name)
	_add_string_property("unit", current_resource.unit)
	_add_float_property("mass", current_resource.mass)
	_add_float_property("volume", current_resource.volume)
	_add_color_property("color", current_resource.color)

func _add_string_property(name: String, value: String) -> void:
	var container = HBoxContainer.new()
	var label = Label.new()
	label.text = name.capitalize()
	var line_edit = LineEdit.new()
	line_edit.text = value
	line_edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	line_edit.text_submitted.connect(
		func(new_text): property_changed.emit(name, new_text)
	)
	container.add_child(label)
	container.add_child(line_edit)
	add_child(container)

func _add_float_property(name: String, value: float) -> void:
	var container = HBoxContainer.new()
	var label = Label.new()
	label.text = name.capitalize()
	var spin_box = SpinBox.new()
	spin_box.min_value = 0.0
	spin_box.max_value = 9999.0
	spin_box.step = 0.1
	spin_box.value = value
	spin_box.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	spin_box.value_changed.connect(
		func(new_value): property_changed.emit(name, new_value)
	)
	container.add_child(label)
	container.add_child(spin_box)
	add_child(container)

func _add_color_property(name: String, value: Color) -> void:
	var container = HBoxContainer.new()
	var label = Label.new()
	label.text = name.capitalize()
	var color_picker = ColorPickerButton.new()
	color_picker.color = value
	color_picker.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	color_picker.color_changed.connect(
		func(new_color): property_changed.emit(name, new_color)
	)
	container.add_child(label)
	container.add_child(color_picker)
	add_child(container)
