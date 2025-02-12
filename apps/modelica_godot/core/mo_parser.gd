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
		"parameters": [],
		"variables": [],
		"components": [],
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
		
		if _match_keyword("model"):
			result["type"] = "model"
			_skip_whitespace_and_comments()
			result["name"] = _parse_identifier()
			break
		elif _match_keyword("connector"):
			result["type"] = "connector"
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
	
	# Parse component body
	while _pos < _len:
		_skip_whitespace_and_comments()
		
		# Check timeout
		if Time.get_unix_time_from_system() - start_time > TIMEOUT_SECONDS:
			push_warning("Parser timeout while parsing body in file: " + file_path)
			break
		
		if _match_keyword("end"):
			break
		
		# Parse parameters
		if _match_keyword("parameter"):
			var param = _parse_parameter()
			if not param.is_empty():
				result["parameters"].append(param)
			continue
		
		# Parse equations section
		if _match_keyword("equation"):
			result["equations"] = _parse_equations()
			continue
		
		# Parse components (connectors)
		var comp = _parse_component()
		if not comp.is_empty():
			result["components"].append(comp)
			continue
		
		# Skip to next semicolon if we can't parse this line
		while _pos < _len and _peek() != ";":
			_pos += 1
		if _peek() == ";":
			_pos += 1
	
	print("Parsed file: ", file_path)
	print("  Type: ", result["type"])
	print("  Name: ", result["name"])
	print("  Parameters: ", result["parameters"].size())
	print("  Components: ", result["components"].size())
	print("  Equations: ", result["equations"].size())
	
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
	_skip_whitespace_and_comments()
	var identifier = ""
	
	# First character must be a letter or underscore
	var c = _peek()
	if not (c.is_valid_identifier() or c == "_"):
		return ""
	
	identifier += c
	_pos += 1
	
	# Parse the rest of the identifier
	while _pos < _len:
		c = _peek()
		if c.is_valid_identifier() or c == "_" or c == "." or c in "0123456789":
			identifier += c
			_pos += 1
		else:
			break
	
	return identifier.strip_edges()

func _parse_string() -> String:
	var string = ""
	while _pos < _len and _text[_pos] != "\"":
		string += _text[_pos]
		_pos += 1
	_pos += 1  # Skip closing quote
	return string

func _match_keyword(keyword: String) -> bool:
	_skip_whitespace_and_comments()
	var start_pos = _pos
	var word = _peek_word()
	
	if word == keyword:
		_pos += keyword.length()
		return true
	
	return false

func _parse_annotation() -> Dictionary:
	var annotation = {}
	
	_skip_whitespace_and_comments()
	if _peek() != "(":
		return annotation
	
	_pos += 1  # Skip opening parenthesis
	var paren_count = 1
	
	while _pos < _len and paren_count > 0:
		var c = _peek()
		if c == "(":
			paren_count += 1
		elif c == ")":
			paren_count -= 1
		_pos += 1
	
	return annotation

func _skip_whitespace() -> void:
	while _pos < _len and _text[_pos] in " \t\n":
		_pos += 1

func _peek() -> String:
	if _pos < _len:
		return _text[_pos]
	return ""

func _parse_parameter() -> Dictionary:
	_skip_whitespace_and_comments()
	
	var param = {
		"type": "",
		"name": "",
		"value": "",
		"unit": "",
		"description": ""
	}
	
	# Parse type (Real, Integer, etc.)
	_skip_whitespace_and_comments()
	param.type = _parse_identifier()
	if param.type.is_empty():
		return {}
	
	# Parse name
	_skip_whitespace_and_comments()
	param.name = _parse_identifier()
	if param.name.is_empty():
		return {}
	
	# Parse default value if present
	_skip_whitespace_and_comments()
	if _peek() == "=":
		_pos += 1  # Skip =
		_skip_whitespace_and_comments()
		
		# Parse numeric value
		var value = ""
		var had_minus = false
		while _pos < _len:
			var c = _peek()
			if c == "-" and not had_minus:
				had_minus = true
				value += c
			elif c in "0123456789.":
				value += c
			else:
				break
			_pos += 1
		param.value = value.strip_edges()
		
		# Parse description string if present
		_skip_whitespace_and_comments()
		if _peek() == "\"":
			_pos += 1  # Skip opening quote
			param.description = _parse_string()
			
			# Try to extract unit from description
			var unit_start = param.description.find("(")
			var unit_end = param.description.find(")")
			if unit_start != -1 and unit_end != -1:
				param.unit = param.description.substr(unit_start + 1, unit_end - unit_start - 1)
	
	# Skip to end of declaration
	while _pos < _len and _peek() != ";":
		_pos += 1
	if _peek() == ";":
		_pos += 1
	
	return param

func _parse_equations() -> Array[String]:
	var equations: Array[String] = []
	
	# Skip the "equation" keyword as it's already been matched
	_skip_whitespace_and_comments()
	
	while _pos < _len:
		_skip_whitespace_and_comments()
		
		# Check for section end
		if _match_keyword("end") or _match_keyword("initial") or _match_keyword("algorithm"):
			break
		
		# Parse single equation
		var eq = _parse_equation()
		if not eq.is_empty():
			equations.append(eq)
		
		_skip_whitespace_and_comments()
	
	return equations

func _parse_equation() -> String:
	_skip_whitespace_and_comments()
	var equation = ""
	var paren_count = 0
	var in_string = false
	
	while _pos < _len:
		var c = _peek()
		
		if not in_string:
			if c == "\"":
				in_string = true
			elif c == "(":
				paren_count += 1
			elif c == ")":
				paren_count -= 1
			elif c == ";" and paren_count == 0:
				_pos += 1  # Skip semicolon
				break
			elif c == "=" and paren_count == 0:
				# Ensure we have spaces around equals sign for readability
				if not equation.ends_with(" "):
					equation += " "
				equation += "="
				if _pos + 1 < _len and _text[_pos + 1] != " ":
					equation += " "
				_pos += 1
				continue
		else:
			if c == "\"":
				in_string = false
		
		equation += c
		_pos += 1
		
		# Check for end of equation section
		if not in_string and paren_count == 0:
			var next_word = _peek_word()
			if next_word in ["end", "initial", "algorithm"]:
				break
	
	return equation.strip_edges()

func _peek_word() -> String:
	var start_pos = _pos
	var word = ""
	
	while start_pos < _len:
		var c = _text[start_pos]
		if c in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ":
			word += c
			start_pos += 1
		else:
			break
	
	return word

func _parse_component() -> Dictionary:
	_skip_whitespace_and_comments()
	
	var comp = {
		"type": "",
		"name": "",
		"description": "",
		"is_component": true,
		"modifiers": {},
		"annotation": {},
		"attributes": [],
		"value": "",
		"unit": ""
	}
	
	# Parse type
	comp.type = _parse_identifier()
	if comp.type.is_empty():
		return {}
	
	# Parse name
	_skip_whitespace_and_comments()
	comp.name = _parse_identifier()
	if comp.name.is_empty():
		return {}
	
	# Parse description string if present
	_skip_whitespace_and_comments()
	if _peek() == "\"":
		_pos += 1  # Skip opening quote
		comp.description = _parse_string()
	
	# Parse value if present (for equations or assignments)
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
		
		comp.value = _text.substr(value_start, _pos - value_start).strip_edges()
	
	# Parse annotation if present
	_skip_whitespace_and_comments()
	if _match_keyword("annotation"):
		comp.annotation = _parse_annotation()
	
	# Skip to end of declaration
	while _pos < _len and _peek() != ";":
		_pos += 1
	if _peek() == ";":
		_pos += 1
	
	return comp
	
