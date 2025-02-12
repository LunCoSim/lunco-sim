class_name ModelManager
extends Node

var components: Array[ModelicaComponent] = []
var equation_system: EquationSystem
var time: float = 0.0
var dt: float = 0.01  # Time step

func _init():
	equation_system = EquationSystem.new()
	add_child(equation_system)

func add_component(component: ModelicaComponent) -> void:
	components.append(component)
	add_child(component)
	
	# Add component equations to system
	for eq in component.get_equations():
		equation_system.add_equation(eq, component)

func get_component(node_name: StringName) -> ModelicaComponent:
	# Convert StringName to NodePath
	var node_path = NodePath(node_name)
	# Find component by name
	for component in components:
		if component.name == node_name:
			return component
	return null

func connect_components(from_component: ModelicaComponent, from_port: String, 
						to_component: ModelicaComponent, to_port: String) -> bool:
	# Verify connection compatibility
	var from_connector = from_component.get_connector(from_port)
	var to_connector = to_component.get_connector(to_port)
	
	if from_connector.type != to_connector.type:
		push_error("Cannot connect different connector types")
		return false
	
	# Add connection equations
	for var_name in from_connector.variables.keys():
		if _is_across_variable(var_name):
			# Across variables are equal
			equation_system.add_equation(
				"%s.%s.%s = %s.%s.%s" % [
					from_component.name, from_port, var_name,
					to_component.name, to_port, var_name
				],
				null
			)
		else:
			# Through variables sum to zero
			equation_system.add_equation(
				"%s.%s.%s + %s.%s.%s = 0" % [
					from_component.name, from_port, var_name,
					to_component.name, to_port, var_name
				],
				null
			)
	return true

func disconnect_components(from_component: ModelicaComponent, from_port: String,
						 to_component: ModelicaComponent, to_port: String) -> bool:
	# TODO: Remove connection equations
	return true

func _is_across_variable(var_name: String) -> bool:
	return var_name in ["position", "voltage", "temperature", "pressure"]

func simulate(duration: float) -> void:
	var steps = int(duration / dt)
	for i in range(steps):
		time += dt
		equation_system.solve() 
