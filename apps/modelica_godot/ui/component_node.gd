@tool
extends GraphNode

var component_type: String
var component_data: Dictionary = {}
var parameter_controls: Dictionary = {}

# Simulation variables
var current_position := 0.0
var current_velocity := 0.0
var current_acceleration := 0.0
var simulation_panel: Node = null

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
		"Damper":
			_setup_damper()
		"Fixed":
			_setup_fixed()
		"SpringMassDamper":
			_setup_spring_mass_damper()
			_setup_simulation()

func _create_parameter_section(title: String) -> VBoxContainer:
	var section = VBoxContainer.new()
	var header = Label.new()
	header.text = title
	header.add_theme_color_override("font_color", Color(0.7, 0.7, 1.0))
	section.add_child(header)
	var separator = HSeparator.new()
	section.add_child(separator)
	return section

func _add_parameter_control(container: Node, param_name: String, label: String, default_value: float, min_val: float, max_val: float, step: float) -> void:
	var param_container = HBoxContainer.new()
	var param_label = Label.new()
	param_label.text = label
	param_label.custom_minimum_size.x = 120
	
	var param_spin = SpinBox.new()
	param_spin.min_value = min_val
	param_spin.max_value = max_val
	param_spin.step = step
	param_spin.value = component_data.get(param_name, default_value)
	param_spin.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	param_spin.value_changed.connect(_on_parameter_changed.bind(param_name))
	
	parameter_controls[param_name] = param_spin
	
	param_container.add_child(param_label)
	param_container.add_child(param_spin)
	container.add_child(param_container)

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

func _setup_damper() -> void:
	# Add damping coefficient parameter control
	var param_container = VBoxContainer.new()
	var d_label = Label.new()
	d_label.text = "Damping (N.s/m):"
	var d_spin = SpinBox.new()
	d_spin.min_value = 0.0
	d_spin.max_value = 100.0
	d_spin.step = 0.1
	d_spin.value = component_data.get("d", 0.5)
	d_spin.value_changed.connect(_on_damper_changed)
	
	param_container.add_child(d_label)
	param_container.add_child(d_spin)
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

func _setup_spring_mass_damper() -> void:
	# Parameters section
	var params = _create_parameter_section("System Parameters")
	
	# Mass parameters
	_add_parameter_control(params, "mass", "Mass (kg):", 1.0, 0.1, 100.0, 0.1)
	
	# Spring parameters
	_add_parameter_control(params, "k", "Spring (N/m):", 10.0, 0.1, 1000.0, 0.1)
	
	# Damper parameters
	_add_parameter_control(params, "d", "Damping (N.s/m):", 0.5, 0.0, 100.0, 0.1)
	
	add_child(params)
	
	# Initial conditions section
	var init_conditions = _create_parameter_section("Initial Conditions")
	_add_parameter_control(init_conditions, "x0", "Position (m):", 0.5, -10.0, 10.0, 0.1)
	_add_parameter_control(init_conditions, "v0", "Velocity (m/s):", 0.0, -10.0, 10.0, 0.1)
	
	add_child(init_conditions)
	
	# Add ports
	set_slot(0, true, 0, Color.BLUE, true, 0, Color.BLUE)

func _setup_simulation() -> void:
	# Create and add simulation panel
	var simulation_scene := load("res://apps/modelica_godot/ui/simulation_panel.tscn")
	simulation_panel = simulation_scene.instantiate()
	add_child(simulation_panel)
	
	# Connect signals
	simulation_panel.simulation_started.connect(_on_simulation_started)
	simulation_panel.simulation_paused.connect(_on_simulation_paused)
	simulation_panel.simulation_reset.connect(_on_simulation_reset)
	simulation_panel.simulation_step.connect(_on_simulation_step)

func _on_simulation_started() -> void:
	# Initialize simulation if needed
	if not validate_parameters():
		simulation_panel.is_simulating = false
		return

func _on_simulation_paused() -> void:
	pass

func _on_simulation_reset() -> void:
	current_position = component_data.get("x0", 0.5)
	current_velocity = component_data.get("v0", 0.0)
	current_acceleration = 0.0
	
	if simulation_panel:
		simulation_panel.update_state(current_position, current_velocity, current_acceleration)

func _on_simulation_step(delta: float) -> void:
	if not validate_parameters():
		return
	
	# Get parameters with explicit types
	var mass: float = component_data.get("mass", 1.0)
	var k: float = component_data.get("k", 10.0)
	var d: float = component_data.get("d", 0.5)
	
	# Calculate forces with explicit types
	var spring_force: float = -k * current_position
	var damping_force: float = -d * current_velocity
	var total_force: float = spring_force + damping_force
	
	# Calculate acceleration
	current_acceleration = total_force / mass
	
	# Update velocity and position using semi-implicit Euler integration
	current_velocity += current_acceleration * delta
	current_position += current_velocity * delta
	
	# Update visualization
	if simulation_panel:
		simulation_panel.update_state(current_position, current_velocity, current_acceleration)

func _on_mass_changed(value: float) -> void:
	component_data["m"] = value

func _on_k_changed(value: float) -> void:
	component_data["k"] = value

func _on_position_changed(value: float) -> void:
	component_data["position"] = value

func _on_damper_changed(value: float) -> void:
	component_data["d"] = value
	update_visualization()

func _on_parameter_changed(value: float, param_name: String) -> void:
	component_data[param_name] = value
	update_visualization()

func validate_parameters() -> bool:
	match component_type:
		"SpringMassDamper":
			if component_data.get("mass", 1.0) <= 0:
				printerr("Mass must be positive")
				return false
			if component_data.get("k", 10.0) <= 0:
				printerr("Spring constant must be positive")
				return false
			if component_data.get("d", 0.5) < 0:
				printerr("Damping coefficient must be non-negative")
				return false
	return true

func update_visualization() -> void:
	if not validate_parameters():
		modulate = Color(1.0, 0.8, 0.8)  # Reddish tint for invalid parameters
	else:
		modulate = Color(1.0, 1.0, 1.0)  # Normal color for valid parameters
	
	if simulation_panel:
		simulation_panel.update_state(current_position, current_velocity, current_acceleration)
	queue_redraw()

func get_component_data() -> Dictionary:
	return {
		"type": component_type,
		"data": component_data
	}
