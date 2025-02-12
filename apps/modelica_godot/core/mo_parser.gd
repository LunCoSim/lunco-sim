class_name MOParser
extends RefCounted

enum TokenType {
	KEYWORD,
	IDENTIFIER,
	NUMBER,
	OPERATOR,
	STRING,
	COMMENT,
	NEWLINE,
	EOF
}

const KEYWORDS = [
	"model", "end", "parameter", "equation", "der", "input", "output",
	"flow", "stream", "connector", "package", "class", "type", "constant",
	"discrete", "Real", "Integer", "Boolean", "String"
]

var _text: String
var _pos: int = 0
var _line: int = 1
var _column: int = 1

func parse_file(path: String) -> Dictionary:
	print("Opening file: ", path)
	var file = FileAccess.open(path, FileAccess.READ)
	if file == null:
		push_error("Could not open file: " + path)
		return {}
		
	_text = file.get_as_text()
	print("File contents: ", _text.substr(0, 100) + "...")
	_pos = 0
	_line = 1
	_column = 1
	
	var model = _parse_model()
	print("Parsed model structure: ", model.keys())
	return model

func _parse_model() -> Dictionary:
	var model = {
		"type": "",  # model, connector, package, etc.
		"name": "",
		"description": "",
		"parameters": [],
		"variables": [],
		"equations": [],
		"annotations": {}
	}
	
	print("Starting model parsing")
	while _pos < _text.length():
		var token = _next_token()
		print("Token: ", token)
		
		match token.type:
			TokenType.KEYWORD:
				match token.value:
					"model", "connector", "package", "class":
						model.type = token.value
						var name_token = _next_token()
						if name_token.type == TokenType.IDENTIFIER:
							model.name = name_token.value
						print("Found model type: ", model.type, " name: ", model.name)
					
					"parameter":
						var param = _parse_parameter()
						model.parameters.append(param)
						print("Found parameter: ", param)
					
					"equation":
						var eq = _parse_equation()
						model.equations.append(eq)
						print("Found equation: ", eq)
					
					"end":
						print("Found end of model")
						break
			
			TokenType.COMMENT:
				if token.value.begins_with("\""):
					# Description string
					model.description = token.value.trim_prefix("\"").trim_suffix("\"")
					print("Found description: ", model.description)
			
			TokenType.NEWLINE:
				continue
				
			TokenType.EOF:
				print("Reached end of file")
				break
	
	return model

func _parse_parameter() -> Dictionary:
	var param = {
		"type": "",
		"name": "",
		"value": 0.0,
		"unit": "",
		"description": ""
	}
	
	while true:
		var token = _next_token()
		match token.type:
			TokenType.KEYWORD:
				param.type = token.value
			
			TokenType.IDENTIFIER:
				param.name = token.value
			
			TokenType.OPERATOR:
				if token.value == "=":
					var value_token = _next_token()
					if value_token.type == TokenType.NUMBER:
						param.value = float(value_token.value)
			
			TokenType.STRING:
				param.description = token.value
			
			TokenType.NEWLINE:
				break
	
	return param

func _parse_equation() -> String:
	var equation = ""
	
	while true:
		var token = _next_token()
		match token.type:
			TokenType.NEWLINE:
				break
			TokenType.COMMENT:
				continue
			_:
				equation += token.value + " "
	
	return equation.strip_edges()

func _next_token() -> Dictionary:
	_skip_whitespace()
	
	if _pos >= _text.length():
		return {"type": TokenType.EOF, "value": ""}
	
	var char = _text[_pos]
	
	# Handle comments
	if char == "/" and _pos + 1 < _text.length() and _text[_pos + 1] == "/":
		return _read_line_comment()
	
	# Handle string literals
	if char == "\"":
		return _read_string()
	
	# Handle numbers
	if _is_digit(char) or char == "-":
		return _read_number()
	
	# Handle identifiers and keywords
	if _is_letter(char):
		return _read_identifier()
	
	# Handle operators
	if "=+-*/()<>".contains(char):
		_pos += 1
		return {"type": TokenType.OPERATOR, "value": char}
	
	# Handle newlines
	if char == "\n":
		_line += 1
		_column = 1
		_pos += 1
		return {"type": TokenType.NEWLINE, "value": "\n"}
	
	# Skip unrecognized characters
	_pos += 1
	return _next_token()

func _skip_whitespace() -> void:
	while _pos < _text.length() and _text[_pos] in [" ", "\t", "\r"]:
		_pos += 1
		_column += 1

func _read_line_comment() -> Dictionary:
	var comment = ""
	_pos += 2  # Skip //
	
	while _pos < _text.length() and _text[_pos] != "\n":
		comment += _text[_pos]
		_pos += 1
	
	return {"type": TokenType.COMMENT, "value": comment.strip_edges()}

func _read_string() -> Dictionary:
	var string = ""
	_pos += 1  # Skip opening quote
	
	while _pos < _text.length() and _text[_pos] != "\"":
		string += _text[_pos]
		_pos += 1
	
	_pos += 1  # Skip closing quote
	return {"type": TokenType.STRING, "value": string}

func _read_number() -> Dictionary:
	var number = ""
	var has_decimal = false
	
	if _text[_pos] == "-":
		number += "-"
		_pos += 1
	
	while _pos < _text.length():
		var char = _text[_pos]
		if _is_digit(char):
			number += char
		elif char == "." and not has_decimal:
			number += char
			has_decimal = true
		else:
			break
		_pos += 1
	
	return {"type": TokenType.NUMBER, "value": number}

func _read_identifier() -> Dictionary:
	var identifier = ""
	
	while _pos < _text.length() and (_is_letter(_text[_pos]) or _is_digit(_text[_pos]) or _text[_pos] == "_"):
		identifier += _text[_pos]
		_pos += 1
	
	if identifier in KEYWORDS:
		return {"type": TokenType.KEYWORD, "value": identifier}
	else:
		return {"type": TokenType.IDENTIFIER, "value": identifier}

func _is_letter(c: String) -> bool:
	return (c >= "a" and c <= "z") or (c >= "A" and c <= "Z")

func _is_digit(c: String) -> bool:
	return c >= "0" and c <= "9" 