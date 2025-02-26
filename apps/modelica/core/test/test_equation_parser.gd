extends SceneTree

const EquationParser = preload("res://apps/modelica_godot/core/parser/equation_parser.gd")

var parser: EquationParser

func _ready():
    run_all_tests()

func run_all_tests():
    parser = EquationParser.new()
    
    print("\nRunning Equation Parser Tests:")
    print("------------------------------")
    
    test_simple_equation()
    test_binary_expression()
    test_unary_expression()
    test_function_call()
    test_derivative()
    test_complex_equation()
    test_invalid_equation()
    test_missing_right_side()
    test_missing_left_side()
    
    print("\nAll tests completed!")
    get_tree().quit()

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

func assert_almost_eq(a, b, tolerance: float, message: String = ""):
    if abs(a - b) > tolerance:
        push_error("Assertion failed: |%s - %s| > %s. %s" % [str(a), str(b), str(tolerance), message])
    else:
        print("✓ " + message)

func test_simple_equation():
    var result = parser.parse("x = 5")
    assert_eq(result.error, "", "No error expected")
    assert_eq(result.ast.type, "equation")
    assert_eq(result.ast.left.type, "identifier")
    assert_eq(result.ast.left.value, "x")
    assert_eq(result.ast.right.type, "number")
    assert_eq(result.ast.right.value, 5)

func test_binary_expression():
    var result = parser.parse("y = a + b * c")
    assert_eq(result.error, "", "No error expected")
    assert_eq(result.ast.type, "equation")
    assert_eq(result.ast.left.type, "identifier")
    assert_eq(result.ast.left.value, "y")
    assert_eq(result.ast.right.type, "binary")
    assert_eq(result.ast.right.operator, "+")
    assert_eq(result.ast.right.left.type, "identifier")
    assert_eq(result.ast.right.left.value, "a")
    assert_eq(result.ast.right.right.type, "binary")
    assert_eq(result.ast.right.right.operator, "*")
    assert_eq(result.ast.right.right.left.type, "identifier")
    assert_eq(result.ast.right.right.left.value, "b")
    assert_eq(result.ast.right.right.right.type, "identifier")
    assert_eq(result.ast.right.right.right.value, "c")

func test_unary_expression():
    var result = parser.parse("z = -x")
    assert_eq(result.error, "", "No error expected")
    assert_eq(result.ast.type, "equation")
    assert_eq(result.ast.left.type, "identifier")
    assert_eq(result.ast.left.value, "z")
    assert_eq(result.ast.right.type, "unary")
    assert_eq(result.ast.right.operator, "-")
    assert_eq(result.ast.right.operand.type, "identifier")
    assert_eq(result.ast.right.operand.value, "x")

func test_function_call():
    var result = parser.parse("y = sin(x)")
    assert_eq(result.error, "", "No error expected")
    assert_eq(result.ast.type, "equation")
    assert_eq(result.ast.left.type, "identifier")
    assert_eq(result.ast.left.value, "y")
    assert_eq(result.ast.right.type, "call")
    assert_eq(result.ast.right.function, "sin")
    assert_eq(result.ast.right.arguments.size(), 1)
    assert_eq(result.ast.right.arguments[0].type, "identifier")
    assert_eq(result.ast.right.arguments[0].value, "x")

func test_derivative():
    var result = parser.parse("der(x) = v")
    assert_eq(result.error, "", "No error expected")
    assert_eq(result.ast.type, "equation")
    assert_eq(result.ast.left.type, "derivative")
    assert_eq(result.ast.left.variable, "x")
    assert_eq(result.ast.right.type, "identifier")
    assert_eq(result.ast.right.value, "v")

func test_complex_equation():
    var result = parser.parse("der(v) = -k * x / m - c * v / m + f / m")
    assert_eq(result.error, "", "No error expected")
    assert_eq(result.ast.type, "equation")
    assert_eq(result.ast.left.type, "derivative")
    assert_eq(result.ast.left.variable, "v")
    assert_eq(result.ast.right.type, "binary")
    assert_eq(result.ast.right.operator, "+")

func test_invalid_equation():
    var result = parser.parse("x + y")
    assert_ne(result.error, "", "Error expected for invalid equation")

func test_missing_right_side():
    var result = parser.parse("x =")
    assert_ne(result.error, "", "Error expected for missing right side")

func test_missing_left_side():
    var result = parser.parse("= x")
    assert_ne(result.error, "", "Error expected for missing left side") 