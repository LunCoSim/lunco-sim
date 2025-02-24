extends GutTest

const DAESystem = preload("res://apps/modelica_godot/core/system/dae/dae_system.gd")
const ModelicaParser = preload("res://apps/modelica_godot/core/parser/modelica_parser.gd")

var system: DAESystem
var parser: ModelicaParser

func before_each():
    system = DAESystem.new()
    parser = ModelicaParser.new()

func test_add_variable():
    var var_name = "x"
    var var_type = DAESystem.VariableType.STATE
    var value = 1.0
    var derivative = 0.0
    
    system.add_variable(var_name, var_type, value, derivative)
    
    assert_true(system.variables.has(var_name), "Variable should be added")
    var variable = system.variables[var_name]
    assert_eq(variable.type, var_type)
    assert_eq(variable.value, value)
    assert_eq(variable.derivative, derivative)

func test_add_equation():
    var equation_str = "der(x) = -k * x"
    var result = parser.parse_equation(equation_str)
    assert_eq(result.error, "", "No error expected")
    
    system.add_equation(result.ast)
    
    assert_eq(system.equations.size(), 1, "Equation should be added")
    var equation = system.equations[0]
    assert_eq(equation.ast, result.ast)

func test_add_initial_equation():
    var equation_str = "x = 1.0"
    var result = parser.parse_equation(equation_str)
    assert_eq(result.error, "", "No error expected")
    
    system.add_initial_equation(result.ast)
    
    assert_eq(system.initial_equations.size(), 1, "Initial equation should be added")
    var equation = system.initial_equations[0]
    assert_eq(equation.ast, result.ast)

func test_get_state_variables():
    system.add_variable("x", DAESystem.VariableType.STATE, 1.0, 0.0)
    system.add_variable("y", DAESystem.VariableType.ALGEBRAIC, 2.0)
    system.add_variable("z", DAESystem.VariableType.STATE, 3.0, 0.0)
    
    var state_vars = system.get_state_variables()
    
    assert_eq(state_vars.size(), 2, "Should have 2 state variables")
    assert_true(state_vars.has("x"))
    assert_true(state_vars.has("z"))

func test_get_algebraic_variables():
    system.add_variable("x", DAESystem.VariableType.STATE, 1.0, 0.0)
    system.add_variable("y", DAESystem.VariableType.ALGEBRAIC, 2.0)
    system.add_variable("z", DAESystem.VariableType.ALGEBRAIC, 3.0)
    
    var alg_vars = system.get_algebraic_variables()
    
    assert_eq(alg_vars.size(), 2, "Should have 2 algebraic variables")
    assert_true(alg_vars.has("y"))
    assert_true(alg_vars.has("z"))

func test_initialize():
    system.add_variable("x", DAESystem.VariableType.STATE, 1.0, 0.0)
    var init_eq = parser.parse_equation("x = 2.0").ast
    system.add_initial_equation(init_eq)
    
    var success = system.initialize()
    
    assert_true(success, "Initialization should succeed")
    assert_eq(system.variables["x"].value, 2.0)

func test_solve_continuous():
    # Setup a simple harmonic oscillator
    system.add_variable("x", DAESystem.VariableType.STATE, 1.0, 0.0)
    system.add_variable("v", DAESystem.VariableType.STATE, 0.0, 0.0)
    system.add_variable("k", DAESystem.VariableType.PARAMETER, 1.0)
    system.add_variable("m", DAESystem.VariableType.PARAMETER, 1.0)
    
    var eq1 = parser.parse_equation("der(x) = v").ast
    var eq2 = parser.parse_equation("der(v) = -k * x / m").ast
    system.add_equation(eq1)
    system.add_equation(eq2)
    
    system.initialize()
    var success = system.solve_continuous(0.1)  # dt = 0.1
    
    assert_true(success, "Continuous solve should succeed")
    # For harmonic oscillator, energy should be conserved
    var energy = 0.5 * system.variables["m"].value * system.variables["v"].value * system.variables["v"].value + \
                 0.5 * system.variables["k"].value * system.variables["x"].value * system.variables["x"].value
    assert_almost_eq(energy, 0.5, 0.01)  # Initial energy was 0.5 * k * x^2 = 0.5

func test_to_string():
    system.add_variable("x", DAESystem.VariableType.STATE, 1.0, 0.0)
    system.add_variable("v", DAESystem.VariableType.STATE, 0.0, 0.0)
    var eq = parser.parse_equation("der(x) = v").ast
    system.add_equation(eq)
    
    var str_repr = str(system)
    
    assert_true(str_repr.contains("x"), "String representation should contain variable x")
    assert_true(str_repr.contains("v"), "String representation should contain variable v")
    assert_true(str_repr.contains("der(x) = v"), "String representation should contain equation")