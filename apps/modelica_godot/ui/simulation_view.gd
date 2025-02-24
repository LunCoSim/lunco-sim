@tool
extends Control

var dae_system: DAESystem
var solver: DAESolver
var time: float = 0.0
var dt: float = 0.01  # Time step

@onready var plot_container: Control = $PlotContainer
@onready var parameter_container: Control = $ParameterContainer
@onready var k_spinbox = $HSplitContainer/RightPanel/ParametersPanel/VBoxContainer/SpringParams/KSpinBox
@onready var length_spinbox = $HSplitContainer/RightPanel/ParametersPanel/VBoxContainer/SpringParams/LengthSpinBox
@onready var simulation_world = $HSplitContainer/ViewportContainer/SubViewport/SimulationWorld

var spring_visualization = null
var spring_component_name: String = "Spring"

func _ready() -> void:
	initialize()

func initialize() -> void:
	reset()

func reset() -> void:
	time = 0.0
	dae_system = null
	solver = null
	_clear_plots()
	_clear_parameters()

func set_system(system: DAESystem, p_solver: DAESolver) -> void:
	dae_system = system
	solver = p_solver
	
	# Create parameter controls
	_create_parameter_controls()
	
	# Create plots for state variables
	_create_plots()

func update_simulation() -> void:
	if not (dae_system and solver):
		return
	
	# Solve one step
	if solver.solve_continuous(0.01):
		time += 0.01
		_update_plots()
	else:
		push_error("Simulation step failed")

func _create_parameter_controls() -> void:
	_clear_parameters()
	
	for var_name in dae_system.variables:
		var var_obj = dae_system.variables[var_name]
		if var_obj.type == DAESystem.VariableType.PARAMETER:
			var container = HBoxContainer.new()
			
			var label = Label.new()
			label.text = var_name
			container.add_child(label)
			
			var slider = HSlider.new()
			slider.min_value = var_obj.min_value if var_obj.min_value > -INF else -100
			slider.max_value = var_obj.max_value if var_obj.max_value < INF else 100
			slider.value = var_obj.value
			slider.size_flags_horizontal = Control.SIZE_EXPAND_FILL
			slider.value_changed.connect(
				func(value): _on_parameter_changed(var_name, value)
			)
			container.add_child(slider)
			
			parameter_container.add_child(container)

func _create_plots() -> void:
	_clear_plots()
	
	for var_name in dae_system.variables:
		var var_obj = dae_system.variables[var_name]
		if var_obj.type == DAESystem.VariableType.STATE:
			var plot = _create_plot(var_name)
			plot_container.add_child(plot)

func _create_plot(var_name: String) -> Control:
	var container = VBoxContainer.new()
	
	var label = Label.new()
	label.text = var_name
	container.add_child(label)
	
	var plot = Line2D.new()
	plot.name = var_name + "_plot"
	plot.default_color = Color.WHITE
	plot.width = 2.0
	container.add_child(plot)
	
	return container

func _update_plots() -> void:
	for var_name in dae_system.variables:
		var var_obj = dae_system.variables[var_name]
		if var_obj.type == DAESystem.VariableType.STATE:
			var plot = plot_container.get_node_or_null(var_name + "_plot")
			if plot:
				plot.add_point(Vector2(time, var_obj.value))
				# Keep only last 1000 points
				if plot.points.size() > 1000:
					plot.remove_point(0)

func _clear_plots() -> void:
	for child in plot_container.get_children():
		child.queue_free()

func _clear_parameters() -> void:
	for child in parameter_container.get_children():
		child.queue_free()

func _on_parameter_changed(var_name: String, value: float) -> void:
	if dae_system and dae_system.variables.has(var_name):
		var var_obj = dae_system.variables[var_name]
		var_obj.value = value
		# Reinitialize system after parameter change
		solver.solve_initialization()

func _on_k_value_changed(value: float) -> void:
	if dae_system and dae_system.has_component(spring_component_name):
		dae_system.set_component_parameter(spring_component_name, "k", value)

func _on_length_value_changed(value: float) -> void:
	if dae_system and dae_system.has_component(spring_component_name):
		dae_system.set_component_parameter(spring_component_name, "l0", value)

func get_simulation_world() -> Node2D:
	return simulation_world 

func simulate(duration: float) -> void:
	var steps = int(duration / dt)
	for i in range(steps):
		time += dt
		dae_system.solve()

func _on_step_button_pressed() -> void:
	simulate(dt)

func _on_run_button_pressed() -> void:
	simulate(1.0)  # Simulate for 1 second

func _on_reset_button_pressed() -> void:
	time = 0.0
	if dae_system:
		dae_system.queue_free()
	dae_system = DAESystem.new()
	add_child(dae_system)

func _to_string() -> String:
	var result = "SimulationView:\n"
	result += "  Time: %f\n" % time
	result += "  Equation System:\n"
	result += dae_system._to_string()
	return result 
