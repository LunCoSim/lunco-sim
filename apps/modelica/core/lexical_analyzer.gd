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
	print("\nStarting tokenization of text (length: %d)" % text.length())
	_text = text
	_length = text.length()
	_position = 0
	_line = 1
	_column = 1
	
	var tokens: Array[Token] = []
	var last_position = -1
	var stuck_count = 0
	
	while _position < _length:
		print("Position: %d, Line: %d, Column: %d, Current char: '%s'" % [_position, _line, _column, _current_char()])
		var token = _next_token()
		if token != null and token.type != TokenType.WHITESPACE:  # Skip whitespace tokens
			print("Generated token: %s" % token._to_string())
			tokens.append(token)
		
		# Check if we're stuck
		if last_position == _position:
			stuck_count += 1
			if stuck_count > 5:
				print("WARNING: Possibly stuck at position %d" % _position)
				print("Context: '%s'" % _text.substr(max(0, _position - 10), min(20, _length - _position)))
				break
		else:
			stuck_count = 0
		last_position = _position
	
	# Add EOF token
	var eof_token = Token.new(TokenType.EOF, "", _line, _column, _position)
	print("Generated EOF token: %s" % eof_token._to_string())
	tokens.append(eof_token)
	print("Tokenization complete. Generated %d tokens" % tokens.size())
	return tokens

func _next_token() -> Token:
	if _position >= _length:
		return null
	
	var c = _current_char()
	print("Processing character: '%s' at position %d" % [c, _position])
	
	# Skip whitespace
	if c.strip_edges().is_empty():
		print("Handling whitespace")
		return _handle_whitespace()
	
	# Handle comments
	if c == "/" and _peek_next() == "/":
		print("Handling line comment")
		return _handle_line_comment()
	if c == "/" and _peek_next() == "*":
		print("Handling block comment")
		return _handle_block_comment()
	
	# Handle identifiers and keywords
	if c.is_valid_identifier():
		print("Handling identifier")
		return _handle_identifier()
	
	# Handle numbers
	if c.is_valid_int() or (c == "-" and _peek_next().is_valid_int()):
		print("Handling number")
		return _handle_number()
	
	# Handle strings
	if c == "\"":
		print("Handling string")
		return _handle_string()
	
	# Handle operators and punctuation
	print("Handling operator/punctuation")
	return _handle_operator()

func _handle_whitespace() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	
	while _position < _length and _current_char().strip_edges().is_empty():
		if _current_char() == "\n":
			_line += 1
			_column = 1
		else:
			_column += 1
		value += _current_char()
		_position += 1
	
	return Token.new(TokenType.WHITESPACE, value, _line, start_col, start_pos)

func _handle_line_comment() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	
	# Skip //
	_position += 2
	_column += 2
	
	while _position < _length and _current_char() != "\n":
		value += _current_char()
		_position += 1
		_column += 1
	
	return Token.new(TokenType.COMMENT, value, _line, start_col, start_pos)

func _handle_block_comment() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	
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
		
		value += _current_char()
		_position += 1
	
	return Token.new(TokenType.COMMENT, value, _line, start_col, start_pos)

func _handle_identifier() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	
	while _position < _length and (_current_char().is_valid_identifier() or _current_char().is_valid_int()):
		value += _current_char()
		_position += 1
		_column += 1
	
	# Check if it's a keyword
	if value in _keywords:
		return Token.new(TokenType.KEYWORD, value, _line, start_col, start_pos)
	
	return Token.new(TokenType.IDENTIFIER, value, _line, start_col, start_pos)

func _handle_number() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	var has_decimal = false
	
	# Handle negative numbers
	if _current_char() == "-":
		value += _current_char()
		_position += 1
		_column += 1
	
	while _position < _length:
		var c = _current_char()
		if c.is_valid_int():
			value += c
			_position += 1
			_column += 1
		elif c == "." and not has_decimal:
			value += c
			has_decimal = true
			_position += 1
			_column += 1
		elif c == "e" or c == "E":  # Handle scientific notation
			value += c
			_position += 1
			_column += 1
			if _position < _length and (_current_char() == "+" or _current_char() == "-"):
				value += _current_char()
				_position += 1
				_column += 1
		else:
			break
	
	return Token.new(TokenType.NUMBER, value, _line, start_col, start_pos)

func _handle_string() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	
	# Skip opening quote
	_position += 1
	_column += 1
	
	while _position < _length and _current_char() != "\"":
		if _current_char() == "\n":
			_line += 1
			_column = 1
		else:
			_column += 1
		value += _current_char()
		_position += 1
	
	# Skip closing quote
	if _position < _length:
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