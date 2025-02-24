@tool
extends RefCounted
class_name LexicalAnalyzer

class Token:
	var type: int
	var value: String
	var line: int
	var column: int
	var position: int
	
	func _init(p_type: int, p_value: String, p_line: int, p_column: int, p_position: int) -> void:
		type = p_type
		value = p_value
		line = p_line
		column = p_column
		position = p_position
	
	func _to_string() -> String:
		return "Token(%d, '%s', line=%d, col=%d)" % [type, value, line, column]

enum TokenType {
	UNKNOWN,
	EOF,
	WHITESPACE,
	NEWLINE,
	IDENTIFIER,
	NUMBER,
	STRING,
	OPERATOR,
	KEYWORD,
	COMMENT,
	PUNCTUATION
}

var _text: String = ""
var _length: int = 0
var _position: int = 0
var _line: int = 1
var _column: int = 1
var _keywords: Array[String] = []

func _init() -> void:
	pass

func set_keywords(keywords: Array[String]) -> void:
	_keywords = keywords

func tokenize(text: String) -> Array[Token]:
	_text = text
	_length = text.length()
	_position = 0
	_line = 1
	_column = 1
	
	var tokens: Array[Token] = []
	
	while _position < _length:
		var token = _next_token()
		if token != null:
			tokens.append(token)
	
	# Add EOF token
	tokens.append(Token.new(TokenType.EOF, "", _line, _column, _position))
	return tokens

func _next_token() -> Token:
	if _position >= _length:
		return null
	
	var c = _current_char()
	
	# Skip whitespace
	if c.strip_edges().is_empty():
		return _handle_whitespace()
	
	# Handle comments
	if c == "/" and _peek_next() == "/":
		return _handle_line_comment()
	if c == "/" and _peek_next() == "*":
		return _handle_block_comment()
	
	# Handle identifiers and keywords
	if c.is_valid_identifier():
		return _handle_identifier()
	
	# Handle numbers
	if c.is_valid_int() or (c == "-" and _peek_next().is_valid_int()):
		return _handle_number()
	
	# Handle strings
	if c == "\"":
		return _handle_string()
	
	# Handle operators and punctuation
	return _handle_operator()

func _handle_whitespace() -> Token:
	var start_pos = _position
	var start_col = _column
	
	while _position < _length and _current_char().strip_edges().is_empty():
		if _current_char() == "\n":
			_line += 1
			_column = 1
		else:
			_column += 1
		_position += 1
	
	return Token.new(
		TokenType.WHITESPACE,
		_text.substr(start_pos, _position - start_pos),
		_line,
		start_col,
		start_pos
	)

func _handle_line_comment() -> Token:
	var start_pos = _position
	var start_col = _column
	
	# Skip //
	_position += 2
	_column += 2
	
	while _position < _length and _current_char() != "\n":
		_position += 1
		_column += 1
	
	return Token.new(
		TokenType.COMMENT,
		_text.substr(start_pos, _position - start_pos),
		_line,
		start_col,
		start_pos
	)

func _handle_block_comment() -> Token:
	var start_pos = _position
	var start_col = _column
	var start_line = _line
	
	# Skip /*
	_position += 2
	_column += 2
	
	while _position < _length:
		if _current_char() == "*" and _peek_next() == "/":
			_position += 2
			_column += 2
			break
		
		if _current_char() == "\n":
			_line += 1
			_column = 1
		else:
			_column += 1
		
		_position += 1
	
	return Token.new(
		TokenType.COMMENT,
		_text.substr(start_pos, _position - start_pos),
		start_line,
		start_col,
		start_pos
	)

func _handle_identifier() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	
	while _position < _length:
		var c = _current_char()
		if not (c.is_valid_identifier() or c == "_" or c.is_valid_int()):
			break
		value += c
		_position += 1
		_column += 1
	
	var type = TokenType.IDENTIFIER
	if _keywords.has(value):
		type = TokenType.KEYWORD
	
	return Token.new(type, value, _line, start_col, start_pos)

func _handle_number() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	var has_decimal = false
	var has_exponent = false
	
	# Handle negative numbers
	if _current_char() == "-":
		value += _current_char()
		_position += 1
		_column += 1
	
	while _position < _length:
		var c = _current_char()
		
		# Handle decimal point
		if c == "." and not has_decimal and not has_exponent:
			has_decimal = true
			value += c
			_position += 1
			_column += 1
			continue
		
		# Handle exponent
		if (c == "e" or c == "E") and not has_exponent:
			has_exponent = true
			value += c
			_position += 1
			_column += 1
			
			# Handle exponent sign
			if _position < _length and (_peek_next() == "+" or _peek_next() == "-"):
				value += _peek_next()
				_position += 1
				_column += 1
			continue
		
		if not c.is_valid_int():
			break
			
		value += c
		_position += 1
		_column += 1
	
	return Token.new(TokenType.NUMBER, value, _line, start_col, start_pos)

func _handle_string() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	
	# Skip opening quote
	_position += 1
	_column += 1
	
	while _position < _length:
		var c = _current_char()
		
		if c == "\"":
			# Skip closing quote
			_position += 1
			_column += 1
			break
			
		if c == "\\":
			# Handle escape sequences
			_position += 1
			_column += 1
			if _position < _length:
				value += "\\" + _current_char()
				_position += 1
				_column += 1
			continue
		
		value += c
		_position += 1
		_column += 1
	
	return Token.new(TokenType.STRING, value, _line, start_col, start_pos)

func _handle_operator() -> Token:
	var start_pos = _position
	var start_col = _column
	var c = _current_char()
	
	# Handle multi-character operators
	var two_char = c + _peek_next()
	if two_char in ["<=", ">=", "==", "!=", "=>", "->", ":="]:
		_position += 2
		_column += 2
		return Token.new(TokenType.OPERATOR, two_char, _line, start_col, start_pos)
	
	# Handle single-character operators and punctuation
	_position += 1
	_column += 1
	
	var type = TokenType.OPERATOR if c in ["+", "-", "*", "/", "^", "=", "<", ">", "!"] else TokenType.PUNCTUATION
	return Token.new(type, c, _line, start_col, start_pos)

func _current_char() -> String:
	if _position >= _length:
		return ""
	return _text[_position]

func _peek_next() -> String:
	if _position + 1 >= _length:
		return ""
	return _text[_position + 1] 