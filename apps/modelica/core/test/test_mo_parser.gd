@tool
extends SceneTree

const MOParser = preload("../parser/mo_parser.gd")

var parser: MOParser

func _init() -> void:
	print("\nRunning Modelica Parser Tests...")
	parser = MOParser.new()
	_run_all_tests()
	quit()

func _run_all_tests() -> void:
	test_basic_model()
	test_connector()
	test_inheritance()
	test_parameter_basic_types()
	test_parameter_modifications()
	test_parameter_annotations()
	test_parameter_scientific_notation()
	test_parameter_error_handling()

func test_basic_model() -> void:
	print("\nTesting basic model parsing...")
	var test_model = """
model Spring "A simple spring model"
	extends BaseSpring;
	parameter Real k = 100 "Spring constant";
	Flange_a flange_a;
	Flange_b flange_b;
equation
	f = k * (s_rel);
	flange_a.f + flange_b.f = 0;
end Spring;
"""
	
	var result = parser.parse_text(test_model)
	
	assert(result.type == "model", "Correct model type")
	assert(result.name == "Spring", "Correct model name")
	assert(result.extends.size() == 1, "Has one extends clause")
	assert(result.extends[0].base_class == "BaseSpring", "Correct base class")
	assert(result.components.size() == 3, "Has three components")  # k, flange_a, flange_b
	assert(result.equations.size() == 2, "Has two equations")
	print("  ✓ Basic model tests passed")

func test_connector() -> void:
	print("\nTesting connector parsing...")
	var test_connector = """
connector Flange_a "Flange of a 1D mechanical system"
	flow Real f "Force";
	Real s "Position";
end Flange_a;
"""
	
	var result = parser.parse_text(test_connector)
	
	assert(result.type == "connector", "Correct type")
	assert(result.name == "Flange_a", "Correct name")
	assert(result.components.size() == 2, "Has two components")  # f and s
	
	var f_comp = result.components[0]
	assert("flow" in f_comp.attributes, "Flow attribute present")
	assert(f_comp.type == "Real", "Correct type")
	assert(f_comp.name == "f", "Correct name")
	print("  ✓ Connector tests passed")

func test_inheritance() -> void:
	print("\nTesting inheritance parsing...")
	var test_model = """
model DerivedModel
	extends BaseModel(param1=100, param2="test");
	extends AnotherBase;
	Real x;
equation
	der(x) = -x;
end DerivedModel;
"""
	
	var result = parser.parse_text(test_model)
	
	assert(result.extends.size() == 2, "Has two extends clauses")
	assert(result.extends[0].base_class == "BaseModel", "First base class correct")
	assert(result.extends[0].modifications.has("param1"), "Has param1 modification")
	assert(result.extends[0].modifications["param1"] == "100", "param1 value correct")
	print("  ✓ Inheritance tests passed")

func test_parameter_basic_types() -> void:
	print("\nTesting basic parameter types...")
	
	var test_model = """
	model TestModel
		parameter Real p1 = 1.0;
		parameter Integer i1 = 42;
		parameter Boolean b1 = true;
		parameter String s1 = "test";
	end TestModel;
	"""
	
	var result = parser.parse_text(test_model)
	assert_not_null(result, "Model parsed successfully")
	
	var params = result.get("parameters", [])
	assert_eq(params.size(), 4, "Found all parameters")
	
	# Test Real parameter
	var p1 = _find_parameter(params, "p1")
	assert_not_null(p1, "Real parameter exists")
	assert_eq(p1.get("type"), "Real", "Correct Real type")

func test_parameter_modifications() -> void:
	print("\nTesting parameter modifications...")
	
	var test_model = """
	model TestModel
		parameter Real p1(min=0, max=10) = 5.0 "With bounds";
		parameter Real p2(fixed=false) = 2.0 "Not fixed";
		parameter Real p3(unit="kg") = 1.0 "With unit";
	end TestModel;
	"""
	
	var result = parser.parse_text(test_model)
	assert_not_null(result, "Model parsed successfully")
	
	var params = result.get("parameters", [])
	assert_eq(params.size(), 3, "Found all parameters")
	
	# Test parameter with bounds
	var p1 = _find_parameter(params, "p1")
	assert_not_null(p1, "Bounded parameter exists")
	assert_eq(float(p1.get("min", "0")), 0.0, "Correct min value")
	assert_eq(float(p1.get("max", "10")), 10.0, "Correct max value")
	
	# Test parameter with fixed=false
	var p2 = _find_parameter(params, "p2")
	assert_not_null(p2, "Non-fixed parameter exists")
	assert_eq(p2.get("fixed"), false, "Parameter is not fixed")
	
	# Test parameter with unit
	var p3 = _find_parameter(params, "p3")
	assert_not_null(p3, "Parameter with unit exists")
	assert_eq(p3.get("unit"), "kg", "Correct unit")
	
	print("  ✓ Parameter modifications test passed")

func test_parameter_annotations() -> void:
	print("\nTesting parameter annotations...")
	
	var test_model = """
	model TestModel
		parameter Real p1 = 1.0 annotation(Evaluate=false);
		parameter Real p2 = 2.0 annotation(Evaluate=true);
	end TestModel;
	"""
	
	var result = parser.parse_text(test_model)
	assert_not_null(result, "Model parsed successfully")
	
	var params = result.get("parameters", [])
	assert_eq(params.size(), 2, "Found all parameters")
	
	# Test non-evaluable parameter
	var p1 = _find_parameter(params, "p1")
	assert_not_null(p1, "Non-evaluable parameter exists")
	assert_eq(p1.get("evaluate"), false, "Parameter is not evaluable")
	
	# Test evaluable parameter
	var p2 = _find_parameter(params, "p2")
	assert_not_null(p2, "Evaluable parameter exists")
	assert_eq(p2.get("evaluate"), true, "Parameter is evaluable")
	
	print("  ✓ Parameter annotations test passed")

func test_parameter_scientific_notation() -> void:
	print("\nTesting scientific notation in parameters...")
	
	var test_model = """
	model TestModel
		parameter Real p1 = 1.5e-3;
		parameter Real p2 = 2.0E+6;
	end TestModel;
	"""
	
	var result = parser.parse_text(test_model)
	assert_not_null(result, "Model parsed successfully")
	
	var params = result.get("parameters", [])
	assert_eq(params.size(), 2, "Found all parameters")
	
	# Test small number
	var p1 = _find_parameter(params, "p1")
	assert_not_null(p1, "Small number parameter exists")
	assert_approx(float(p1.get("value", "0")), 0.0015, "Correct small number value")
	
	# Test large number
	var p2 = _find_parameter(params, "p2")
	assert_not_null(p2, "Large number parameter exists")
	assert_approx(float(p2.get("value", "0")), 2000000.0, "Correct large number value")
	
	print("  ✓ Scientific notation test passed")

func test_parameter_error_handling() -> void:
	print("\nTesting parameter error handling...")
	
	var test_model = """
	model TestModel
		parameter Real p1 = -1.0 (min=0, max=10);  // Value out of bounds
		parameter Integer i1 = 1.5;  // Non-integer value
		parameter Boolean b1 = 42;   // Non-boolean value
	end TestModel;
	"""
	
	var result = parser.parse_text(test_model)
	assert_not_null(result, "Model parsed successfully")
	
	var params = result.get("parameters", [])
	
	# Test out-of-bounds parameter
	var p1 = _find_parameter(params, "p1")
	assert_not_null(p1, "Out-of-bounds parameter exists")
	assert_true(float(p1.get("value", "0")) < float(p1.get("min", "0")), "Value is correctly out of bounds")
	
	# Test invalid integer
	var i1 = _find_parameter(params, "i1")
	assert_not_null(i1, "Invalid integer parameter exists")
	assert_true(not is_integer(i1.get("value", "0")), "Non-integer value detected")
	
	# Test invalid boolean
	var b1 = _find_parameter(params, "b1")
	assert_not_null(b1, "Invalid boolean parameter exists")
	assert_true(not is_boolean(b1.get("value", "false")), "Non-boolean value detected")
	
	print("  ✓ Error handling test passed")

func _find_parameter(params: Array, name: String) -> Dictionary:
	for param in params:
		if param.get("name") == name:
			return param
	return {}

func assert_not_null(value, message: String) -> void:
	if value == null or (value is Dictionary and value.is_empty()):
		push_error("Assertion failed: " + message)
	else:
		print("  ✓ " + message)

func assert_eq(actual, expected, message: String) -> void:
	if actual != expected:
		push_error("Assertion failed: " + message + "\n  Expected: " + str(expected) + "\n  Got: " + str(actual))
	else:
		print("  ✓ " + message)

func assert_true(condition: bool, message: String) -> void:
	if not condition:
		push_error("Assertion failed: " + message)
	else:
		print("  ✓ " + message)

func assert_approx(actual: float, expected: float, message: String, tolerance: float = 0.0001) -> void:
	if abs(actual - expected) > tolerance:
		push_error("Assertion failed: " + message + "\n  Expected: " + str(expected) + "\n  Got: " + str(actual))
	else:
		print("  ✓ " + message)

func is_integer(value: String) -> bool:
	# Check if string represents an integer
	if value.is_empty():
		return false
	for c in value:
		if not c.is_valid_int():
			return false
	return true

func is_boolean(value: String) -> bool:
	# Check if string represents a boolean
	return value == "true" or value == "false" 