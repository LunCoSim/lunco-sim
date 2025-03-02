#!/usr/bin/env -S godot --headless --script
extends SceneTree

class TestExpressionParser extends "res://apps/modelica/tests/base_test.gd":
	const Parser = preload("res://apps/modelica/core/parser.gd")
	const ModelicaASTNode = preload("res://apps/modelica/core/ast_node.gd")
	const LexerImpl = preload("res://apps/modelica/core/lexer.gd")
	
	var parser
	
	func setup():
		print("Setting up test...")
		# Create a ModelicaParser instance using the factory method
		parser = Parser.create_modelica_parser()
		print("Parser created: " + str(parser))
		
		# Add debug tracing to parser methods
		print("Adding debug tracing to parser...")
		var orig_parse_addition = parser._parse_addition
		parser._parse_addition = func(args=null):
			print("DEBUG: _parse_addition called at token: " + str(parser.current_token.value if parser.current_token else "null"))
			return orig_parse_addition.call()
			
		var orig_parse_term = parser._parse_term
		parser._parse_term = func(args=null):
			print("DEBUG: _parse_term called at token: " + str(parser.current_token.value if parser.current_token else "null"))
			return orig_parse_term.call()
			
		var orig_parse_factor = parser._parse_factor
		parser._parse_factor = func(args=null):
			print("DEBUG: _parse_factor called at token: " + str(parser.current_token.value if parser.current_token else "null"))
			return orig_parse_factor.call()
	
	# Test complex mathematical expressions with mixed operators
	func test_complex_expressions():
		print("Testing complex expressions with mixed operators...")
		
		# Create a simple model with complex expressions
		var model_source = """model ComplexExpressions
			parameter Real a = 1.0;
			parameter Real b = 2.0;
			parameter Real c = 3.0;
			parameter Real d = 4.0;
			parameter Real e = 5.0;
			Real x;
			Real y;
		equation
			x = a + b * c - d / e;
			y = a * (b + (c - d) * e);
		end ComplexExpressions;
		"""
		
		print("Parsing model with complex expressions...")
		var ast = parser.parse(model_source)
		assert_not_null(ast, "AST should not be null")
		
		# Find the equation section
		var equation_section = null
		for node in ast.children:
			if node.type == ModelicaASTNode.NodeType.EQUATION and node.value == "section":
				equation_section = node
				break
		
		assert_not_null(equation_section, "Equation section should exist")
		
		# Get the first equation (x = a + b * c - d / e)
		var first_equation = equation_section.children[0]
		assert_equal(first_equation.type, ModelicaASTNode.NodeType.EQUATION, "Should be an equation")
		assert_equal(first_equation.value, "=", "Equation should be an assignment")
		
		# Verify the right side has correct structure for the expression with operator precedence
		# a + b * c - d / e should be parsed as a + (b * c) - (d / e)
		var rhs = first_equation.children[1]
		
		# This should be a binary operation with + at the top level
		assert_equal(rhs.type, ModelicaASTNode.NodeType.OPERATOR, "RHS should be an operator")
		assert_equal(rhs.value, "-", "Outer operation should be subtraction")
		
		# Get second equation (y = a * (b + (c - d) * e))
		var second_equation = equation_section.children[1]
		
		# Verify the right side has correct structure for the nested expression
		var rhs2 = second_equation.children[1]
		assert_equal(rhs2.type, ModelicaASTNode.NodeType.OPERATOR, "RHS should be an operator")
		assert_equal(rhs2.value, "*", "Outer operation should be multiplication")
		
		print("Complex expression test passed!")
	
	# Test unary operators
	func test_unary_operators():
		print("Testing unary operators...")
		
		var model_source = """model UnaryOperators
			parameter Real a = 1.0;
			parameter Real b = 2.0;
			Real x;
		equation
			x = -a * +b;
		end UnaryOperators;
		"""
		
		var ast = parser.parse(model_source)
		assert_not_null(ast, "AST should not be null")
		
		# Find the equation section
		var equation_section = null
		for node in ast.children:
			if node.type == ModelicaASTNode.NodeType.EQUATION and node.value == "section":
				equation_section = node
				break
		
		assert_not_null(equation_section, "Equation section should exist")
		
		# Get the equation (x = -a * +b)
		var equation = equation_section.children[0]
		var rhs = equation.children[1]
		
		# This should be a binary operation with * at the top level
		assert_equal(rhs.type, ModelicaASTNode.NodeType.OPERATOR, "RHS should be an operator")
		assert_equal(rhs.value, "*", "Operation should be multiplication")
		
		# Left operand should be -a (unary minus)
		var left = rhs.children[0]
		assert_equal(left.type, ModelicaASTNode.NodeType.OPERATOR, "Left operand should be an operator")
		assert_equal(left.value, "-", "Left operand should be unary minus")
		
		# Right operand should be +b (unary plus)
		var right = rhs.children[1]
		assert_equal(right.type, ModelicaASTNode.NodeType.OPERATOR, "Right operand should be an operator")
		assert_equal(right.value, "+", "Right operand should be unary plus")
		
		print("Unary operators test passed!")
	
	# Test nested function calls
	func test_nested_function_calls():
		print("Testing nested function calls...")
		
		var model_source = """model NestedFunctions
			parameter Real x = 0.5;
			Real y;
		equation
			y = sin(cos(x));
		end NestedFunctions;
		"""
		
		var ast = parser.parse(model_source)
		assert_not_null(ast, "AST should not be null")
		
		# Find the equation section
		var equation_section = null
		for node in ast.children:
			if node.type == ModelicaASTNode.NodeType.EQUATION and node.value == "section":
				equation_section = node
				break
		
		assert_not_null(equation_section, "Equation section should exist")
		
		# Get the equation (y = sin(cos(x)))
		var equation = equation_section.children[0]
		var rhs = equation.children[1]
		
		# This should be a function call with sin
		assert_equal(rhs.type, ModelicaASTNode.NodeType.FUNCTION_CALL, "RHS should be a function call")
		assert_equal(rhs.value, "sin", "Outer function should be sin")
		
		# The argument should be cos(x)
		var arg = rhs.children[0]
		assert_equal(arg.type, ModelicaASTNode.NodeType.FUNCTION_CALL, "Argument should be a function call")
		assert_equal(arg.value, "cos", "Inner function should be cos")
		
		print("Nested function calls test passed!")
	
	# Test function calls with complex arguments
	func test_function_calls_with_complex_args():
		print("Testing function calls with complex arguments...")
		
		var model_source = """model ComplexArgs
			parameter Real a = 1.0;
			parameter Real b = 2.0;
			parameter Real c = 3.0;
			parameter Real d = 4.0;
			Real z;
		equation
			z = max(a + b, c * d);
		end ComplexArgs;
		"""
		
		var ast = parser.parse(model_source)
		assert_not_null(ast, "AST should not be null")
		
		# Find the equation section
		var equation_section = null
		for node in ast.children:
			if node.type == ModelicaASTNode.NodeType.EQUATION and node.value == "section":
				equation_section = node
				break
		
		assert_not_null(equation_section, "Equation section should exist")
		
		# Get the equation (z = max(a + b, c * d))
		var equation = equation_section.children[0]
		var rhs = equation.children[1]
		
		# This should be a function call with max
		assert_equal(rhs.type, ModelicaASTNode.NodeType.FUNCTION_CALL, "RHS should be a function call")
		assert_equal(rhs.value, "max", "Function should be max")
		
		# Check that there are 2 arguments
		assert_equal(rhs.children.size(), 2, "Max function should have 2 arguments")
		
		# First argument should be a + b
		var arg1 = rhs.children[0]
		assert_equal(arg1.type, ModelicaASTNode.NodeType.OPERATOR, "First argument should be an operator")
		assert_equal(arg1.value, "+", "First argument should be addition")
		
		# Second argument should be c * d
		var arg2 = rhs.children[1]
		assert_equal(arg2.type, ModelicaASTNode.NodeType.OPERATOR, "Second argument should be an operator")
		assert_equal(arg2.value, "*", "Second argument should be multiplication")
		
		print("Function calls with complex arguments test passed!")
	
	# Test derivative expressions
	func test_derivative_expressions():
		print("Testing derivative expressions...")
		
		var model_source = """model Derivatives
			Real x;
			Real v;
			Real a;
		equation
			v = der(x);
			a = der(v) + der(x);
		end Derivatives;
		"""
		
		var ast = parser.parse(model_source)
		assert_not_null(ast, "AST should not be null")
		
		# Find the equation section
		var equation_section = null
		for node in ast.children:
			if node.type == ModelicaASTNode.NodeType.EQUATION and node.value == "section":
				equation_section = node
				break
		
		assert_not_null(equation_section, "Equation section should exist")
		
		# Get the first equation (v = der(x))
		var first_equation = equation_section.children[0]
		var rhs1 = first_equation.children[1]
		
		# This should be a der function call
		assert_equal(rhs1.type, ModelicaASTNode.NodeType.FUNCTION_CALL, "RHS should be a function call")
		assert_equal(rhs1.value, "der", "Function should be der")
		
		# Get the second equation (a = der(v) + der(x))
		var second_equation = equation_section.children[1]
		var rhs2 = second_equation.children[1]
		
		# This should be an addition of two der calls
		assert_equal(rhs2.type, ModelicaASTNode.NodeType.OPERATOR, "RHS should be an operator")
		assert_equal(rhs2.value, "+", "Operation should be addition")
		
		# Left operand should be der(v)
		var left = rhs2.children[0]
		assert_equal(left.type, ModelicaASTNode.NodeType.FUNCTION_CALL, "Left operand should be a function call")
		assert_equal(left.value, "der", "Left function should be der")
		
		# Right operand should be der(x)
		var right = rhs2.children[1]
		assert_equal(right.type, ModelicaASTNode.NodeType.FUNCTION_CALL, "Right operand should be a function call")
		assert_equal(right.value, "der", "Right function should be der")
		
		print("Derivative expressions test passed!")
	
	# Test error recovery for missing operators
	func test_error_recovery_missing_operators():
		print("Testing error recovery for missing operators...")
		
		# This model has a syntax error: missing operator between 3 and x
		var model_source = """model ErrorRecovery
			parameter Real a = 1.0;
			Real x;
			Real y;
		equation
			# This is intentionally erroneous
			y = 3 x;
		end ErrorRecovery;
		"""
		
		var ast = parser.parse(model_source)
		assert_not_null(ast, "AST should not be null")
		
		# The AST should have error flags
		assert_true(ast.has_errors(), "AST should have errors")
		
		print("Error recovery test passed!")

func _init():
	print("Starting test_expression_parser.gd...")
	var direct_execution = true
	
	# Check if we're being run directly or via the test runner
	for arg in OS.get_cmdline_args():
		if arg.ends_with("run_tests.gd"):
			direct_execution = false
			break
	
	if direct_execution:
		print("\nRunning TestExpressionParser...")
		var test = TestExpressionParser.new()
		print("Running tests...")
		test.run_tests()
		print("Test execution complete, quitting...")
		quit() 