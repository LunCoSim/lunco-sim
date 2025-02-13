@tool
extends Node
class_name MOParser

var _text: String = ""
var _pos: int = 0
var _len: int = 0

# Essential Modelica keywords
const KEYWORDS = ["model", "connector", "package", "class", "record", "block", 
                 "type", "function", "extends", "parameter", "constant", "input", 
                 "output", "flow", "stream", "equation"]

func parse_file(file_path: String) -> Dictionary:
	var file = FileAccess.open(file_path, FileAccess.READ)
	if not file:
		push_error("Failed to open file: " + file_path)
		return {}
	
	_text = file.get_as_text()
	_pos = 0
	_len = _text.length()
	
	return parse_definition()

func parse_text(text: String) -> Dictionary:
	_text = text
	_pos = 0
	_len = text.length()
	return parse_definition()

func parse_definition() -> Dictionary:
	var result = {
		"type": "",           # model, connector, package, etc.
		"name": "",           # component name
		"extends": [],        # list of base classes with modifications
		"components": [],     # list of component declarations
		"equations": [],      # list of equations
		"annotations": {},    # annotations
		"within": "",         # within clause
	}
	
	_skip_whitespace_and_comments()
	
	# Parse within clause if present
	if _match_keyword("within"):
		result.within = _parse_qualified_name()
		_skip_until_semicolon()
	
	# Parse definition type and name
	result.type = _parse_definition_type()
	if result.type.is_empty():
		return result
	
	result.name = _parse_identifier()
	
	# Parse description string if present
	_skip_whitespace_and_comments()
	if _peek() == "\"":
		result.description = _parse_string()
	
	# Parse extends clauses
	while _match_keyword("extends"):
		var extends_info = _parse_extends()
		if not extends_info.is_empty():
			result.extends.append(extends_info)
	
	# Parse body until "end"
	while not _match_keyword("end"):
		_skip_whitespace_and_comments()
		
		if _pos >= _len:
			break
			
		# Parse equations section
		if _match_keyword("equation"):
			result.equations = _parse_equations()
			continue
			
		# Parse component declaration
		var comp = _parse_component()
		if not comp.is_empty():
			result.components.append(comp)
			continue
			
		# Skip unknown content until semicolon
		_skip_until_semicolon()
	
	# Skip to end of definition
	_skip_whitespace_and_comments()
	if _peek() == ";":
		_pos += 1
	
	return result

func _parse_definition_type() -> String:
	for keyword in KEYWORDS:
		if _match_keyword(keyword):
			return keyword
	return ""

func _parse_extends() -> Dictionary:
	var extends_info = {
		"base_class": "",
		"modifications": {}
	}
	
	extends_info.base_class = _parse_qualified_name()
	
	# Parse modifications if present
	if _peek() == "(":
		extends_info.modifications = _parse_modifications()
	
	_skip_until_semicolon()
	return extends_info

func _parse_component() -> Dictionary:
	_skip_whitespace_and_comments()
	
	var comp = {
		"type": "",
		"name": "",
		"modifications": {},
		"attributes": [],  # input, output, parameter, etc.
		"description": ""  # String description if present
	}
	
	# Parse attributes
	while true:
		var attr = _parse_identifier()
		if attr.is_empty():
			return {}  # Invalid component
		
		if attr in ["input", "output", "flow", "stream", "parameter", "constant"]:
			comp.attributes.append(attr)
			_skip_whitespace_and_comments()
		else:
			# Not an attribute, must be the type
			comp.type = attr
			break
	
	_skip_whitespace_and_comments()
	comp.name = _parse_identifier()
	if comp.name.is_empty():
		return {}  # Invalid component
	
	# Parse modifications or value assignment
	_skip_whitespace_and_comments()
	if _peek() == "(":
		comp.modifications = _parse_modifications()
	elif _peek() == "=":
		_pos += 1  # Skip =
		_skip_whitespace_and_comments()
		comp.modifications = {"value": _parse_value()}
	
	# Parse description string if present
	_skip_whitespace_and_comments()
	if _peek() == "\"":
		comp.description = _parse_string()
	
	_skip_until_semicolon()
	return comp

func _parse_equations() -> Array:
	var equations = []
	
	while not _match_keyword("end") and _pos < _len:
		_skip_whitespace_and_comments()
		
		var eq = ""
		var in_string = false
		var paren_count = 0
		
		# Parse until semicolon
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
			else:
				if c == "\"":
					in_string = false
			
			eq += c
			_pos += 1
		
		eq = eq.strip_edges()
		if not eq.is_empty():
			equations.append(eq)
	
	return equations

func _parse_modifications() -> Dictionary:
	var mods = {}
	
	_expect("(")
	
	while _pos < _len and _peek() != ")":
		var name = _parse_identifier()
		_expect("=")
		var value = _parse_value()
		mods[name] = value
		
		if _peek() == ",":
			_pos += 1  # Skip comma
	
	_expect(")")
	return mods

func _parse_qualified_name() -> String:
	var name = _parse_identifier()
	
	while _peek() == ".":
		_pos += 1  # Skip dot
		name += "." + _parse_identifier()
	
	return name

func _parse_identifier() -> String:
	_skip_whitespace_and_comments()
	
	var identifier = ""
	var c = _peek()
	
	# First character must be letter or underscore
	if not (c.is_valid_identifier() or c == "_"):
		return ""
	
	while _pos < _len:
		c = _peek()
		# Use is_valid_identifier() for letters and check digits separately
		if not (c.is_valid_identifier() or c == "_" or c in "0123456789" or c == "."):
			break
		identifier += c
		_pos += 1
	
	return identifier

func _parse_string() -> String:
	var string = ""
	_pos += 1  # Skip opening quote
	
	while _pos < _len and _peek() != "\"":
		string += _text[_pos]
		_pos += 1
	
	if _pos < _len:
		_pos += 1  # Skip closing quote
	
	return string

func _parse_value() -> String:
	_skip_whitespace_and_comments()
	
	var value = ""
	var in_string = false
	var paren_count = 0
	
	while _pos < _len:
		var c = _peek()
		
		if not in_string:
			if c == "\"":
				in_string = true
			elif c == "(":
				paren_count += 1
			elif c == ")":
				paren_count -= 1
				if paren_count < 0:
					break
			elif paren_count == 0 and (c == "," or c == ")" or c == ";"):
				break
		else:
			if c == "\"":
				in_string = false
		
		value += c
		_pos += 1
	
	return value.strip_edges()

func _match_keyword(keyword: String) -> bool:
	_skip_whitespace_and_comments()
	
	if _pos + keyword.length() > _len:
		return false
	
	var text = _text.substr(_pos, keyword.length())
	if text == keyword:
		var next_char = _peek(keyword.length())
		if next_char.is_empty() or not next_char.is_valid_identifier():
			_pos += keyword.length()
			return true
	
	return false

func _expect(char: String) -> void:
	_skip_whitespace_and_comments()
	
	if _pos < _len and _peek() == char:
		_pos += 1
	else:
		push_warning("Expected '" + char + "' at position " + str(_pos))

func _peek(offset: int = 0) -> String:
	if _pos + offset >= _len:
		return ""
	return _text[_pos + offset]

func _skip_whitespace_and_comments() -> void:
	while _pos < _len:
		# Skip whitespace
		while _pos < _len and _text[_pos].strip_edges().is_empty():
			_pos += 1
		
		if _pos + 1 >= _len:
			break
		
		# Skip single-line comments
		if _text[_pos] == "/" and _text[_pos + 1] == "/":
			while _pos < _len and _text[_pos] != "\n":
				_pos += 1
			continue
		
		# Skip multi-line comments
		if _text[_pos] == "/" and _text[_pos + 1] == "*":
			_pos += 2
			while _pos + 1 < _len:
				if _text[_pos] == "*" and _text[_pos + 1] == "/":
					_pos += 2
					break
				_pos += 1
			continue
		
		break

func _skip_until_semicolon() -> void:
	var in_string = false
	var paren_count = 0
	
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
		else:
			if c == "\"":
				in_string = false
		
		_pos += 1

func _skip_until(char: String) -> void:
	while _pos < _len and _peek() != char:
		_pos += 1
	if _peek() == char:
		_pos += 1
	
