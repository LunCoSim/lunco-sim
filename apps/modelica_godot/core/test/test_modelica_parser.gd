class_name ModelicaTestParser

extends SceneTree

const ModelicaParser = preload("res://apps/modelica_godot/core/parser/modelica/modelica_parser.gd")
const ModelicaLexer = preload("res://apps/modelica_godot/core/parser/modelica/modelica_lexer.gd")
const BaseParser = preload("res://apps/modelica_godot/core/parser/base/syntax_parser.gd")
const BaseLexer = preload("res://apps/modelica_godot/core/parser/base/lexical_analyzer.gd")
const NodeTypes = preload("res://apps/modelica_godot/core/parser/ast/ast_node.gd").NodeType
const ModelicaASTNodeClass = preload("res://apps/modelica_godot/core/parser/ast/ast_node.gd")

var parser: ModelicaParser
var tests_run := 0
var tests_passed := 0
var current_test := ""

func _init():
    print("\nRunning Modelica Parser Tests...")
    _run_all_tests()
    print("\nTests completed: %d/%d passed" % [tests_passed, tests_run])

func _run_all_tests() -> void:
    parser = ModelicaParser.new()
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

func assert_eq(a, b, message: String = ""):
    if a != b:
        push_error("Assertion failed: %s != %s. %s" % [str(a), str(b), message])
    else:
        print("✓ " + message)

func assert_ne(a, b, message: String = ""):
    if a == b:
        push_error("Assertion failed: %s == %s. %s" % [str(a), str(b), message])
    else:
        print("✓ " + message)

func assert_true(condition, message: String = ""):
    if !condition:
        push_error("Assertion failed: Expected true but got false. %s" % message)
    else:
        print("✓ " + message)

func test_parse_simple_model():
    var model = """
    model SimpleModel
        Real x;
        Real v;
    equation
        der(x) = v;
        der(v) = -x;
    end SimpleModel;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    assert_true(result.ast.has("classes"), "AST should have classes")
    assert_true(result.ast.classes.has("SimpleModel"), "AST should have SimpleModel")
    
    var simple_model = result.ast.classes["SimpleModel"]
    assert_eq(simple_model.components.size(), 2, "Model should have 2 components")
    assert_eq(simple_model.equations.size(), 2, "Model should have 2 equations")

func test_parse_component_with_parameters():
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
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var spring = result.ast.classes["Spring"]
    assert_eq(spring.components.size(), 4, "Model should have 4 components")
    
    var k_param = spring.components[0]
    assert_eq(k_param.name, "k")
    assert_eq(k_param.type, "Real")
    assert_true(k_param.is_parameter)
    assert_eq(k_param.default_value, 100)

func test_parse_connector():
    var model = """
    connector Flange
        Real s;
        flow Real f;
    end Flange;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var flange = result.ast.classes["Flange"]
    assert_eq(flange.type, "connector")
    assert_eq(flange.components.size(), 2)
    
    var f_comp = flange.components[1]
    assert_eq(f_comp.name, "f")
    assert_true(f_comp.is_flow)

func test_parse_model_with_extends():
    var model = """
    model DoublePendulum
        extends Modelica.Mechanics.MultiBody.Examples.Elementary.DoublePendulum;
        parameter Real m1 = 1;
        parameter Real m2 = 1;
    end DoublePendulum;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var pendulum = result.ast.classes["DoublePendulum"]
    assert_eq(pendulum.extends_clause, "Modelica.Mechanics.MultiBody.Examples.Elementary.DoublePendulum")

func test_parse_model_with_connect():
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
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var system = result.ast.classes["System"]
    assert_eq(system.equations.size(), 3)
    assert_eq(system.equations[0].type, "connect")

func test_parse_model_with_if():
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
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var controller = result.ast.classes["Controller"]
    assert_eq(controller.equations.size(), 1)
    assert_eq(controller.equations[0].type, "if")

func test_parse_model_with_for():
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
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var array = result.ast.classes["Array"]
    assert_eq(array.equations.size(), 1)
    assert_eq(array.equations[0].type, "for")

func test_parse_model_with_when():
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
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var bounce = result.ast.classes["Bounce"]
    assert_eq(bounce.equations.size(), 3)
    assert_eq(bounce.equations[2].type, "when")

func test_parse_invalid_model():
    var model = """
    model Invalid
        Real x
        equation
        der(x) = 
    end Invalid
    """
    
    var result = parser.parse(model)
    assert_ne(result.error, "", "Error expected for invalid model")

func test_parse_empty_model():
    var model = """
    model Empty
    end Empty;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    assert_true(result.ast.classes.has("Empty"))
    
    var empty = result.ast.classes["Empty"]
    assert_eq(empty.components.size(), 0)
    assert_eq(empty.equations.size(), 0) 