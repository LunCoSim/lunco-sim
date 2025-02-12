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
		"equations": [],
		"description": "",
		"within": ""
	}
	
	# Skip any initial whitespace and comments
	_skip_whitespace_and_comments()
	
	# Parse within statement if present
	if _match_keyword("within"):
		_skip_whitespace_and_comments()
		var within_path = ""
		while _pos < _len and _peek() != ";":
			within_path += _text[_pos]
			_pos += 1
		result["within"] = within_path.strip_edges()
		_pos += 1  # Skip semicolon
		_skip_whitespace_and_comments()
	
	# Parse type and name
	while _pos < _len:
		# Check timeout
		if Time.get_unix_time_from_system() - start_time > TIMEOUT_SECONDS:
			push_warning("Parser timeout for file: " + file_path)
			return result
		
		if _match_keyword("package"):
			result["type"] = "package"
			_skip_whitespace_and_comments()
			result["name"] = _parse_identifier()
			break
		elif _match_keyword("model"):
			result["type"] = "model"
			_skip_whitespace_and_comments()
			result["name"] = _parse_identifier()
			break
		elif _match_keyword("connector"):
			result["type"] = "connector"
			_skip_whitespace_and_comments()
			result["name"] = _parse_identifier()
			break
		elif _match_keyword("block"):
			result["type"] = "block"
			_skip_whitespace_and_comments()
			result["name"] = _parse_identifier()
			break
		elif _match_keyword("function"):
			result["type"] = "function"
			_skip_whitespace_and_comments()
			result["name"] = _parse_identifier()
			break
		elif _match_keyword("type"):
			result["type"] = "type"
			_skip_whitespace_and_comments()
			result["name"] = _parse_identifier()
			break
		elif _match_keyword("record"):
			result["type"] = "record"
			_skip_whitespace_and_comments()
			result["name"] = _parse_identifier()
			break
		else:
			# Skip this character if we don't recognize it
			_pos += 1
			_skip_whitespace_and_comments()
	
	# Try to parse description string
	_skip_whitespace_and_comments()
	if _peek() == "\"":
		_pos += 1  # Skip opening quote
		result["description"] = _parse_string()
	
	# Parse extends clause if present
	_skip_whitespace_and_comments()
	if _match_keyword("extends"):
		_skip_whitespace_and_comments()
		var extends_name = _parse_identifier()
		# Skip any modifiers in parentheses
		if _peek() == "(":
			var paren_count = 1
			_pos += 1
			while _pos < _len and paren_count > 0:
				if _peek() == "(":
					paren_count += 1
				elif _peek() == ")":
					paren_count -= 1
				_pos += 1
		# Skip semicolon
		while _pos < _len and _peek() != ";":
			_pos += 1
		if _peek() == ";":
			_pos += 1
	
	# Parse components and variables
	while _pos < _len:
		_skip_whitespace_and_comments()
		
		# Check timeout
		if Time.get_unix_time_from_system() - start_time > TIMEOUT_SECONDS:
			push_warning("Parser timeout while parsing components in file: " + file_path)
			break
		
		if _match_keyword("end"):
			break
			
		# Try to parse a declaration
		var decl = _parse_declaration()
		if not decl.is_empty():
			# Determine if this is a component or variable
			var is_component = false
			
			# Components are typically:
			# 1. Models, blocks, or connectors
			# 2. Have qualified names (containing dots)
			# 3. Don't have constant/parameter attributes
			if decl["type"] in ["model", "block", "connector"] or \
			   (decl["type"].contains(".") and \
				not "constant" in decl["attributes"] and \
				not "parameter" in decl["attributes"]):
				is_component = true
			
			if is_component:
				result["components"].append(decl)
			else:
				result["variables"].append(decl)
		else:
			# Skip to next semicolon if we can't parse this line
			while _pos < _len and _peek() != ";":
				_pos += 1
			if _peek() == ";":
				_pos += 1
	
	print("Parsed file: ", file_path)
	print("  Type: ", result["type"])
	print("  Name: ", result["name"])
	print("  Within: ", result["within"])
	print("  Description: ", result["description"])
	print("  Components: ", result["components"].size())
	print("  Variables: ", result["variables"].size())
	
	return result

func _parse_declaration() -> Dictionary:
	var start_pos = _pos
	var decl = {
		"type": "",
		"name": "",
		"description": "",
		"is_component": false,
		"modifiers": {},
		"annotation": {},
		"attributes": [],
		"value": "",
		"unit": ""
	}
	
	# Parse attributes (input, output, flow, stream, etc.)
	while true:
		_skip_whitespace_and_comments()
		var attr = _parse_identifier()
		if attr in ["input", "output", "flow", "stream", "discrete", "parameter", "constant", "final"]:
			decl["attributes"].append(attr)
			_skip_whitespace_and_comments()
		else:
			# Restore position to parse type
			_pos = start_pos + (_pos - start_pos) - attr.length()
			break
	
	# Parse type
	_skip_whitespace_and_comments()
	decl["type"] = _parse_identifier()
	if decl["type"].is_empty():
		_pos = start_pos
		return {}
	
	# Parse unit if present
	_skip_whitespace_and_comments()
	if _peek() == "(":
		_pos += 1  # Skip opening parenthesis
		_skip_whitespace_and_comments()
		if _match_keyword("final"):
			_skip_whitespace_and_comments()
			if _match_keyword("unit"):
				_skip_whitespace_and_comments()
				if _peek() == "=":
					_pos += 1  # Skip =
					_skip_whitespace_and_comments()
					if _peek() == "\"":
						_pos += 1  # Skip opening quote
						decl["unit"] = _parse_string()
		# Skip to closing parenthesis
		while _pos < _len and _peek() != ")":
			_pos += 1
		if _peek() == ")":
			_pos += 1
	
	# Parse name
	_skip_whitespace_and_comments()
	decl["name"] = _parse_identifier()
	if decl["name"].is_empty():
		_pos = start_pos
		return {}
	
	# Parse array dimensions if present
	_skip_whitespace_and_comments()
	if _peek() == "[":
		while _pos < _len and _peek() != "]":
			_pos += 1
		if _peek() == "]":
			_pos += 1
	
	# Parse value if present
	_skip_whitespace_and_comments()
	if _peek() == "=":
		_pos += 1  # Skip =
		_skip_whitespace_and_comments()
		var value_start = _pos
		var paren_count = 0
		var in_string = false
		
		# Parse until semicolon or annotation, handling parentheses and strings
		while _pos < _len:
			var c = _peek()
			if not in_string:
				if c == "\"":
					in_string = true
				elif c == "(" or c == "[":
					paren_count += 1
				elif c == ")" or c == "]":
					paren_count -= 1
				elif paren_count == 0 and (c == ";" or _text.substr(_pos, 10) == "annotation"):
					break
			else:
				if c == "\"":
					in_string = false
			_pos += 1
		
		decl["value"] = _text.substr(value_start, _pos - value_start).strip_edges()
	
	# Parse description string if present
	_skip_whitespace_and_comments()
	if _peek() == "\"":
		_pos += 1  # Skip opening quote
		decl["description"] = _parse_string()
	
	# Parse annotation if present
	_skip_whitespace_and_comments()
	if _match_keyword("annotation"):
		decl["annotation"] = _parse_annotation()
	
	# Skip to end of declaration
	while _pos < _len and _peek() != ";":
		_pos += 1
	if _peek() == ";":
		_pos += 1
	
	return decl

func _skip_whitespace_and_comments() -> void:
	while _pos < _len:
		_skip_whitespace()
		
		# Check for single-line comment
		if _pos + 1 < _len and _text[_pos] == "/" and _text[_pos + 1] == "/":
			while _pos < _len and _text[_pos] != '\n':
				_pos += 1
			continue
			
		# Check for multi-line comment
		if _pos + 1 < _len and _text[_pos] == "/" and _text[_pos + 1] == "*":
			_pos += 2  # Skip /*
			while _pos + 1 < _len:
				if _text[_pos] == "*" and _text[_pos + 1] == "/":
					_pos += 2  # Skip */
					break
				_pos += 1
			continue
			
		break

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

func _parse_identifier() -> String:
	var identifier = ""
	# Allow dots in identifiers for qualified names
	while _pos < _len and _text[_pos] in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_." and not (_text[_pos] == "." and identifier.is_empty()):
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
	
