#!/usr/bin/env -S godot --headless --script
extends SceneTree

class TestExpressions extends "res://apps/modelica/tests/base_test.gd":
	const Parser = preload("res://apps/modelica/core/parser.gd")
	const ASTNode = preload("res://apps/modelica/core/ast_node.gd")

	var parser

	func setup():
		parser = Parser.create_modelica_parser()

	func test_identifier_expression():
		var source = "variable"
		var ast = parser.parse_expression(source)
		
		assert_not_null(ast, "AST should not be null")
		assert_equal(ast.type, ASTNode.NodeType.IDENTIFIER, "Root should be an identifier")
		assert_equal(ast.value, "variable", "Identifier value should be 'variable'")

	func test_number_expression():
		var source = "42"
		var ast = parser.parse_expression(source)
		
		assert_not_null(ast, "AST should not be null")
		assert_equal(ast.type, ASTNode.NodeType.NUMBER, "Root should be a number")
		assert_equal(ast.value, "42", "Number value should be '42'")

	func test_function_call_expression():
		var source = "sin(x)"
		var ast = parser.parse_expression(source)
		
		assert_not_null(ast, "AST should not be null")
		assert_equal(ast.type, ASTNode.NodeType.FUNCTION_CALL, "Root should be a function call")
		assert_equal(ast.value, "sin", "Function name should be 'sin'")
		
		# Check argument (should use the arguments field, not children)
		assert_equal(ast.arguments.size(), 1, "Function should have 1 argument")
		assert_equal(ast.arguments[0].type, ASTNode.NodeType.IDENTIFIER, "Argument should be an identifier")
		assert_equal(ast.arguments[0].value, "x", "Argument value should be 'x'")

	func test_array_access_expression():
		var source = "array[5]"
		var ast = parser.parse_expression(source)
		
		assert_not_null(ast, "AST should not be null")
		assert_equal(ast.type, ASTNode.NodeType.ARRAY_ACCESS, "Root should be an array access")
		assert_equal(ast.value, "array", "Array name should be 'array'")
		
		# Check index (should use the arguments field, not children)
		assert_equal(ast.arguments.size(), 1, "Array access should have 1 index")
		assert_equal(ast.arguments[0].type, ASTNode.NodeType.NUMBER, "Index should be a number")
		assert_equal(ast.arguments[0].value, "5", "Index value should be '5'")

	func test_unary_operation_expression():
		var source = "-x"
		var ast = parser.parse_expression(source)
		
		assert_not_null(ast, "AST should not be null")
		assert_equal(ast.type, ASTNode.NodeType.OPERATOR, "Root should be an operator")
		assert_equal(ast.value, "-", "Operator should be '-'")
		
		# Check operand (uses the operand field, not children)
		assert_not_null(ast.operand, "Operand should not be null")
		assert_equal(ast.operand.type, ASTNode.NodeType.IDENTIFIER, "Operand should be an identifier")
		assert_equal(ast.operand.value, "x", "Operand should be 'x'")

# Bridge method for the test runner
func run_tests():
	var test_instance = TestExpressions.new()
	return test_instance.run_tests()

func _init():
	var direct_execution = true
	
	# Check if we're being run directly or via the test runner
	for arg in OS.get_cmdline_args():
		if arg.ends_with("run_tests.gd"):
			direct_execution = false
			break
	
	if direct_execution:
		print("\nRunning TestExpressions...")
		var test = TestExpressions.new()
		test.run_tests()
		quit() 