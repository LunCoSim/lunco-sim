extends SceneTree

const MOParser = preload("res://core/mo_parser.gd")

var tests_run := 0
var tests_passed := 0
var current_test := ""
var test_dir := "res://components/Mechanical/"

func _init():
	print("\nRunning Modelica Parser Tests...")
	_run_all_tests()
	print("\nTests completed: %d/%d passed" % [tests_passed, tests_run])
	quit()

func _run_all_tests() -> void:
	_test_spring_parsing()
	_test_mass_parsing()

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

func _test_spring_parsing() -> void:
	_start_test("Spring Model Parsing")
	
	var parser = MOParser.new()
	var model = parser.parse_file(test_dir + "Spring.mo")
	
	print("Model data:", model)
	
	_assert(model.type == "model", "Correct model type")
	_assert(model.name == "Spring", "Correct model name")
	_assert(model.description.contains("1D spring"), "Description parsed")
	
	# Test parameters
	var found_k = false
	var found_l0 = false
	for param in model.get("parameters", []):
		match param.name:
			"k":
				found_k = true
				_assert(param.value == "1.0", "Spring constant default value")
				_assert(param.unit == "N/m", "Spring constant unit")
			"l0":
				found_l0 = true
				_assert(param.value == "1.0", "Natural length default value")
				_assert(param.unit == "m", "Natural length unit")
	
	_assert(found_k, "Found spring constant parameter")
	_assert(found_l0, "Found natural length parameter")
	
	# Test connectors
	var found_port_a = false
	var found_port_b = false
	for comp in model.get("components", []):
		match comp.name:
			"port_a":
				found_port_a = true
				_assert(comp.type == "MechanicalConnector", "Port A type correct")
			"port_b":
				found_port_b = true
				_assert(comp.type == "MechanicalConnector", "Port B type correct")
	
	_assert(found_port_a, "Found port A")
	_assert(found_port_b, "Found port B")
	
	# Test equations
	var equations = model.get("equations", [])
	_assert(equations.size() >= 4, "Found all equations")
	
	var has_length = false
	var has_elongation = false
	var has_force_a = false
	var has_force_b = false
	
	for eq in equations:
		if eq.contains("length = port_b.position - port_a.position"):
			has_length = true
		elif eq.contains("elongation = length - l0"):
			has_elongation = true
		elif eq.contains("port_a.force = k * elongation"):
			has_force_a = true
		elif eq.contains("port_b.force = -k * elongation"):
			has_force_b = true
	
	_assert(has_length, "Found length equation")
	_assert(has_elongation, "Found elongation equation")
	_assert(has_force_a, "Found force A equation")
	_assert(has_force_b, "Found force B equation")

func _test_mass_parsing() -> void:
	_start_test("Mass Model Parsing")
	
	var parser = MOParser.new()
	var model = parser.parse_file(test_dir + "Mass.mo")
	
	print("Model data:", model)
	
	_assert(model.type == "model", "Correct model type")
	_assert(model.name == "Mass", "Correct model name")
	_assert(model.description.contains("Point mass"), "Description parsed")
	
	# Test parameters
	var found_mass = false
	var found_x0 = false
	var found_v0 = false
	for param in model.get("parameters", []):
		match param.name:
			"m":
				found_mass = true
				_assert(param.value == "1.0", "Mass default value")
				_assert(param.unit == "kg", "Mass unit")
			"x0":
				found_x0 = true
				_assert(param.value == "0.0", "Initial position default value")
				_assert(param.unit == "m", "Position unit")
			"v0":
				found_v0 = true
				_assert(param.value == "0.0", "Initial velocity default value")
				_assert(param.unit == "m/s", "Velocity unit")
	
	_assert(found_mass, "Found mass parameter")
	_assert(found_x0, "Found initial position parameter")
	_assert(found_v0, "Found initial velocity parameter")
	
	# Test connector
	var found_port = false
	for comp in model.get("components", []):
		if comp.name == "port":
			found_port = true
			_assert(comp.type == "MechanicalConnector", "Port type correct")
	
	_assert(found_port, "Found port")
	
	# Test equations
	var equations = model.get("equations", [])
	_assert(equations.size() >= 3, "Found all equations")
	
	var has_newton = false
	var has_velocity = false
	var has_acceleration = false
	
	for eq in equations:
		if eq.contains("m * acceleration = port.force"):
			has_newton = true
		elif eq.contains("der(port.position) = port.velocity"):
			has_velocity = true
		elif eq.contains("der(port.velocity) = acceleration"):
			has_acceleration = true
	
	_assert(has_newton, "Found Newton's law equation")
	_assert(has_velocity, "Found velocity equation")
	_assert(has_acceleration, "Found acceleration equation") 