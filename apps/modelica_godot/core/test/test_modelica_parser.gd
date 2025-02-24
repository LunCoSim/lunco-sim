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
    setup()

func after_each_test() -> void:
    teardown()

func _init():
    print("\nRunning Modelica Parser Tests...")
    _run_all_tests()
    print("\nTests completed: %d/%d passed" % [tests_passed, tests_run])

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
    test_parse_model_with_algorithm()
    test_parse_model_with_initial_equations()
    test_parse_model_with_protected()
    test_parse_stream_connector()
    test_parse_model_with_assert()
    test_parse_model_with_terminate()
    test_parse_multiple_inheritance()
    test_parse_redeclare()
    test_parse_replaceable()
    test_parse_conditional_components()
    test_parse_type_definitions()
    test_parse_function()
    test_parse_external_function()
    test_parse_pure_impure_functions()
    test_parse_operator_overloading()
    test_parse_record()

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

func test_parse_model_with_algorithm():
    before_each_test("test_parse_model_with_algorithm")
    var model = """
    model AlgorithmTest
        Real x(start=0);
        Real y;
    algorithm
        when sample(0, 0.1) then
            x := pre(x) + 1;
            y := x^2;
        end when;
    equation
        when x > 10 then
            reinit(x, 0);
        end when;
    end AlgorithmTest;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var algo_test = result.ast.classes["AlgorithmTest"]
    assert_eq(algo_test.components.size(), 2)
    assert_eq(algo_test.algorithms.size(), 1)
    assert_eq(algo_test.equations.size(), 1)
    after_each_test()

func test_parse_model_with_initial_equations():
    before_each_test("test_parse_model_with_initial_equations")
    var model = """
    model InitialTest
        Real x;
        Real v;
    initial equation
        x = 1;
        v = 0;
    equation
        der(x) = v;
        der(v) = -x;
    end InitialTest;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var init_test = result.ast.classes["InitialTest"]
    assert_eq(init_test.initial_equations.size(), 2)
    assert_eq(init_test.equations.size(), 2)
    after_each_test()

func test_parse_model_with_protected():
    before_each_test("test_parse_model_with_protected")
    var model = """
    model ProtectedTest
        public 
            Real x;
            Real y;
        protected
            Real internal;
        equation
            der(x) = internal;
            der(y) = -internal;
            internal = -(x + y);
    end ProtectedTest;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var protected_test = result.ast.classes["ProtectedTest"]
    assert_eq(protected_test.public_components.size(), 2)
    assert_eq(protected_test.protected_components.size(), 1)
    after_each_test()

func test_parse_stream_connector():
    before_each_test("test_parse_stream_connector")
    var model = """
    connector FluidPort
        Real p;
        flow Real m_flow;
        stream Real h_outflow;
        stream Real s_outflow;
    end FluidPort;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var fluid_port = result.ast.classes["FluidPort"]
    assert_eq(fluid_port.type, "connector")
    
    var stream_vars = 0
    var flow_vars = 0
    for comp in fluid_port.components:
        if comp.is_stream:
            stream_vars += 1
        if comp.is_flow:
            flow_vars += 1
    
    assert_eq(stream_vars, 2, "Should have 2 stream variables")
    assert_eq(flow_vars, 1, "Should have 1 flow variable")
    after_each_test()

func test_parse_model_with_assert():
    before_each_test("test_parse_model_with_assert")
    var model = """
    model AssertTest
        Real x(min=-1, max=1);
    equation
        der(x) = -x;
        assert(x >= -1 and x <= 1, "x must be between -1 and 1");
    end AssertTest;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var assert_test = result.ast.classes["AssertTest"]
    assert_eq(assert_test.equations.size(), 2)
    assert_eq(assert_test.equations[1].type, "assert")
    after_each_test()

func test_parse_model_with_terminate():
    before_each_test("test_parse_model_with_terminate")
    var model = """
    model TerminateTest
        Real x(start=1);
    equation
        der(x) = -x;
        when x < 0.01 then
            terminate("Simulation terminated: x too small");
        end when;
    end TerminateTest;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var terminate_test = result.ast.classes["TerminateTest"]
    assert_eq(terminate_test.equations.size(), 2)
    assert_eq(terminate_test.equations[1].type, "when")
    after_each_test()

func test_parse_multiple_inheritance():
    before_each_test("test_parse_multiple_inheritance")
    var model = """
    model MultiInheritance
        extends TwoMasses;
        extends Damper(d=100);
        extends Spring(c=1000);
        parameter Real m1 = 1 "Mass 1";
        parameter Real m2 = 1 "Mass 2";
    equation
        connect(mass1.flange_b, spring.flange_a);
        connect(spring.flange_b, mass2.flange_a);
        connect(mass1.flange_b, damper.flange_a);
        connect(damper.flange_b, mass2.flange_a);
    end MultiInheritance;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var multi = result.ast.classes["MultiInheritance"]
    assert_eq(multi.extends_clauses.size(), 3)
    assert_eq(multi.components.size(), 2)
    assert_eq(multi.equations.size(), 4)
    after_each_test()

func test_parse_redeclare():
    before_each_test("test_parse_redeclare")
    var model = """
    model RedeclareTest
        extends PartialSystem(
            redeclare model Controller = PIDController(k=100, Ti=0.1, Td=0.01),
            redeclare package Medium = Modelica.Media.Water.StandardWater
        );
        redeclare Real x;
        redeclare model Plant = DetailedPlant;
    end RedeclareTest;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var redeclare = result.ast.classes["RedeclareTest"]
    assert_eq(redeclare.extends_clauses.size(), 1)
    assert_true(redeclare.extends_clauses[0].has_redeclarations)
    after_each_test()

func test_parse_replaceable():
    before_each_test("test_parse_replaceable")
    var model = """
    model ReplaceableTest
        replaceable model Controller = PI
            constrainedby PartialController;
        replaceable package Medium = IdealGas
            constrainedby PartialMedium;
        Controller controller;
        Sensor sensor(redeclare package Medium = Medium);
    end ReplaceableTest;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var replaceable = result.ast.classes["ReplaceableTest"]
    assert_eq(replaceable.replaceable_types.size(), 2)
    assert_eq(replaceable.components.size(), 2)
    after_each_test()

func test_parse_conditional_components():
    before_each_test("test_parse_conditional_components")
    var model = """
    model ConditionalTest
        parameter Boolean use_cooling = false;
        parameter Boolean use_heating = true;
        Modelica.Blocks.Interfaces.RealInput T_ref;
        Modelica.Blocks.Interfaces.RealOutput Q_flow;
        Cooler cooler if use_cooling;
        Heater heater if use_heating;
    equation
        if use_cooling then
            connect(cooler.T_in, T_ref);
            Q_flow = cooler.Q_flow;
        else
            Q_flow = 0;
        end if;
    end ConditionalTest;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var conditional = result.ast.classes["ConditionalTest"]
    assert_eq(conditional.components.size(), 6)
    
    var conditional_comps = 0
    for comp in conditional.components:
        if comp.has_condition:
            conditional_comps += 1
    
    assert_eq(conditional_comps, 2, "Should have 2 conditional components")
    after_each_test()

func test_parse_type_definitions():
    before_each_test("test_parse_type_definitions")
    var model = """
    model TypeTest
        type Angle = Real(unit="rad", displayUnit="deg");
        type Force = Real(unit="N");
        type Velocity = Real(unit="m/s", min=0);
        type Temperature = Real(unit="K", min=0, nominal=300);
        Angle theta;
        Force f;
        Velocity v;
        Temperature T;
    equation
        der(theta) = v;
        f = sin(theta);
        T = 300;
    end TypeTest;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var type_test = result.ast.classes["TypeTest"]
    assert_eq(type_test.type_definitions.size(), 4)
    assert_eq(type_test.components.size(), 4)
    after_each_test()

func test_parse_function():
    before_each_test("test_parse_function")
    var model = """
    function quadratic "Evaluates quadratic function"
        input Real a;
        input Real b;
        input Real c;
        input Real x;
        output Real y "= a*x^2 + b*x + c";
    algorithm
        y := a*x^2 + b*x + c;
    end quadratic;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var quadratic = result.ast.classes["quadratic"]
    assert_eq(quadratic.type, "function")
    assert_eq(quadratic.inputs.size(), 4)
    assert_eq(quadratic.outputs.size(), 1)
    assert_eq(quadratic.algorithms.size(), 1)
    after_each_test()

func test_parse_external_function():
    before_each_test("test_parse_external_function")
    var model = """
    function readMatrixSize "Read matrix size from binary file"
        input String fileName;
        output Integer n;
        output Integer m;
    external "C"
        n = read_matrix_size(fileName, m);
        annotation(Include="#include <matrix_utils.h>");
    end readMatrixSize;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var read_matrix = result.ast.classes["readMatrixSize"]
    assert_eq(read_matrix.type, "function")
    assert_true(read_matrix.is_external)
    assert_eq(read_matrix.external_language, "C")
    after_each_test()

func test_parse_pure_impure_functions():
    before_each_test("test_parse_pure_impure_functions")
    var model = """
    function pureFunc
        input Real x;
        output Real y;
        pure annotation(Inline=true);
    algorithm
        y := sin(x);
    end pureFunc;

    function impureFunc
        input String fileName;
        output Real data[:];
        impure;
    external "C" data = readDataFromFile(fileName);
    end impureFunc;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var pure_func = result.ast.classes["pureFunc"]
    assert_true(pure_func.is_pure)
    assert_true(pure_func.has_annotation("Inline"))
    
    var impure_func = result.ast.classes["impureFunc"]
    assert_true(impure_func.is_impure)
    assert_true(impure_func.is_external)
    after_each_test()

func test_parse_operator_overloading():
    before_each_test("test_parse_operator_overloading")
    var model = """
    operator '*' "Multiplication of complex numbers"
        input Complex c1;
        input Complex c2;
        output Complex result;
    algorithm
        result.re := c1.re*c2.re - c1.im*c2.im;
        result.im := c1.re*c2.im + c1.im*c2.re;
        annotation(Inline=true);
    end '*';
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var mult_op = result.ast.classes["*"]
    assert_eq(mult_op.type, "operator")
    assert_eq(mult_op.inputs.size(), 2)
    assert_eq(mult_op.outputs.size(), 1)
    assert_eq(mult_op.algorithms.size(), 1)
    after_each_test()

func test_parse_record():
    before_each_test("test_parse_record")
    var model = """
    record Complex "Complex number with operator overloading"
        Real re "Real part";
        Real im "Imaginary part";
        
        encapsulated operator function '+'
            input Complex c1;
            input Complex c2;
            output Complex result;
        algorithm
            result.re := c1.re + c2.re;
            result.im := c1.im + c2.im;
        end '+';
        
        encapsulated operator function 'String'
            input Complex c;
            output String s;
        algorithm
            s := String(c.re) + " + " + String(c.im) + "i";
        end 'String';
    end Complex;
    """
    
    var result = parser.parse(model)
    assert_eq(result.error, "", "No error expected")
    
    var complex = result.ast.classes["Complex"]
    assert_eq(complex.type, "record")
    assert_eq(complex.components.size(), 2)
    assert_eq(complex.operator_functions.size(), 2)
    after_each_test() 