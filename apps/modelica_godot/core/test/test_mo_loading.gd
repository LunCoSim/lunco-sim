@tool
extends SceneTree

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")
var parser: MOParser

func _init() -> void:
	print("Starting Modelica file loading tests")
	parser = MOParser.new()
	get_root().add_child(parser)
	test_damping_mass()
	quit()

func test_damping_mass() -> void:
	print("\nTesting DampingMassTest.mo loading")
	var result = parser.parse_file("res://apps/modelica_godot/components/Mechanical/DampingMassTest.mo")
	
	# Test basic model structure
	assert_eq(result.type, "model", "Model type")
	assert_eq(result.name, "DampingMassTest", "Model name")
	assert_eq(result.description, "Test model for a mass with damping", "Model description")
	assert_eq(result.within, "ModelicaGodot.Mechanical", "Within clause")
	
	# Test components
	assert_eq(result.components.size(), 3, "Number of components")
	var mass = find_component(result.components, "mass")
	assert_not_null(mass, "Mass component exists")
	assert_eq(mass.type, "Mass", "Mass component type")
	assert_eq(mass.modifications.get("m"), "1.0", "Mass value")
	
	var damper = find_component(result.components, "damper")
	assert_not_null(damper, "Damper component exists")
	assert_eq(damper.type, "Damper", "Damper component type")
	assert_eq(damper.modifications.get("d"), "0.5", "Damper coefficient")
	
	var fixed = find_component(result.components, "fixed")
	assert_not_null(fixed, "Fixed component exists")
	assert_eq(fixed.type, "Fixed", "Fixed component type")
	
	# Test parameters
	assert_eq(result.parameters.size(), 2, "Number of parameters")
	var x0 = find_parameter(result.parameters, "x0")
	assert_not_null(x0, "x0 parameter exists")
	assert_eq(x0.type, "Real", "x0 parameter type")
	assert_eq(x0.value, "1.0", "x0 value")
	assert_eq(x0.description, "Initial position in meters", "x0 description")
	
	var v0 = find_parameter(result.parameters, "v0")
	assert_not_null(v0, "v0 parameter exists")
	assert_eq(v0.type, "Real", "v0 parameter type")
	assert_eq(v0.value, "0.0", "v0 value")
	assert_eq(v0.description, "Initial velocity in m/s", "v0 description")
	
	# Test initial equations
	assert_eq(result.initial_equations.size(), 2, "Number of initial equations")
	assert_eq(result.initial_equations[0], "mass.s = x0", "First initial equation")
	assert_eq(result.initial_equations[1], "mass.v = v0", "Second initial equation")
	
	# Test connect equations
	assert_eq(result.equations.size(), 2, "Number of equations")
	assert_eq(result.equations[0], "connect(fixed.flange, damper.flange_a)", "First connect equation")
	assert_eq(result.equations[1], "connect(damper.flange_b, mass.flange_a)", "Second connect equation")
	
	# Test annotations
	assert_not_null(result.annotations, "Has annotations")
	assert_not_null(result.annotations.get("content"), "Has annotation content")
	assert_true(result.annotations.content.contains("experiment"), "Has experiment annotation")
	
	print("DampingMassTest.mo tests completed successfully")

func find_component(components: Array, name: String) -> Dictionary:
	for component in components:
		if component.name == name:
			return component
	return {}

func find_parameter(parameters: Array, name: String) -> Dictionary:
	for parameter in parameters:
		if parameter.name == name:
			return parameter
	return {}

func assert_eq(actual, expected, message: String) -> void:
	if actual != expected:
		push_error("Assertion failed: " + message + "\nExpected: " + str(expected) + "\nActual: " + str(actual))

func assert_not_null(value, message: String) -> void:
	if value == null or value.is_empty():
		push_error("Assertion failed: " + message + " (value is null or empty)")

func assert_true(value: bool, message: String) -> void:
	if not value:
		push_error("Assertion failed: " + message + " - Expected true") 