@tool
extends SceneTree

const ModelicaParser = preload("res://apps/modelica_godot/core/parser/modelica/modelica_parser.gd")
const ModelicaLexer = preload("res://apps/modelica_godot/core/parser/modelica/modelica_lexer.gd")
const BaseParser = preload("res://apps/modelica_godot/core/parser/base/syntax_parser.gd")
const BaseLexer = preload("res://apps/modelica_godot/core/parser/base/lexical_analyzer.gd")
const ModelicaASTNode = preload("res://apps/modelica_godot/core/parser/ast/ast_node.gd")
const NodeTypes = ModelicaASTNode.NodeType
const ModelicaTypeClass = preload("res://apps/modelica_godot/core/parser/types/modelica_type.gd")

var parser: ModelicaParser
var tests_run := 0
var tests_passed := 0
var current_test := ""

func setup() -> void:
	parser = ModelicaParser.new()
	tests_run = 0
	tests_passed = 0
	current_test = ""

func teardown() -> void:
	if parser:
		parser.free()
	parser = null

func before_each_test(test_name: String) -> void:
	current_test = test_name
	print("\nRunning test: " + test_name)
	setup()

func after_each_test() -> void:
	teardown()

func _init():
	print("\nRunning Modelica Parser Tests...")
	_run_all_tests()
	print("\nTests completed: %d/%d passed" % [tests_passed, tests_run])
	quit()

func _run_all_tests() -> void:
	test_parse_simple_model()
	test_parse_component_with_parameters()
	test_parse_connector()
	test_parse_model_with_extends()
	test_parse_model_with_connect()
	test_parse_model_with_if()
	test_parse_model_with_for()
	test_parse_model_with_when()
	test_parse_invalid_model()
	test_parse_empty_model()

func assert_eq(a, b, message: String = "") -> bool:
	tests_run += 1
	if a != b:
		print("❌ Assertion failed: %s != %s. %s" % [str(a), str(b), message])
		return false
	print("✓ " + message)
	tests_passed += 1
	return true

func assert_true(condition: bool, message: String = "") -> bool:
	tests_run += 1
	if not condition:
		print("❌ Assertion failed: Expected true but got false. %s" % message)
		return false
	print("✓ " + message)
	tests_passed += 1
	return true

func assert_false(condition: bool, message: String = "") -> bool:
	return assert_true(not condition, message)

func assert_node_type(node: ModelicaASTNode, expected_type: int, message: String = "") -> bool:
	return assert_eq(node.type, expected_type, message + " (node type check)")

func assert_no_errors(node: ModelicaASTNode, message: String = "") -> bool:
	if node.has_errors:
		print("❌ Unexpected errors in node: ", node.errors)
		return false
	return assert_true(true, message + " (no errors check)")

func find_child_by_type(node: ModelicaASTNode, type: int) -> Array:
	var result: Array = []
	for child in node.children:
		if child.type == type:
			result.append(child)
	return result

func test_parse_simple_model() -> void:
	before_each_test("test_parse_simple_model")
	
	var model = """
	model SimpleModel
		Real x;
		Real v;
	equation
		der(x) = v;
		der(v) = -x;
	end SimpleModel;
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_no_errors(ast)
	
	var models = find_child_by_type(ast, NodeTypes.MODEL)
	assert_eq(models.size(), 1, "Should have one model")
	
	var simple_model = models[0]
	assert_eq(simple_model.value, "SimpleModel", "Model name check")
	assert_eq(simple_model.modelica_type.kind, ModelicaTypeClass.TypeKind.MODEL, "Model type check")
	
	var components = find_child_by_type(simple_model, NodeTypes.COMPONENT)
	assert_eq(components.size(), 2, "Should have two components")
	
	var equations = find_child_by_type(simple_model, NodeTypes.EQUATION)
	assert_eq(equations.size(), 2, "Should have two equations")
	
	after_each_test()

func test_parse_component_with_parameters() -> void:
	before_each_test("test_parse_component_with_parameters")
	
	var model = """
	model Spring
		parameter Real k = 100;
		parameter Real l0 = 1;
		Real f;
		Real s;
	equation
		f = k * (s - l0);
	end Spring;
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_no_errors(ast)
	
	var models = find_child_by_type(ast, NodeTypes.MODEL)
	assert_eq(models.size(), 1, "Should have one model")
	
	var spring = models[0]
	assert_eq(spring.value, "Spring", "Model name check")
	
	var components = find_child_by_type(spring, NodeTypes.COMPONENT)
	assert_eq(components.size(), 4, "Should have four components")
	
	var params = components.filter(func(c): return c.variability == "parameter")
	assert_eq(params.size(), 2, "Should have two parameters")
	
	var k_param = params[0]
	assert_eq(k_param.value, "k", "Parameter name check")
	assert_true(k_param.modelica_type.is_numeric(), "Parameter type check")
	assert_eq(k_param.modifications.get("value").value, 100, "Parameter value check")
	
	after_each_test()

func test_parse_connector() -> void:
	before_each_test("test_parse_connector")
	
	var model = """
	connector Flange
		Real s;
		flow Real f;
	end Flange;
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_no_errors(ast)
	
	var connectors = find_child_by_type(ast, NodeTypes.CONNECTOR)
	assert_eq(connectors.size(), 1, "Should have one connector")
	
	var flange = connectors[0]
	assert_eq(flange.value, "Flange", "Connector name check")
	assert_eq(flange.modelica_type.kind, ModelicaTypeClass.TypeKind.CONNECTOR, "Connector type check")
	
	var components = find_child_by_type(flange, NodeTypes.COMPONENT)
	assert_eq(components.size(), 2, "Should have two components")
	
	var flow_var = components[1]
	assert_true(flow_var.causality == "flow", "Flow variable check")
	
	after_each_test()

func test_parse_model_with_extends() -> void:
	before_each_test("test_parse_model_with_extends")
	
	var model = """
	model DoublePendulum
		extends Modelica.Mechanics.MultiBody.Examples.Elementary.DoublePendulum;
		parameter Real m1 = 1;
		parameter Real m2 = 1;
	end DoublePendulum;
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_no_errors(ast)
	
	var models = find_child_by_type(ast, NodeTypes.MODEL)
	var pendulum = models[0]
	
	var extends_nodes = find_child_by_type(pendulum, NodeTypes.EXTENDS)
	assert_eq(extends_nodes.size(), 1, "Should have one extends clause")
	assert_eq(extends_nodes[0].value, "Modelica.Mechanics.MultiBody.Examples.Elementary.DoublePendulum", "Extends clause check")
	
	after_each_test()

func test_parse_model_with_connect() -> void:
	before_each_test("test_parse_model_with_connect")
	
	var model = """
	model System
		Spring spring1;
		Spring spring2;
		Flange flange1;
		Flange flange2;
	equation
		connect(spring1.flange_a, flange1);
		connect(spring1.flange_b, spring2.flange_a);
		connect(spring2.flange_b, flange2);
	end System;
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_no_errors(ast)
	
	var models = find_child_by_type(ast, NodeTypes.MODEL)
	var system = models[0]
	
	var components = find_child_by_type(system, NodeTypes.COMPONENT)
	assert_eq(components.size(), 4, "Should have four components")
	
	var equations = find_child_by_type(system, NodeTypes.CONNECT_EQUATION)
	assert_eq(equations.size(), 3, "Should have three connect equations")
	
	after_each_test()

func test_parse_model_with_if() -> void:
	before_each_test("test_parse_model_with_if")
	
	var model = """
	model Controller
		Real x;
		Real y;
	equation
		if x > 0 then
			y = 2*x;
		else
			y = 0;
		end if;
	end Controller;
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_no_errors(ast)
	
	var models = find_child_by_type(ast, NodeTypes.MODEL)
	var controller = models[0]
	
	var if_equations = find_child_by_type(controller, NodeTypes.IF_EQUATION)
	assert_eq(if_equations.size(), 1, "Should have one if equation")
	
	after_each_test()

func test_parse_model_with_for() -> void:
	before_each_test("test_parse_model_with_for")
	
	var model = """
	model Array
		parameter Integer n = 10;
		Real x[n];
		Real y[n];
	equation
		for i in 1:n loop
			der(x[i]) = -y[i];
			der(y[i]) = x[i];
		end for;
	end Array;
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_no_errors(ast)
	
	var models = find_child_by_type(ast, NodeTypes.MODEL)
	var array = models[0]
	
	var components = find_child_by_type(array, NodeTypes.COMPONENT)
	assert_eq(components.size(), 3, "Should have three components")
	
	# Check array types
	var arrays = components.filter(func(c): return c.modelica_type.kind == ModelicaTypeClass.TypeKind.ARRAY)
	assert_eq(arrays.size(), 2, "Should have two array components")
	
	var for_equations = find_child_by_type(array, NodeTypes.FOR_EQUATION)
	assert_eq(for_equations.size(), 1, "Should have one for equation")
	
	after_each_test()

func test_parse_model_with_when() -> void:
	before_each_test("test_parse_model_with_when")
	
	var model = """
	model Bounce
		Real h;
		Real v;
	equation
		der(h) = v;
		der(v) = -9.81;
		when h <= 0 then
			reinit(v, -0.9*pre(v));
		end when;
	end Bounce;
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_no_errors(ast)
	
	var models = find_child_by_type(ast, NodeTypes.MODEL)
	var bounce = models[0]
	
	var when_equations = find_child_by_type(bounce, NodeTypes.WHEN_EQUATION)
	assert_eq(when_equations.size(), 1, "Should have one when equation")
	
	after_each_test()

func test_parse_invalid_model() -> void:
	before_each_test("test_parse_invalid_model")
	
	var model = """
	model Invalid
		Real x
		equation
		der(x) = 
	end Invalid
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_true(ast.has_errors, "Should have errors")
	
	after_each_test()

func test_parse_empty_model() -> void:
	before_each_test("test_parse_empty_model")
	
	var model = """
	model Empty
	end Empty;
	"""
	
	var ast = parser.parse(model)
	assert_node_type(ast, NodeTypes.ROOT)
	assert_no_errors(ast)
	
	var models = find_child_by_type(ast, NodeTypes.MODEL)
	assert_eq(models.size(), 1, "Should have one model")
	
	var empty = models[0]
	assert_eq(empty.children.size(), 0, "Should have no children")
	
	after_each_test() 