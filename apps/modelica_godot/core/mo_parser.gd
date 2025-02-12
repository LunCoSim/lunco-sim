@tool
class_name MOParser
extends Node

var _text: String = ""
var _pos: int = 0
var _len: int = 0
const TIMEOUT_SECONDS = 5.0

func _init() -> void:
	_text = ""
	_pos = 0
	_len = 0

func parse_file(file_path: String) -> Dictionary:
	print("Parsing file: ", file_path)
	
	# Read file content
	var file = FileAccess.open(file_path, FileAccess.READ)
	if not file:
		push_error("Failed to open file: " + file_path)
		return {}
	
	_text = file.get_as_text()
	_pos = 0
	_len = _text.length()
	
	var start_time = Time.get_unix_time_from_system()
	
	# Initialize with basic info from filename
	var result = {
		"type": "unknown",
		"name": file_path.get_file().get_basename(),
		"path": file_path,
		"components": [],
		"variables": [],
		"equations": []
	}
	
	while _pos < _len:
		# Check timeout
		if Time.get_unix_time_from_system() - start_time > TIMEOUT_SECONDS:
			push_warning("Parser timeout for file: " + file_path)
			return result
		
		_skip_whitespace()
		
		# Try to parse type and name
		if _match_keyword("model"):
			result["type"] = "model"
			_skip_whitespace()
			var name = _parse_identifier()
			if not name.is_empty():
				result["name"] = name
		elif _match_keyword("package"):
			result["type"] = "package"
			_skip_whitespace()
			var name = _parse_identifier()
			if not name.is_empty():
				result["name"] = name
		elif _match_keyword("connector"):
			result["type"] = "connector"
			_skip_whitespace()
			var name = _parse_identifier()
			if not name.is_empty():
				result["name"] = name
		elif _match_keyword("block"):
			result["type"] = "block"
			_skip_whitespace()
			var name = _parse_identifier()
			if not name.is_empty():
				result["name"] = name
		else:
			# Skip to next line if we don't recognize the content
			while _pos < _len and _text[_pos] != '\n':
				_pos += 1
			_pos += 1
	
	print("Parsed file: ", file_path)
	print("  Type: ", result["type"])
	print("  Name: ", result["name"])
	print("  Components: ", result["components"].size())
	print("  Variables: ", result["variables"].size())
	print("  Equations: ", result["equations"].size())
	
	return result

func _parse_modifiers() -> Dictionary:
	var modifiers := {}
	
	if _peek() != "(":
		return modifiers
		
	_pos += 1  # Skip opening parenthesis
	
	while _pos < _len:
		_skip_whitespace()
		
		# Check for end of modifiers
		if _peek() == ")":
			_pos += 1
			break
			
		# Parse modifier name
		var name = _parse_identifier()
		if name.is_empty():
			break
			
		# Look for equals sign
		_skip_whitespace()
		if _peek() == "=":
			_pos += 1  # Skip =
			_skip_whitespace()
			
			# Parse value
			var value = ""
			var start_pos = _pos
			
			# Handle quoted strings
			if _peek() == "\"":
				_pos += 1  # Skip opening quote
				value = _parse_string()
			else:
				# Parse until comma or closing parenthesis
				while _pos < _len and _peek() not in [",", ")"]:
					_pos += 1
				value = _text.substr(start_pos, _pos - start_pos).strip_edges()
			
			modifiers[name] = value
		
		# Skip comma if present
		_skip_whitespace()
		if _peek() == ",":
			_pos += 1
	
	return modifiers

func _parse_variable_declaration() -> Dictionary:
	var var_decl := {
		"type": "",
		"name": "",
		"description": "",
		"flow": false,
		"input": false,
		"output": false,
		"annotation": {},
		"modifiers": {}
	}
	
	_skip_whitespace()
	var start_pos = _pos
	
	# Check for flow/input/output prefix
	if _match_keyword("flow"):
		var_decl["flow"] = true
	elif _match_keyword("input"):
		var_decl["input"] = true
	elif _match_keyword("output"):
		var_decl["output"] = true
	
	# Parse type
	_skip_whitespace()
	var_decl["type"] = _parse_identifier()
	if var_decl["type"].is_empty():
		_pos = start_pos
		return {}
	
	# Parse modifiers if present
	_skip_whitespace()
	if _peek() == "(":
		var_decl["modifiers"] = _parse_modifiers()
	
	# Parse name
	_skip_whitespace()
	var_decl["name"] = _parse_identifier()
	if var_decl["name"].is_empty():
		_pos = start_pos
		return {}
	
	# Look for description string
	_skip_whitespace()
	if _peek() == "\"":
		_pos += 1  # Skip opening quote
		var_decl["description"] = _parse_string()
	
	# Look for annotation
	_skip_whitespace()
	if _match_keyword("annotation"):
		var_decl["annotation"] = _parse_annotation()
	
	# Skip to end of declaration
	while _pos < _len and _peek() != ";":
		_pos += 1
	if _peek() == ";":
		_pos += 1
	
	return var_decl

func _parse_identifier() -> String:
	var identifier = ""
	while _pos < _len and _text[_pos] in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_":
		identifier += _text[_pos]
		_pos += 1
	return identifier

func _parse_string() -> String:
	var string = ""
	while _pos < _len and _text[_pos] != "\"":
		string += _text[_pos]
		_pos += 1
	_pos += 1  # Skip closing quote
	return string

func _match_keyword(keyword: String) -> bool:
	var start_pos = _pos
	while _pos < _len and _text[_pos] in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_":
		_pos += 1
	return _text.substr(start_pos, _pos - start_pos) == keyword

func _parse_annotation() -> Dictionary:
	var annotation = {}
	_skip_whitespace()
	if _peek() == "{":
		_pos += 1  # Skip opening brace
		while _pos < _len and _peek() != "}":
			_skip_whitespace()
			var key = _parse_identifier()
			_skip_whitespace()
			if _peek() == "=":
				_pos += 1  # Skip =
				_skip_whitespace()
				var value = ""
				if _peek() == "\"":
					_pos += 1  # Skip opening quote
					value = _parse_string()
				else:
					while _pos < _len and _peek() not in [",", "}"] and _peek() != ";":
						_pos += 1
					value = _text.substr(_pos - _pos, _pos - _pos).strip_edges()
				annotation[key] = value
			_skip_whitespace()
			if _peek() == ",":
				_pos += 1
		_pos += 1  # Skip closing brace
	return annotation

func _skip_whitespace() -> void:
	while _pos < _len and _text[_pos] in " \t\n":
		_pos += 1

func _peek() -> String:
	if _pos < _len:
		return _text[_pos]
	return ""
	
