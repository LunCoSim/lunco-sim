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
	print("\n=== Parsing Modelica File ===")
	print("File: ", file_path)
	
	var file = FileAccess.open(file_path, FileAccess.READ)
	if not file:
		push_error("Failed to open file: " + file_path)
		return {}
	
	_text = file.get_as_text()
	_pos = 0
	_len = _text.length()
	
	return parse_definition()

func parse_text(text: String) -> Dictionary:
	print("\n=== Parsing Modelica Text ===")
	_text = text
	_pos = 0
	_len = text.length()
	return parse_definition()

func parse_definition() -> Dictionary:
	print("\nParsing definition...")
	var result = {
		"type": "",           # model, connector, package, etc.
		"name": "",           # component name
		"description": "",    # description string
		"extends": [],        # list of base classes with modifications
		"components": [],     # list of component declarations
		"equations": [],      # list of equations
		"initial_equations": [], # list of initial equations
		"annotations": {},    # annotations
		"within": "",         # within clause
		"imports": [],        # list of import statements
	}
	
	_skip_whitespace_and_comments()
	
	# Parse within clause if present
	if _match_keyword("within"):
		print("Found 'within' clause")
		result.within = _parse_qualified_name()
		print("Within: ", result.within)
		_skip_until_semicolon()
	
	# Parse import statements
	while _match_keyword("import"):
		print("Found 'import' statement")
		var import_info = _parse_import()
		if not import_info.is_empty():
			result.imports.append(import_info)
			print("Import: ", import_info)
	
	# Parse definition type and name
	result.type = _parse_definition_type()
	if result.type.is_empty():
		print("No definition type found")
		return result
	
	print("Definition type: ", result.type)
	result.name = _parse_identifier()
	print("Definition name: ", result.name)
	
	# Parse description string if present
	_skip_whitespace_and_comments()
	if _peek() == "\"":
		result.description = _parse_string()
		print("Description: ", result.description)
	
	# Parse extends clauses
	while _match_keyword("extends"):
		print("Found 'extends' clause")
		var extends_info = _parse_extends()
		if not extends_info.is_empty():
			result.extends.append(extends_info)
			print("Extends: ", extends_info)
	
	# Parse components and equations
	while _pos < _len:
		_skip_whitespace_and_comments()
		
		# Check for end of definition
		if _match_keyword("end"):
			print("Found 'end' keyword")
			_skip_whitespace_and_comments()
			var end_name = _parse_identifier()
			if end_name != result.name:
				push_error("End name '" + end_name + "' does not match definition name '" + result.name + "'")
			_skip_until_semicolon()
			break
			
		# Check for equation section
		if _match_keyword("equation"):
			print("Found 'equation' section")
			while _pos < _len:
				_skip_whitespace_and_comments()
				if _match_keyword("end") or _match_keyword("annotation"):
					break
				var equation = _parse_equation()
				if not equation.is_empty():
					result.equations.append(equation)
			continue
		
		# Parse component declaration
		var component = _parse_component_declaration()
		if not component.is_empty():
			result.components.append(component)
			print("Component: ", component)
			_skip_until_semicolon()
			continue
		
		# If we get here, we couldn't parse anything meaningful
		if _pos < _len:
			push_error("Unexpected character at position " + str(_pos) + ": '" + _peek() + "'")
			_next()  # Skip the problematic character to avoid infinite loop
	
	return result

func _parse_qualified_name() -> String:
	_skip_whitespace_and_comments()
	var name = ""
	while _pos < _len:
		var c = _peek()
		if c.is_valid_identifier() or c == ".":
			name += _next()
		else:
			break
	return name.strip_edges()

func _parse_import() -> Dictionary:
	_skip_whitespace_and_comments()
	var import_info = {}
	
	# Parse import name
	var name = _parse_qualified_name()
	if name.is_empty():
		return {}
	
	import_info["name"] = name
	
	# Check for alias
	_skip_whitespace_and_comments()
	if _peek() == "=":
		_next()  # Skip =
		_skip_whitespace_and_comments()
		import_info["alias"] = _parse_identifier()
	
	_skip_until_semicolon()
	return import_info

func _parse_extends() -> Dictionary:
	_skip_whitespace_and_comments()
	var extends_info = {}
	
	# Parse base class name
	extends_info["base_class"] = _parse_qualified_name()
	
	# Parse modifications if any
	_skip_whitespace_and_comments()
	if _peek() == "(":
		extends_info["modifications"] = _parse_modifications()
	
	_skip_until_semicolon()
	return extends_info

func _parse_component_declaration() -> Dictionary:
	_skip_whitespace_and_comments()
	var component = {}
	
	# Parse type
	component["type"] = _parse_qualified_name()
	if component["type"].is_empty():
		return {}
	
	_skip_whitespace_and_comments()
	
	# Parse name
	component["name"] = _parse_identifier()
	if component["name"].is_empty():
		return {}
	
	# Parse modifications if any
	_skip_whitespace_and_comments()
	if _peek() == "(":
		component["modifications"] = _parse_modifications()
	
	# Parse description string if present
	_skip_whitespace_and_comments()
	if _peek() == "\"":
		component["description"] = _parse_string()
	
	# Parse component annotation if present
	_skip_whitespace_and_comments()
	if _match_keyword("annotation"):
		print("Found component annotation for: ", component["name"])
		component["annotation"] = _parse_model_annotation()
	
	_skip_until_semicolon()
	return component

func _parse_equation() -> String:
	_skip_whitespace_and_comments()
	var equation = ""
	
	while _pos < _len:
		# Check for end of equation
		if _peek() == ";":
			_next()  # Skip semicolon
			break
			
		# Check for end of equation section
		var save_pos = _pos
		_skip_whitespace_and_comments()
		if _match_keyword("end") or _match_keyword("annotation"):
			_pos = save_pos
			break
		_pos = save_pos
		
		equation += _next()
	
	equation = equation.strip_edges()
	if not equation.is_empty():
		print("Parsed equation: ", equation)
	return equation

func _parse_definition_type() -> String:
	_skip_whitespace_and_comments()
	for keyword in KEYWORDS:
		if _match_keyword(keyword):
			return keyword
	return ""

func _parse_identifier() -> String:
	_skip_whitespace_and_comments()
	var identifier = ""
	
	if _pos >= _len:
		return ""
	
	# First character must be a letter
	var first = _peek()
	if not first.is_valid_identifier():
		return ""
	
	identifier += _next()
	
	# Rest can be letters, digits or underscore
	while _pos < _len:
		var c = _peek()
		if not (c.is_valid_identifier() or c == "_" or c.is_valid_int()):
			break
		identifier += _next()
	
	return identifier

func _parse_string() -> String:
	if _peek() != "\"":
		return ""
		
	_next()  # Skip opening quote
	var string = ""
	
	while _pos < _len:
		var c = _peek()
		if c == "\"":
			_next()  # Skip closing quote
			break
		if c == "\\":
			_next()  # Skip backslash
			if _pos < _len:
				string += _next()  # Add escaped character
		else:
			string += _next()
	
	return string

func _parse_modifications() -> Dictionary:
	var modifications = {}
	
	_next()  # Skip opening parenthesis
	
	while _pos < _len:
		_skip_whitespace_and_comments()
		if _peek() == ")":
			_next()  # Skip closing parenthesis
			break
			
		var name = _parse_identifier()
		if name.is_empty():
			break
			
		_skip_whitespace_and_comments()
		if _peek() == "=":
			_next()  # Skip =
			_skip_whitespace_and_comments()
			modifications[name] = _parse_value()
		
		_skip_whitespace_and_comments()
		if _peek() == ",":
			_next()  # Skip comma
	
	return modifications

func _parse_value() -> String:
	_skip_whitespace_and_comments()
	var value = ""
	
	while _pos < _len:
		var c = _peek()
		if c in [",", ")", ";"]:
			break
		value += _next()
	
	return value.strip_edges()

func _skip_whitespace_and_comments() -> void:
	while _pos < _len:
		var c = _peek()
		
		# Skip whitespace
		if c.strip_edges().is_empty():
			_next()
			continue
		
		# Skip single-line comments
		if c == "/" and _peek(1) == "/":
			while _pos < _len and _peek() != "\n":
				_next()
			if _pos < _len:
				_next()  # Skip the newline
			continue
		
		# Skip multi-line comments
		if c == "/" and _peek(1) == "*":
			_next()  # Skip /
			_next()  # Skip *
			while _pos < _len:
				if _peek() == "*" and _peek(1) == "/":
					_next()  # Skip *
					_next()  # Skip /
					break
				_next()
			continue
			
		# No more whitespace or comments
		break

func _skip_until_semicolon() -> void:
	while _pos < _len and _peek() != ";":
		_next()
	if _pos < _len:
		_next()  # Skip the semicolon

func _peek(offset: int = 0) -> String:
	var pos = _pos + offset
	if pos >= _len:
		return ""
	return _text[pos]

func _next() -> String:
	if _pos >= _len:
		return ""
	var c = _text[_pos]
	_pos += 1
	print("Current position: ", _pos, ", Current char: '", c, "'")
	return c

func _match_keyword(keyword: String) -> bool:
	_skip_whitespace_and_comments()
	var save_pos = _pos
	
	# Try to match the keyword
	for c in keyword:
		if _pos >= _len or _peek() != c:
			_pos = save_pos
			return false
		_next()
	
	# Check that the next character is not a letter, digit or underscore
	# (to ensure we matched a complete keyword)
	if _pos < _len:
		var next_char = _peek()
		if next_char.is_valid_identifier() or next_char == "_" or next_char.is_valid_int():
			_pos = save_pos
			return false
	
	return true

func _parse_model_annotation() -> Dictionary:
	print("Parsing model annotation...")
	var annotation = {}
	var parentheses_count = 0
	var content = ""
	
	_skip_whitespace_and_comments()
	if _peek() != "(":
		return {}
		
	_next()  # Skip opening parenthesis
	parentheses_count += 1
	
	while _pos < _len and parentheses_count > 0:
		var c = _peek()
		
		if c == "(":
			parentheses_count += 1
		elif c == ")":
			parentheses_count -= 1
		
		content += _next()
	
	_skip_whitespace_and_comments()
	if _peek() == ";":
		_next()  # Skip semicolon
	
	if not content.is_empty():
		annotation["content"] = content
	
	return annotation
