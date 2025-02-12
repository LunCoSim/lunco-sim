extends RefCounted
class_name MOParser

# Token types
const TOKEN_KEYWORD = 0
const TOKEN_IDENTIFIER = 1
const TOKEN_NUMBER = 2
const TOKEN_OPERATOR = 3
const TOKEN_STRING = 4
const TOKEN_COMMENT = 5
const TOKEN_NEWLINE = 6
const TOKEN_EOF = 7

# Keywords list
const KEYWORDS = [
	"model", "end", "parameter", "equation", "der", "input", "output",
	"flow", "stream", "connector", "package", "class", "type", "constant",
	"discrete", "Real", "Integer", "Boolean", "String", "extends",
	"block", "function", "record", "partial", "encapsulated", "within",
	"import", "public", "protected", "external", "annotation"
]

var _text: String = ""
var _pos: int = 0
var _line: int = 1
var _column: int = 1
var _package_path: String = ""
var _model_cache: Dictionary = {}

func _init() -> void:
	_reset()

func _reset() -> void:
	_text = ""
	_pos = 0
	_line = 1
	_column = 1
	_package_path = ""

func parse_file(path: String) -> Dictionary:
	if _model_cache.has(path):
		return _model_cache[path]
		
	print("Opening file: ", path)
	var file := FileAccess.open(path, FileAccess.READ)
	if not file:
		push_error("Could not open file: " + path)
		return {}
		
	_text = file.get_as_text()
	_reset()
	var model_data = _parse_model()
	_model_cache[path] = model_data
	return model_data

func _is_whitespace(c: String) -> bool:
	return c == " " or c == "\t" or c == "\n" or c == "\r"

func _skip_whitespace() -> void:
	while _pos < _text.length():
		var c := _text[_pos]
		if _is_whitespace(c):
			_pos += 1
			if c == "\n":
				_line += 1
				_column = 1
			else:
				_column += 1
		elif c == "/" and _pos + 1 < _text.length():
			if _text[_pos + 1] == "/":
				_skip_line_comment()
			elif _text[_pos + 1] == "*":
				_skip_block_comment()
			else:
				break
		else:
			break

func _skip_line_comment() -> void:
	while _pos < _text.length() and _text[_pos] != "\n":
		_pos += 1

func _skip_block_comment() -> void:
	_pos += 2  # Skip /*
	while _pos < _text.length():
		if _text[_pos] == "*" and _pos + 1 < _text.length() and _text[_pos + 1] == "/":
			_pos += 2
			break
		if _text[_pos] == "\n":
			_line += 1
			_column = 1
		else:
			_column += 1
		_pos += 1

func _skip_until(delimiter: String) -> void:
	while _pos < _text.length() and _text[_pos] != delimiter:
		if _text[_pos] == "\n":
			_line += 1
			_column = 1
		else:
			_column += 1
		_pos += 1
	if _pos < _text.length():
		_pos += 1  # Skip the delimiter

func _peek() -> String:
	return _text[_pos] if _pos < _text.length() else ""

func _peek_at(pos: int) -> String:
	return _text[pos] if pos < _text.length() else ""

func _parse_identifier() -> String:
	_skip_whitespace()
	var start := _pos
	
	while _pos < _text.length():
		var c := _text[_pos]
		if not (c.is_valid_identifier() or c == "."):
			break
		_pos += 1
	
	if _pos > start:
		return _text.substr(start, _pos - start)
	return ""

func _parse_string() -> String:
	var start := _pos
	
	while _pos < _text.length() and _text[_pos] != "\"":
		if _text[_pos] == "\\":
			_pos += 2  # Skip escape sequence
		else:
			_pos += 1
			
	var result := ""
	if _pos > start:
		result = _text.substr(start, _pos - start)
	
	if _pos < _text.length():
		_pos += 1  # Skip closing quote
		
	return result.strip_edges()

func _match_keyword(keyword: String) -> bool:
	_skip_whitespace()
	var start_pos := _pos
	var chars := keyword.length()
	
	if _pos + chars <= _text.length():
		if _text.substr(_pos, chars) == keyword:
			var next_char := _peek_at(_pos + chars)
			if next_char.is_empty() or not next_char.is_valid_identifier():
				_pos += chars
				return true
	
	_pos = start_pos
	return false

func _parse_model() -> Dictionary:
	var model := {
		"type": "",
		"name": "",
		"description": "",
		"parameters": [],
		"equations": [],
		"components": []
	}
	
	# Skip whitespace and comments at start
	_skip_whitespace()
	
	# Parse 'within' statement if present
	if _match_keyword("within"):
		model["within"] = _parse_identifier()
		_skip_until(";")
		_skip_whitespace()
	
	# Parse type declaration
	var type := _parse_type_declaration()
	if not type.is_empty():
		model["type"] = type.get("type", "")
		model["name"] = type.get("name", "")
		if type.has("description"):
			model["description"] = type.get("description", "")
		if type.has("extends"):
			model["extends"] = type.get("extends", "")
	
	# Parse body until 'end'
	while _pos < _text.length():
		_skip_whitespace()
		
		# Check for end of declaration
		if _match_keyword("end"):
			var end_name = _parse_identifier()
			if end_name != model["name"]:
				push_warning("End name '%s' does not match model name '%s'" % [end_name, model["name"]])
			break
			
		# Parse parameters
		if _match_keyword("parameter"):
			var param = _parse_parameter()
			if not param.is_empty():
				model["parameters"].append(param)
				
		# Parse equations
		elif _match_keyword("equation"):
			var eq = _parse_equation()
			if not eq.is_empty():
				model["equations"].append(eq)
				
		# Parse components
		elif _is_component_declaration():
			var comp = _parse_component()
			if not comp.is_empty():
				model["components"].append(comp)
		
		# Skip other content for now
		else:
			_skip_until(";")
	
	return model

func _parse_type_declaration() -> Dictionary:
	var result := {}
	
	# Skip any leading whitespace and comments
	_skip_whitespace()
	
	# Look for type keywords
	for type in ["package", "model", "connector", "block", "record", "class", "function"]:
		if _match_keyword(type):
			result["type"] = type
			_skip_whitespace()
			
			# Get the name
			var name := _parse_identifier()
			if not name.is_empty():
				result["name"] = name.strip_edges()
				
				# Look for description string
				_skip_whitespace()
				if _peek() == "\"":
					_pos += 1  # Skip opening quote
					result["description"] = _parse_string()
			
			# Handle extends clause
			_skip_whitespace()
			if _match_keyword("extends"):
				_skip_whitespace()
				var extends_name := _parse_identifier()
				if not extends_name.is_empty():
					result["extends"] = extends_name.strip_edges()
			
			# Skip to end of declaration
			_skip_until(";")
			return result
	
	return result

func _parse_parameter() -> Dictionary:
	var param := {}
	_skip_whitespace()
	
	# Get type
	var type = _parse_identifier()
	if not type.is_empty():
		param["type"] = type
		
		# Get name
		_skip_whitespace()
		var name = _parse_identifier()
		if not name.is_empty():
			param["name"] = name
			
			# Get default value if present
			_skip_whitespace()
			if _peek() == "=":
				_pos += 1
				_skip_whitespace()
				param["default"] = _parse_value()
	
	_skip_until(";")
	return param

func _parse_equation() -> Dictionary:
	var eq := {}
	_skip_whitespace()
	
	var start_pos = _pos
	_skip_until(";")
	
	if _pos > start_pos:
		eq["expression"] = _text.substr(start_pos, _pos - start_pos).strip_edges()
	
	return eq

func _is_component_declaration() -> bool:
	var start_pos = _pos
	var is_comp = false
	
	# Skip type name
	if not _parse_identifier().is_empty():
		_skip_whitespace()
		# Check for component name
		if not _parse_identifier().is_empty():
			is_comp = true
	
	_pos = start_pos
	return is_comp

func _parse_component() -> Dictionary:
	var comp := {}
	
	# Get type
	var type = _parse_identifier()
	if not type.is_empty():
		comp["type"] = type
		
		# Get name
		_skip_whitespace()
		var name = _parse_identifier()
		if not name.is_empty():
			comp["name"] = name
	
	_skip_until(";")
	return comp

func _parse_value() -> String:
	_skip_whitespace()
	var value = ""
	
	if _peek() == "\"":
		value = _parse_string()
	else:
		var start_pos = _pos
		while _pos < _text.length() and _text[_pos] != ";" and _text[_pos] != ",":
			_pos += 1
		if _pos > start_pos:
			value = _text.substr(start_pos, _pos - start_pos).strip_edges()
	
	return value 
