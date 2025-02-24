@tool
extends LexicalAnalyzer
class_name ModelicaLexer

const MODELICA_KEYWORDS: Array[String] = [
	"model", "connector", "package", "class", "record", "block",
	"type", "function", "extends", "parameter", "constant", "input",
	"output", "flow", "stream", "equation", "algorithm", "end",
	"if", "then", "else", "elseif", "for", "loop", "in", "while",
	"when", "elsewhen", "connect", "der", "initial", "inner", "outer",
	"public", "protected", "final", "each", "partial", "redeclare",
	"replaceable", "import", "within", "encapsulated", "annotation",
	"external", "and", "or", "not", "true", "false"
]

const MODELICA_OPERATORS: Array[String] = [
	"+", "-", "*", "/", "^", "=", "<", ">", "<=", ">=", "==", "<>",
	":=", ".", ",", ";", "(", ")", "[", "]", "{", "}", ":", ".."
]

func _init() -> void:
	super._init()
	set_keywords(MODELICA_KEYWORDS)

func _next_token() -> Token:
	if _position >= _text.length():
		return null
	
	var c = _current_char()
	
	# Skip whitespace and newlines
	while _position < _text.length() and (c.strip_edges().is_empty() or c == "\n"):
		if c == "\n":
			_line += 1
			_column = 1
		else:
			_column += 1
		_position += 1
		if _position < _text.length():
			c = _current_char()
		else:
			return null
	
	# Handle identifiers and keywords
	if c.is_valid_identifier():
		return _handle_identifier()
	
	# Handle numbers
	if c.is_valid_hex_number() or c == "-":
		return _handle_number()
	
	# Handle operators and punctuation
	return _handle_operator()

func _handle_identifier() -> Token:
	var start_pos = _position
	var start_col = _column
	var identifier = ""
	
	while _position < _text.length():
		var c = _current_char()
		if not (c.is_valid_identifier() or c == "_" or (identifier.length() > 0 and c.is_valid_hex_number())):
			break
		identifier += c
		_position += 1
		_column += 1
	
	# Check if it's a keyword
	if _keywords.has(identifier):
		return Token.new(TokenType.KEYWORD, identifier, _line, start_col, start_pos)
	
	return Token.new(TokenType.IDENTIFIER, identifier, _line, start_col, start_pos)

func _handle_number() -> Token:
	var start_pos = _position
	var start_col = _column
	var number = ""
	var is_float = false
	
	# Handle negative numbers
	if _current_char() == "-":
		number += "-"
		_position += 1
		_column += 1
	
	while _position < _text.length():
		var c = _current_char()
		
		if c >= "0" and c <= "9":
			number += c
		elif c == "." and not is_float:
			number += c
			is_float = true
		elif c == "e" or c == "E":
			# Handle scientific notation
			number += c
			_position += 1
			_column += 1
			
			# Handle optional sign in exponent
			c = _current_char()
			if c == "+" or c == "-":
				number += c
				_position += 1
				_column += 1
		else:
			break
		
		_position += 1
		_column += 1
	
	return Token.new(TokenType.NUMBER, number, _line, start_col, start_pos)

func _handle_operator() -> Token:
	var start_pos = _position
	var start_col = _column
	var c = _current_char()
	
	# Handle Modelica-specific operators
	var two_char = c + _peek_next()
	if two_char in ["<=", ">=", "==", "<>", ":=", ".."]:
		_position += 2
		_column += 2
		return Token.new(TokenType.PUNCTUATION, two_char, _line, start_col, start_pos)
	
	# Handle single-character operators and punctuation
	_position += 1
	_column += 1
	
	# All operators are treated as punctuation for simplicity
	return Token.new(TokenType.PUNCTUATION, c, _line, start_col, start_pos)

func _current_char() -> String:
	if _position < _text.length():
		return _text[_position]
	return ""

func _peek_next() -> String:
	if _position + 1 < _text.length():
		return _text[_position + 1]
	return "" 