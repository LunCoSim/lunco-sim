extends GDScript

class_name TestMOParser

var tests_run := 0
var tests_passed := 0
var current_test := ""

func _init():
	print("\nRunning Modelica Parser Tests...")
	_run_all_tests()
	print("\nTests completed: %d/%d passed" % [tests_passed, tests_run])

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
	var model = parser.parse_file("res://components/mechanical/Spring.mo")
	
	_assert(model.type == "model", "Correct model type")
	_assert(model.name == "Spring", "Correct model name")
	_assert(model.description.contains("1D translational spring"), "Description parsed")
	
	# Test parameters
	var found_k = false
	for param in model.parameters:
		if param.name == "k":
			found_k = true
			_assert(param.value == 1.0, "Spring constant default value")
			_assert(param.unit == "N/m", "Spring constant unit")
	_assert(found_k, "Found spring constant parameter")
	
	# Test equations
	var has_force_balance = false
	for eq in model.equations:
		if eq.contains("f = k*(s_rel - s_rel0)"):
			has_force_balance = true
	_assert(has_force_balance, "Found force balance equation")

func _test_mass_parsing() -> void:
	_start_test("Mass Model Parsing")
	
	var parser = MOParser.new()
	var model = parser.parse_file("res://components/mechanical/Mass.mo")
	
	_assert(model.type == "model", "Correct model type")
	_assert(model.name == "Mass", "Correct model name")
	_assert(model.description.contains("Sliding mass"), "Description parsed")
	
	# Test parameters
	var found_mass = false
	for param in model.parameters:
		if param.name == "m":
			found_mass = true
			_assert(param.value == 1.0, "Mass default value")
			_assert(param.unit == "kg", "Mass unit")
	_assert(found_mass, "Found mass parameter")
	
	# Test state variables
	var has_position = false
	var has_velocity = false
	for var_def in model.variables:
		match var_def.name:
			"s":
				has_position = true
				_assert(var_def.unit == "m", "Position unit")
			"v":
				has_velocity = true
				_assert(var_def.unit == "m/s", "Velocity unit")
	
	_assert(has_position, "Found position variable")
	_assert(has_velocity, "Found velocity variable")
	
	# Test equations
	var has_newton_law = false
	for eq in model.equations:
		if eq.contains("m*der(v)"):
			has_newton_law = true
	_assert(has_newton_law, "Found Newton's law equation") 