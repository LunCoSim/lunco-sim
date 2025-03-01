#!/usr/bin/env -S godot --headless --script
extends SceneTree

class TestLogicalOperators extends "res://apps/modelica/tests/base_test.gd":
	const LexicalAnalyzer = preload("res://apps/modelica/core/lexer.gd")
	
	var basic_lexer: LexicalAnalyzer
	var modelica_lexer: LexicalAnalyzer
	var equation_lexer: LexicalAnalyzer
	
	func setup():
		basic_lexer = LexicalAnalyzer.new(LexicalAnalyzer.LexerMode.BASIC)
		modelica_lexer = LexicalAnalyzer.create_modelica_lexer()
		equation_lexer = LexicalAnalyzer.create_equation_lexer()
	
	func test_logical_operators_in_basic_mode():
		# In basic mode, logical operators should be treated as identifiers
		var tokens = basic_lexer.tokenize("and or not")
		
		# Should have 3 identifier tokens + EOF
		assert_equal(tokens.size(), 4, "Should have 3 tokens + EOF")
		
		# Check that they are all identifiers
		assert_equal(tokens[0].type, LexicalAnalyzer.TokenType.IDENTIFIER, "First token should be IDENTIFIER")
		assert_equal(tokens[1].type, LexicalAnalyzer.TokenType.IDENTIFIER, "Second token should be IDENTIFIER")
		assert_equal(tokens[2].type, LexicalAnalyzer.TokenType.IDENTIFIER, "Third token should be IDENTIFIER")
		
		# Check the values
		assert_equal(tokens[0].value, "and", "First token should be 'and'")
		assert_equal(tokens[1].value, "or", "Second token should be 'or'")
		assert_equal(tokens[2].value, "not", "Third token should be 'not'")
	
	func test_logical_operators_in_modelica_mode():
		# In Modelica mode, logical operators should be treated as keywords
		var tokens = modelica_lexer.tokenize("and or not")
		
		# Should have 3 keyword tokens + EOF
		assert_equal(tokens.size(), 4, "Should have 3 tokens + EOF")
		
		# Check that they are all keywords
		assert_equal(tokens[0].type, LexicalAnalyzer.TokenType.KEYWORD, "First token should be KEYWORD")
		assert_equal(tokens[1].type, LexicalAnalyzer.TokenType.KEYWORD, "Second token should be KEYWORD")
		assert_equal(tokens[2].type, LexicalAnalyzer.TokenType.KEYWORD, "Third token should be KEYWORD")
		
		# Check the values
		assert_equal(tokens[0].value, "and", "First token should be 'and'")
		assert_equal(tokens[1].value, "or", "Second token should be 'or'")
		assert_equal(tokens[2].value, "not", "Third token should be 'not'")
	
	func test_logical_operators_in_equation_mode():
		# In equation mode, logical operators should be treated as operators
		var tokens = equation_lexer.tokenize("and or not")
		
		# Should have 3 operator tokens + EOF
		assert_equal(tokens.size(), 4, "Should have 3 tokens + EOF")
		
		# Check that they are all operators
		assert_equal(tokens[0].type, LexicalAnalyzer.TokenType.OPERATOR, "First token should be OPERATOR")
		assert_equal(tokens[1].type, LexicalAnalyzer.TokenType.OPERATOR, "Second token should be OPERATOR")
		assert_equal(tokens[2].type, LexicalAnalyzer.TokenType.OPERATOR, "Third token should be OPERATOR")
		
		# Check the values
		assert_equal(tokens[0].value, "and", "First token should be 'and'")
		assert_equal(tokens[1].value, "or", "Second token should be 'or'")
		assert_equal(tokens[2].value, "not", "Third token should be 'not'")
	
	func test_mixed_expression_in_equation_mode():
		# Test a mixed expression with logical operators in equation mode
		var tokens = equation_lexer.tokenize("x > 0 and y < 10 or not z == 5")
		
		# Print tokens for debugging
		for i in range(tokens.size()):
			print("Token %d: %s" % [i, tokens[i]])
		
		# Check that operators (>, <, ==) and logical operators (and, or, not) are classified as OPERATOR
		assert_equal(tokens[1].type, LexicalAnalyzer.TokenType.OPERATOR, "Token '>' should be OPERATOR")
		assert_equal(tokens[3].type, LexicalAnalyzer.TokenType.OPERATOR, "Token 'and' should be OPERATOR")
		assert_equal(tokens[5].type, LexicalAnalyzer.TokenType.OPERATOR, "Token '<' should be OPERATOR")
		assert_equal(tokens[7].type, LexicalAnalyzer.TokenType.OPERATOR, "Token 'or' should be OPERATOR")
		assert_equal(tokens[8].type, LexicalAnalyzer.TokenType.OPERATOR, "Token 'not' should be OPERATOR")
		assert_equal(tokens[10].type, LexicalAnalyzer.TokenType.OPERATOR, "Token '==' should be OPERATOR")
		
		# Check that variables are classified as IDENTIFIER
		assert_equal(tokens[0].type, LexicalAnalyzer.TokenType.IDENTIFIER, "Token 'x' should be IDENTIFIER")
		assert_equal(tokens[4].type, LexicalAnalyzer.TokenType.IDENTIFIER, "Token 'y' should be IDENTIFIER")
		assert_equal(tokens[9].type, LexicalAnalyzer.TokenType.IDENTIFIER, "Token 'z' should be IDENTIFIER")
		
		# Check that numbers are classified as NUMBER
		assert_equal(tokens[2].type, LexicalAnalyzer.TokenType.NUMBER, "Token '0' should be NUMBER")
		assert_equal(tokens[6].type, LexicalAnalyzer.TokenType.NUMBER, "Token '10' should be NUMBER")
		assert_equal(tokens[11].type, LexicalAnalyzer.TokenType.NUMBER, "Token '5' should be NUMBER")

func _init():
	var test = TestLogicalOperators.new()
	test.run_tests()
	quit() 