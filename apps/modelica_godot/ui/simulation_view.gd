@tool
extends Control

var equation_system: EquationSystem
var time: float = 0.0
var dt: float = 0.01  # Time step

@onready var k_spinbox = $HSplitContainer/RightPanel/ParametersPanel/VBoxContainer/SpringParams/KSpinBox
@onready var length_spinbox = $HSplitContainer/RightPanel/ParametersPanel/VBoxContainer/SpringParams/LengthSpinBox
@onready var simulation_world = $HSplitContainer/ViewportContainer/SubViewport/SimulationWorld

var spring_visualization = null

func _ready() -> void:
	equation_system = EquationSystem.new()
	
	k_spinbox.value_changed.connect(_on_k_value_changed)
	length_spinbox.value_changed.connect(_on_length_value_changed)
	
	# Create spring visualization
	spring_visualization = preload("res://apps/modelica_godot/ui/components/spring_visualization.tscn").instantiate()
	simulation_world.add_child(spring_visualization)

func _on_k_value_changed(value: float):
	if equation_system:
		# Update spring constant in the equation system
		for component in equation_system.components:
			if "Spring" in component.name:
				component.set_parameter("k", value)
				break

func _on_length_value_changed(value: float):
	if equation_system:
		# Update rest length in the equation system
		for component in equation_system.components:
			if "Spring" in component.name:
				component.set_parameter("l0", value)
				break

func set_equation_system(system: EquationSystem):
	equation_system = system
	if system:
		# Initialize UI with system values
		for component in system.components:
			if "Spring" in component.name:
				k_spinbox.value = component.get_parameter("k")
				length_spinbox.value = component.get_parameter("l0")
				break

func _process(_delta):
	if equation_system:
		# Update visualization based on equation system state
		for component in equation_system.components:
			if "Spring" in component.name:
				var length = equation_system.variables.get(component.name + ".length", 1.0)
				var force = equation_system.variables.get(component.name + ".force", 0.0)
				spring_visualization.set_current_length(length)
				spring_visualization.set_force(force)
				break

func get_simulation_world() -> Node2D:
	return simulation_world 

func simulate(duration: float) -> void:
	var steps = int(duration / dt)
	for i in range(steps):
		time += dt
		equation_system.solve()

func _on_step_button_pressed() -> void:
	simulate(dt)

func _on_run_button_pressed() -> void:
	simulate(1.0)  # Simulate for 1 second

func _on_reset_button_pressed() -> void:
	time = 0.0
	equation_system = EquationSystem.new()

func _to_string() -> String:
	var result = "SimulationView:\n"
	result += "  Time: %f\n" % time
	result += "  Equation System:\n"
	result += equation_system._to_string()
	return result 