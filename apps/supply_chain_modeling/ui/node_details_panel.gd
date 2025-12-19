class_name NodeDetailsPanel
extends PanelContainer

@onready var label_title = $MarginContainer/VBoxContainer/Header/LabelTitle
@onready var label_id = $MarginContainer/VBoxContainer/GridContainer/LabelIDValue
@onready var label_domain = $MarginContainer/VBoxContainer/GridContainer/LabelDomainValue
@onready var label_potential = $MarginContainer/VBoxContainer/GridContainer/LabelPotentialValue
@onready var label_flow = $MarginContainer/VBoxContainer/GridContainer/LabelFlowValue
@onready var label_resource = $MarginContainer/VBoxContainer/GridContainer/LabelResourceValue
@onready var label_capacitance = $MarginContainer/VBoxContainer/GridContainer/LabelCapacitanceValue
@onready var label_capacitance_label = $MarginContainer/VBoxContainer/GridContainer/LabelCapacitance
@onready var parameters_container = $MarginContainer/VBoxContainer/ParametersContainer

var current_node: LCSolverNode
var parameter_controls: Dictionary = {}  # param_name -> control widget

func _ready():
	hide()

func _process(_delta):
	if visible and current_node:
		_update_values()

func display_node(node: LCSolverNode):
	print("NodeDetailsPanel: display_node called for: ", node.display_name if node else "null")
	current_node = node
	show()
	print("NodeDetailsPanel: Panel shown. Visible=%s, Position=%s, Size=%s" % [visible, position, size])
	_update_static_info()
	_update_values()
	_build_parameter_controls()

func _update_static_info():
	if not current_node: return
	label_id.text = str(current_node.id)
	label_domain.text = str(current_node.domain)
	
	if current_node.is_storage:
		label_capacitance.text = "%.2f" % current_node.capacitance
		label_capacitance.show()
		label_capacitance_label.show()
	else:
		label_capacitance.hide()
		label_capacitance_label.hide()

func _update_values():
	if not current_node: return
	label_potential.text = "%.4f" % current_node.potential
	label_flow.text = "%.4f" % current_node.flow_accumulation
	label_resource.text = current_node.resource_type if current_node.resource_type else "None"

func _build_parameter_controls():
	# Clear existing controls
	for child in parameters_container.get_children():
		child.queue_free()
	parameter_controls.clear()
	
	if not current_node:
		print("NodeDetailsPanel: No current_node")
		parameters_container.hide()
		return
		
	if not current_node.effector_ref:
		print("NodeDetailsPanel: Node '%s' has no effector_ref" % current_node.display_name)
		parameters_container.hide()
		return
	
	var effector = current_node.effector_ref.get_ref()
	if not effector:
		print("NodeDetailsPanel: effector_ref is dead/null for node '%s'" % current_node.display_name)
		parameters_container.hide()
		return
		
	if not "Parameters" in effector:
		print("NodeDetailsPanel: Effector '%s' has no Parameters dictionary" % effector.name)
		parameters_container.hide()
		return
	
	print("NodeDetailsPanel: Building controls for '%s' with %d parameters" % [effector.name, effector.Parameters.size()])
	
	# Show parameters section
	parameters_container.show()
	
	# Add title
	var title = Label.new()
	title.text = "Parameters"
	title.add_theme_font_size_override("font_size", 14)
	parameters_container.add_child(title)
	
	var separator = HSeparator.new()
	parameters_container.add_child(separator)
	
	# Create controls for each parameter
	for param_name in effector.Parameters:
		var param_def = effector.Parameters[param_name]
		
		# Skip read-only parameters
		if param_def.get("readonly", false):
			continue
		
		var row = HBoxContainer.new()
		parameters_container.add_child(row)
		
		var label = Label.new()
		label.text = param_name + ":"
		label.custom_minimum_size.x = 100
		row.add_child(label)
		
		# Create appropriate control based on type
		var control = null
		match param_def.get("type", "float"):
			"float":
				var slider = HSlider.new()
				slider.min_value = param_def.get("min", 0.0)
				slider.max_value = param_def.get("max", 1.0)
				slider.step = param_def.get("step", 0.01)
				slider.size_flags_horizontal = Control.SIZE_EXPAND_FILL
				slider.value = effector.get(param_def.path)
				slider.value_changed.connect(_on_parameter_changed.bind(effector, param_def.path))
				control = slider
				
				# Add value label
				var value_label = Label.new()
				value_label.text = "%.2f" % slider.value
				value_label.custom_minimum_size.x = 50
				slider.value_changed.connect(func(val): value_label.text = "%.2f" % val)
				row.add_child(slider)
				row.add_child(value_label)
				
			"bool":
				var checkbox = CheckBox.new()
				checkbox.button_pressed = effector.get(param_def.path)
				checkbox.toggled.connect(_on_parameter_changed.bind(effector, param_def.path))
				control = checkbox
				row.add_child(checkbox)
		
		if control:
			parameter_controls[param_name] = control
	
	print("NodeDetailsPanel: Created %d parameter controls (skipped read-only)" % parameter_controls.size())

func _on_parameter_changed(value, effector, param_path):
	if effector and effector.has_method("set"):
		effector.set(param_path, value)
		print("NodeDetailsPanel: Set %s.%s = %s" % [effector.name, param_path, value])

func _on_close_button_pressed():
	hide()
	current_node = null
