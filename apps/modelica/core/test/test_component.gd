class_name ModelicaTestComponent

extends SceneTree

const ModelicaComponent = preload("res://apps/modelica_godot/core/modelica/modelica_component.gd")
const ModelicaConnector = preload("res://apps/modelica_godot/core/modelica/modelica_connector.gd")
const ModelicaVariable = preload("res://apps/modelica_godot/core/modelica/modelica_variable.gd")

# Test framework setup
var tests_run := 0
var tests_passed := 0
var current_test := ""

func _init():
	print("\nRunning Component Tests...")
	_run_all_tests()
	print("\nTests completed: %d/%d passed" % [tests_passed, tests_run])

func _run_all_tests() -> void:
	_test_basic_component()
	_test_mechanical_spring()
	_test_electrical_resistor()
	_test_thermal_conductor()
	_test_save_load()

func _start_test(test_name: String) -> void:
	current_test = test_name
	tests_run += 1
	print("\nRunning test: " + test_name)

func _assert(condition: bool, message: String) -> void:
	if condition:
		tests_passed += 1
		print("  ✓ " + message)
	else:
		print("  ✗ " + message)
		push_error("Test failed: " + current_test + " - " + message)

# Basic component tests
func _test_basic_component() -> void:
	_start_test("Basic Component Creation")
	
	var comp = ModelicaComponent.new("test", "Test component")
	
	_assert(comp != null, "Component created successfully")
	_assert(comp.get_declaration(comp.declarations.keys()[0]).name == "test", "Component name set correctly")
	
	var p1 = comp.add_variable("p1", ModelicaVariable.VariableKind.PARAMETER)
	p1.set_value(10.0)
	p1.set_unit(ModelicaVariable.Unit.METER)
	_assert(float(p1.value) == 10.0, "Parameter value set correctly")
	_assert(p1.unit == ModelicaVariable.Unit.METER, "Parameter unit set correctly")
	
	var v1 = comp.add_variable("v1", ModelicaVariable.VariableKind.REGULAR)
	v1.set_value(5.0)
	v1.set_unit(ModelicaVariable.Unit.NEWTON)
	_assert(float(v1.value) == 5.0, "Variable value set correctly")
	_assert(v1.unit == ModelicaVariable.Unit.NEWTON, "Variable unit set correctly")

# Test mechanical spring component
func _test_mechanical_spring() -> void:
	_start_test("Mechanical Spring Component")
	
	var spring = _create_spring_component()
	
	_assert(spring.get_connector("p1") != null, "Port 1 created")
	_assert(spring.get_connector("p2") != null, "Port 2 created")
	var k = spring.get_variable("k")
	_assert(float(k.value) == 100.0, "Spring constant set")
	_assert(spring.equations.size() == 2, "Spring equations added")
	
	# Test connection
	var mass = _create_mass_component()
	var p1 = spring.get_connector("p1")
	var p2 = mass.get_connector("p1")
	
	_assert(p1 != null and p2 != null, "Connectors retrieved successfully")
	var connection_result = p1.connect_to(p2)
	_assert(connection_result, "Spring connected to mass")

# Test electrical resistor component
func _test_electrical_resistor() -> void:
	_start_test("Electrical Resistor Component")
	
	var resistor = _create_resistor_component()
	
	_assert(resistor.get_connector("p1") != null, "Port 1 created")
	_assert(resistor.get_connector("p2") != null, "Port 2 created")
	var R = resistor.get_variable("R")
	_assert(float(R.value) == 100.0, "Resistance set")
	_assert(resistor.equations.size() == 2, "Resistor equations added")

# Test thermal conductor component
func _test_thermal_conductor() -> void:
	_start_test("Thermal Conductor Component")
	
	var conductor = _create_thermal_conductor_component()
	
	_assert(conductor.get_connector("p1") != null, "Port 1 created")
	_assert(conductor.get_connector("p2") != null, "Port 2 created")
	var G = conductor.get_variable("G")
	_assert(float(G.value) == 0.01, "Thermal conductance set")
	_assert(conductor.equations.size() == 2, "Conductor equations added")

# Test save and load functionality
func _test_save_load() -> void:
	_start_test("Component Save/Load")
	
	var spring = _create_spring_component()
	var save_data = spring.to_dict()
	
	var loaded_spring = ModelicaComponent.new("loaded_spring")
	loaded_spring.from_dict(save_data)
	
	var k1 = spring.get_variable("k")
	var k2 = loaded_spring.get_variable("k")
	_assert(float(k1.value) == float(k2.value), "Parameter preserved")
	_assert(loaded_spring.equations.size() == spring.equations.size(), "Equations preserved")
	_assert(loaded_spring.get_connector("p1").type == spring.get_connector("p1").type, "Connector type preserved")

# Helper functions to create test components
func _create_spring_component() -> ModelicaComponent:
	var spring = ModelicaComponent.new("spring", "Mechanical spring")
	spring.add_connector("p1", ModelicaConnector.ConnectorType.MECHANICAL)
	spring.add_connector("p2", ModelicaConnector.ConnectorType.MECHANICAL)
	var k = spring.add_variable("k", ModelicaVariable.VariableKind.PARAMETER)
	k.set_value(100.0)
	k.set_unit(ModelicaVariable.Unit.NEWTON)
	spring.add_equation("f = k * (p1.position - p2.position)")
	spring.add_equation("p1.force + p2.force = 0")
	return spring

func _create_mass_component() -> ModelicaComponent:
	var mass = ModelicaComponent.new("mass", "Point mass")
	mass.add_connector("p1", ModelicaConnector.ConnectorType.MECHANICAL)
	var m = mass.add_variable("m", ModelicaVariable.VariableKind.PARAMETER)
	m.set_value(1.0)
	
	var pos = mass.add_state_variable("position", 0.0)
	pos.set_unit(ModelicaVariable.Unit.METER)
	
	var vel = mass.add_state_variable("velocity", 0.0)
	vel.set_unit(ModelicaVariable.Unit.METER)
	
	mass.add_equation("der(position) = velocity")
	mass.add_equation("m * der(velocity) = p1.force")
	return mass

func _create_resistor_component() -> ModelicaComponent:
	var resistor = ModelicaComponent.new("resistor", "Electrical resistor")
	resistor.add_connector("p1", ModelicaConnector.ConnectorType.ELECTRICAL)
	resistor.add_connector("p2", ModelicaConnector.ConnectorType.ELECTRICAL)
	var R = resistor.add_variable("R", ModelicaVariable.VariableKind.PARAMETER)
	R.set_value(100.0)
	resistor.add_equation("v = R * i")
	resistor.add_equation("p1.current + p2.current = 0")
	return resistor

func _create_thermal_conductor_component() -> ModelicaComponent:
	var conductor = ModelicaComponent.new("conductor", "Thermal conductor")
	conductor.add_connector("p1", ModelicaConnector.ConnectorType.THERMAL)
	conductor.add_connector("p2", ModelicaConnector.ConnectorType.THERMAL)
	var G = conductor.add_variable("G", ModelicaVariable.VariableKind.PARAMETER)
	G.set_value(0.01)
	G.set_unit(ModelicaVariable.Unit.WATT)
	conductor.add_equation("Q_flow = G * (p1.temperature - p2.temperature)")
	conductor.add_equation("p1.heat_flow + p2.heat_flow = 0")
	return conductor 
