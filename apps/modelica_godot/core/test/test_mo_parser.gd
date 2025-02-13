extends SceneTree

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")

func _init() -> void:
	print("\nRunning Modelica Parser Tests...")
	test_basic_model()
	test_connector()
	test_inheritance()
	print("\nTests completed")
	quit()

func test_basic_model() -> void:
	print("\nTesting basic model parsing...")
	var parser = MOParser.new()
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
	var parser = MOParser.new()
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
	var parser = MOParser.new()
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