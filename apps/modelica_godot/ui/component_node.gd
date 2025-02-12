@tool
extends GraphNode

var component_type: String
var component_data: Dictionary = {}

func setup(type: String, data: Dictionary = {}) -> void:
	component_type = type
	component_data = data
	title = type
	
	# Set up node based on component type
	match type:
		"Mass":
			_setup_mass()
		"Spring":
			_setup_spring()
		"Fixed":
			_setup_fixed()

func _setup_mass() -> void:
	# Add mass parameter control
	var param_container = VBoxContainer.new()
	var mass_label = Label.new()
	mass_label.text = "Mass (kg):"
	var mass_spin = SpinBox.new()
	mass_spin.min_value = 0.1
	mass_spin.max_value = 100.0
	mass_spin.step = 0.1
	mass_spin.value = component_data.get("m", 1.0)
	mass_spin.value_changed.connect(_on_mass_changed)
	
	param_container.add_child(mass_label)
	param_container.add_child(mass_spin)
	add_child(param_container)
	
	# Add single port
	set_slot(0, true, 0, Color.BLUE, true, 0, Color.BLUE)

func _setup_spring() -> void:
	# Add spring constant parameter control
	var param_container = VBoxContainer.new()
	var k_label = Label.new()
	k_label.text = "Spring Constant (N/m):"
	var k_spin = SpinBox.new()
	k_spin.min_value = 0.1
	k_spin.max_value = 1000.0
	k_spin.step = 0.1
	k_spin.value = component_data.get("k", 1.0)
	k_spin.value_changed.connect(_on_k_changed)
	
	param_container.add_child(k_label)
	param_container.add_child(k_spin)
	add_child(param_container)
	
	# Add two ports (left and right)
	set_slot(0, true, 0, Color.BLUE, true, 0, Color.BLUE)

func _setup_fixed() -> void:
	# Add position parameter control
	var param_container = VBoxContainer.new()
	var pos_label = Label.new()
	pos_label.text = "Position (m):"
	var pos_spin = SpinBox.new()
	pos_spin.min_value = -100.0
	pos_spin.max_value = 100.0
	pos_spin.step = 0.1
	pos_spin.value = component_data.get("position", 0.0)
	pos_spin.value_changed.connect(_on_position_changed)
	
	param_container.add_child(pos_label)
	param_container.add_child(pos_spin)
	add_child(param_container)
	
	# Add single port
	set_slot(0, true, 0, Color.BLUE, true, 0, Color.BLUE)

func _on_mass_changed(value: float) -> void:
	component_data["m"] = value

func _on_k_changed(value: float) -> void:
	component_data["k"] = value

func _on_position_changed(value: float) -> void:
	component_data["position"] = value

func get_component_data() -> Dictionary:
	return {
		"type": component_type,
		"data": component_data
	}

func update_visualization() -> void:
	# Update visual representation based on component state
	# This will be overridden by specific component nodes
	pass 
