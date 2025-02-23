@tool
extends Control

var equation_system: EquationSystem
var time: float = 0.0
var dt: float = 0.01  # Time step

@onready var k_spinbox = $HSplitContainer/RightPanel/ParametersPanel/VBoxContainer/SpringParams/KSpinBox
@onready var length_spinbox = $HSplitContainer/RightPanel/ParametersPanel/VBoxContainer/SpringParams/LengthSpinBox
@onready var simulation_world = $HSplitContainer/ViewportContainer/SubViewport/SimulationWorld

var spring_visualization = null
var spring_component_name: String = "Spring"

func _ready() -> void:
	equation_system = EquationSystem.new()
	add_child(equation_system)
	
	k_spinbox.value_changed.connect(_on_k_value_changed)
	length_spinbox.value_changed.connect(_on_length_value_changed)
	
	# Create spring visualization
	spring_visualization = preload("res://apps/modelica_godot/ui/components/spring_visualization.tscn").instantiate()
	simulation_world.add_child(spring_visualization)

func _on_k_value_changed(value: float) -> void:
	if equation_system and equation_system.has_component(spring_component_name):
		equation_system.set_component_parameter(spring_component_name, "k", value)

func _on_length_value_changed(value: float) -> void:
	if equation_system and equation_system.has_component(spring_component_name):
		equation_system.set_component_parameter(spring_component_name, "l0", value)

func set_equation_system(system: EquationSystem) -> void:
	if equation_system:
		equation_system.queue_free()
	equation_system = system
	add_child(equation_system)
	if system and system.has_component(spring_component_name):
		# Initialize UI with system values
		k_spinbox.value = system.get_component_parameter(spring_component_name, "k")
		length_spinbox.value = system.get_component_parameter(spring_component_name, "l0")

func _process(_delta: float) -> void:
	if equation_system and equation_system.has_component(spring_component_name):
		# Update visualization based on equation system state
		var length = equation_system.variables.get(spring_component_name + ".length", 1.0)
		var force = equation_system.variables.get(spring_component_name + ".force", 0.0)
		spring_visualization.set_current_length(length)
		spring_visualization.set_force(force)

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
	if equation_system:
		equation_system.queue_free()
	equation_system = EquationSystem.new()
	add_child(equation_system)

func _to_string() -> String:
	var result = "SimulationView:\n"
	result += "  Time: %f\n" % time
	result += "  Equation System:\n"
	result += equation_system._to_string()
	return result 
