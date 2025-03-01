extends BaseTest

const ModelicaLexer = preload("res://apps/modelica/core/lexer.gd")

var lexer: ModelicaLexer

func setup():
	lexer = ModelicaLexer.new()

func test_number_tokens():
	var source = "123 45.67 0.1 1e5 1.2e-3"
	lexer.init(source)
	
	var token = lexer.next_token()
	assert_equal(token.type, "NUMBER")
	assert_equal(token.value, "123")
	
	token = lexer.next_token()
	assert_equal(token.type, "NUMBER")
	assert_equal(token.value, "45.67")
	
	token = lexer.next_token()
	assert_equal(token.type, "NUMBER")
	assert_equal(token.value, "0.1")
	
	token = lexer.next_token()
	assert_equal(token.type, "NUMBER")
	assert_equal(token.value, "1e5")
	
	token = lexer.next_token()
	assert_equal(token.type, "NUMBER")
	assert_equal(token.value, "1.2e-3")
	
	token = lexer.next_token()
	assert_equal(token.type, "EOF")

func test_identifier_tokens():
	var source = "variable x123 _test model"
	lexer.init(source)
	
	var token = lexer.next_token()
	assert_equal(token.type, "IDENTIFIER")
	assert_equal(token.value, "variable")
	
	token = lexer.next_token()
	assert_equal(token.type, "IDENTIFIER")
	assert_equal(token.value, "x123")
	
	token = lexer.next_token()
	assert_equal(token.type, "IDENTIFIER")
	assert_equal(token.value, "_test")
	
	token = lexer.next_token()
	assert_equal(token.type, "MODEL")  # This should be a keyword
	assert_equal(token.value, "model")
	
	token = lexer.next_token()
	assert_equal(token.type, "EOF")

func test_operator_tokens():
	var source = "+ - * / = == <> < > <= >="
	lexer.init(source)
	
	var token = lexer.next_token()
	assert_equal(token.type, "PLUS")
	
	token = lexer.next_token()
	assert_equal(token.type, "MINUS")
	
	token = lexer.next_token()
	assert_equal(token.type, "ASTERISK")
	
	token = lexer.next_token()
	assert_equal(token.type, "SLASH")
	
	token = lexer.next_token()
	assert_equal(token.type, "EQUALS")
	
	token = lexer.next_token()
	assert_equal(token.type, "DOUBLE_EQUALS")
	
	token = lexer.next_token()
	assert_equal(token.type, "NOT_EQUALS")
	
	token = lexer.next_token()
	assert_equal(token.type, "LESS_THAN")
	
	token = lexer.next_token()
	assert_equal(token.type, "GREATER_THAN")
	
	token = lexer.next_token()
	assert_equal(token.type, "LESS_THAN_EQUALS")
	
	token = lexer.next_token()
	assert_equal(token.type, "GREATER_THAN_EQUALS")
	
	token = lexer.next_token()
	assert_equal(token.type, "EOF")

func test_comment_tokens():
	var source = "x // This is a line comment\ny /* This is a\nblock comment */ z"
	lexer.init(source)
	
	var token = lexer.next_token()
	assert_equal(token.type, "IDENTIFIER")
	assert_equal(token.value, "x")
	
	# Line comment should be skipped
	token = lexer.next_token()
	assert_equal(token.type, "IDENTIFIER")
	assert_equal(token.value, "y")
	
	# Block comment should be skipped
	token = lexer.next_token()
	assert_equal(token.type, "IDENTIFIER")
	assert_equal(token.value, "z")
	
	token = lexer.next_token()
	assert_equal(token.type, "EOF")

func test_keyword_tokens():
	var source = "model end equation parameter Real Integer Boolean String"
	lexer.init(source)
	
	var token = lexer.next_token()
	assert_equal(token.type, "MODEL")
	
	token = lexer.next_token()
	assert_equal(token.type, "END")
	
	token = lexer.next_token()
	assert_equal(token.type, "EQUATION")
	
	token = lexer.next_token()
	assert_equal(token.type, "PARAMETER")
	
	token = lexer.next_token()
	assert_equal(token.type, "REAL")
	
	token = lexer.next_token()
	assert_equal(token.type, "INTEGER")
	
	token = lexer.next_token()
	assert_equal(token.type, "BOOLEAN")
	
	token = lexer.next_token()
	assert_equal(token.type, "STRING")
	
	token = lexer.next_token()
	assert_equal(token.type, "EOF")

func test_string_tokens():
	var source = '"Simple string" "String with \\"escaped\\" quotes" "Multi-line\nstring"'
	lexer.init(source)
	
	var token = lexer.next_token()
	assert_equal(token.type, "STRING_LITERAL")
	assert_equal(token.value, "Simple string")
	
	token = lexer.next_token()
	assert_equal(token.type, "STRING_LITERAL")
	assert_equal(token.value, 'String with "escaped" quotes')
	
	token = lexer.next_token()
	assert_equal(token.type, "STRING_LITERAL")
	assert_equal(token.value, "Multi-line\nstring")
	
	token = lexer.next_token()
	assert_equal(token.type, "EOF")

func test_line_tracking():
	var source = "line1\nline2\nline3"
	lexer.init(source)
	
	var token = lexer.next_token()
	assert_equal(token.line, 1)
	assert_equal(token.column, 1)
	
	token = lexer.next_token()
	assert_equal(token.line, 2)
	assert_equal(token.column, 1)
	
	token = lexer.next_token()
	assert_equal(token.line, 3)
	assert_equal(token.column, 1)
	
	token = lexer.next_token()
	assert_equal(token.type, "EOF") 