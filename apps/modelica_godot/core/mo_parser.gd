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
		"extends": [],        # list of base classes with modifications
		"components": [],     # list of component declarations
		"equations": [],      # list of equations
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
	
	# Parse component declarations
	print("\nParsing component declarations...")
	while not _match_keyword("equation") and not _match_keyword("end") and _pos < _len:
		_skip_whitespace_and_comments()
		if _pos >= _len:
			break
			
		var component = _parse_component_declaration()
		if not component.is_empty():
			result.components.append(component)
			print("Found component: ", component.get("type", ""), " ", component.get("name", ""))
	
	# Parse equations if present
	if _match_keyword("equation"):
		print("\nParsing equations...")
		while not _match_keyword("end") and _pos < _len:
			_skip_whitespace_and_comments()
			if _pos >= _len:
				break
				
			var equation = _parse_equation()
			if not equation.is_empty():
				result.equations.append(equation)
				print("Found equation: ", equation)
	
	print("\nParsing complete")
	print("Components found: ", result.components.size())
	print("Equations found: ", result.equations.size())
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
	
	_skip_until_semicolon()
	return component

func _parse_equation() -> String:
	_skip_whitespace_and_comments()
	var equation = ""
	
	while _pos < _len:
		if _peek() == ";":
			_next()
			break
		equation += _next()
	
	return equation.strip_edges()

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
		if c.is_valid_identifier() or c.is_valid_integer():
			identifier += _next()
		else:
			break
	
	return identifier

func _parse_string() -> String:
	_skip_whitespace_and_comments()
	var string = ""
	
	if _peek() != "\"":
		return ""
	
	_next()  # Skip opening quote
	
	while _pos < _len:
		var c = _peek()
		if c == "\"":
			_next()  # Skip closing quote
			break
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
		
		break

func _skip_until_semicolon() -> void:
	while _pos < _len and _peek() != ";":
		_next()
	if _pos < _len:
		_next()  # Skip semicolon

func _match_keyword(keyword: String) -> bool:
	_skip_whitespace_and_comments()
	var start_pos = _pos
	
	for c in keyword:
		if _pos >= _len or _peek() != c:
			_pos = start_pos
			return false
		_next()
	
	# Check that the next character is not a valid identifier character
	if _pos < _len and _peek().is_valid_identifier():
		_pos = start_pos
		return false
	
	return true

func _peek(offset: int = 0) -> String:
	if _pos + offset >= _len:
		return ""
	return _text[_pos + offset]

func _next() -> String:
	if _pos >= _len:
		return ""
	var c = _text[_pos]
	_pos += 1
	return c
