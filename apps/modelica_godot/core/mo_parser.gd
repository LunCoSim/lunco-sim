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
		"parameters": [],     # list of parameters
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
	
	# Parse component declarations and equations
	print("\nParsing component declarations...")
	var in_equation_section = false
	var in_initial_equation_section = false
	
	while _pos < _len:
		_skip_whitespace_and_comments()
		
		if _pos >= _len:
			break
			
		var current_char = _peek()
		
		if current_char == "e":
			if _match_keyword("end"):
				print("Found 'end' keyword")
				# Parse end name and verify it matches
				_skip_whitespace()
				var end_name = _parse_identifier()
				print("Found end name: ", end_name)
				if end_name != result.name:
					push_warning("End name does not match definition name")
				_skip_until_semicolon()
				print("Found end of definition")
				break
			elif _match_keyword("equation"):
				print("Found equation section")
				in_equation_section = true
				in_initial_equation_section = false
				continue
		elif current_char == "i" and _match_keyword("initial"):
			if _match_keyword("equation"):
				print("Found initial equation section")
				in_initial_equation_section = true
				in_equation_section = false
				continue
		elif current_char == "a" and _match_keyword("annotation"):
			print("\nParsing model annotation...")
			var annotation = _parse_annotation()
			if not annotation.is_empty():
				result.annotations = annotation
				print("Found model annotation: ", annotation)
			continue
			
		if in_equation_section:
			var equation = _parse_equation()
			if not equation.is_empty():
				print("Found equation: ", equation)
				result.equations.append(equation)
		elif in_initial_equation_section:
			var equation = _parse_equation()
			if not equation.is_empty():
				print("Found initial equation: ", equation)
				result.initial_equations.append(equation)
		else:
			var component = _parse_component()
			if not component.is_empty():
				print("Found component: ", component)
				if component.get("type", "") == "parameter":
					result.parameters.append(component)
				else:
					result.components.append(component)
	
	return result

func _parse_qualified_name() -> String:
	var name = _parse_identifier()
	
	while _pos < _len:
		_skip_whitespace_and_comments()
		if _peek() != ".":
			break
		_next()  # Skip dot
		name += "."
		name += _parse_identifier()
	
	return name

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

func _parse_component() -> Dictionary:
	print("\nParsing component at position: ", _pos, ", Current char: ", _peek())
	var component = {
		"type": "",
		"name": "",
		"modifications": {},
		"annotations": {},
		"is_parameter": false,
		"attributes": [],
		"description": "",
		"value": "",
		"default": ""
	}
	
	# Check for parameter keyword
	if _match_keyword("parameter"):
		component.is_parameter = true
		component.type = "parameter"
		_skip_whitespace()
	
	# Parse type name
	var type_name = _parse_type_name()
	if type_name.is_empty():
		print("No type found, returning empty component")
		return {}
	
	if not component.is_parameter:
		component.type = type_name
	
	print("Checking word: ", type_name)
	
	# Parse component name
	_skip_whitespace()
	var name = _parse_identifier()
	if name.is_empty():
		print("No name found, returning empty component")
		return {}
	
	component.name = name
	print("Found name: ", name)
	
	# Parse modifications
	_skip_whitespace()
	if _peek() == "(":
		print("Found modifications")
		component.modifications = _parse_modifications()
		print("Finished parsing modifications: ", component.modifications)
	
	# Parse description string
	_skip_whitespace()
	if _peek() == "\"":
		component.description = _parse_string()
	
	# Look for semicolon
	_skip_whitespace()
	if _peek() == ";":
		_advance()  # Skip semicolon
		print("Found end of component declaration")
	else:
		push_warning("Component declaration did not end with semicolon")
	
	print("Finished parsing component: ", component)
	return component

func _parse_annotation() -> Dictionary:
	print("Parsing model annotation")
	var result = {}
	
	_skip_whitespace()
	if _peek() != "(":
		return result
	
	_advance()  # Skip opening parenthesis
	
	# Parse annotation content
	var content = ""
	var parentheses_count = 1
	
	while _pos < _len and parentheses_count > 0:
		var char = _peek()
		
		if char == "(":
			parentheses_count += 1
		elif char == ")":
			parentheses_count -= 1
			
		if parentheses_count > 0:
			content += char
			
		_advance()
	
	print("Parsed annotation content: ", content)
	
	# Parse experiment settings
	if content.begins_with("experiment"):
		var experiment = {}
		
		# Extract values using regular expressions
		var regex = RegEx.new()
		
		# StartTime
		regex.compile("StartTime\\s*=\\s*(\\d+(\\.\\d+)?)")
		var match_result = regex.search(content)
		if match_result:
			experiment["StartTime"] = match_result.get_string(1)
		
		# StopTime
		regex.compile("StopTime\\s*=\\s*(\\d+(\\.\\d+)?)")
		match_result = regex.search(content)
		if match_result:
			experiment["StopTime"] = match_result.get_string(1)
		
		# Interval
		regex.compile("Interval\\s*=\\s*(\\d+(\\.\\d+)?)")
		match_result = regex.search(content)
		if match_result:
			experiment["Interval"] = match_result.get_string(1)
		
		result["experiment"] = experiment
	
	_skip_until_semicolon()
	return result

func _parse_equation() -> String:
	print("\nParsing equation at position: ", _pos, ", Current char: ", _peek())
	var equation = ""
	var parentheses_count = 0
	
	while _pos < _len:
		var c = _peek()
		print("Equation char: '", c, "', Parentheses count: ", parentheses_count)
		
		if c == ";" and parentheses_count == 0:
			_next()  # Skip semicolon
			print("Found end of equation with semicolon")
			break
		
		if c == "(":
			parentheses_count += 1
		elif c == ")":
			parentheses_count -= 1
		
		equation += _next()
	
	equation = equation.strip_edges()
	if not equation.is_empty():
		print("Parsed equation: ", equation)
	return equation

func _parse_type_name() -> String:
	_skip_whitespace_and_comments()
	var type_name = ""
	
	if _pos >= _len:
		return ""
	
	# First character must be a letter
	var first = _peek()
	if not first.is_valid_identifier():
		return ""
	
	type_name += _next()
	
	# Rest can be letters, digits or underscore
	while _pos < _len:
		var c = _peek()
		if c.is_valid_identifier() or c == "_" or c.is_valid_int() or c == ".":
			type_name += _next()
		else:
			break
	
	return type_name.strip_edges()

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
		if c.is_valid_identifier() or c == "_" or c.is_valid_int():
			identifier += _next()
		else:
			break
	
	return identifier.strip_edges()

func _parse_string() -> String:
	_skip_whitespace_and_comments()
	if _peek() != "\"":
		return ""
	
	_next()  # Skip opening quote
	var string = ""
	
	while _pos < _len:
		var c = _next()
		if c == "\"":
			break
		string += c
	
	return string

func _parse_modifications() -> Dictionary:
	var modifications = {}
	
	_skip_whitespace_and_comments()
	if _peek() != "(":
		return modifications
	
	_next()  # Skip opening parenthesis
	
	while _pos < _len:
		_skip_whitespace_and_comments()
		
		# Check for end of modifications
		if _peek() == ")":
			_next()  # Skip closing parenthesis
			break
		
		# Parse modification name
		var name = _parse_identifier()
		if name.is_empty():
			break
		
		_skip_whitespace_and_comments()
		if _peek() != "=":
			break
		
		_next()  # Skip =
		_skip_whitespace_and_comments()
		
		# Parse modification value
		var value = ""
		while _pos < _len:
			var c = _peek()
			if c == ")" or c == ",":
				break
			value += _next()
		
		modifications[name] = value.strip_edges()
		
		_skip_whitespace_and_comments()
		if _peek() == ",":
			_next()  # Skip comma
			continue
		elif _peek() == ")":
			_next()  # Skip closing parenthesis
			break
	
	return modifications

func _peek() -> String:
	if _pos >= _len:
		return ""
	return _text[_pos]

func _next() -> String:
	if _pos >= _len:
		return ""
	var c = _text[_pos]
	_pos += 1
	return c

func _advance(count: int = 1) -> void:
	_pos = min(_pos + count, _len)

func _skip_whitespace_and_comments() -> void:
	while _pos < _len:
		# Skip whitespace
		while _pos < _len and _text[_pos].strip_edges().is_empty():
			_pos += 1
		
		# Check for comments
		if _pos + 1 < _len:
			if _text[_pos] == "/" and _text[_pos + 1] == "/":
				# Single-line comment
				print("Skipping single-line comment")
				_pos += 2  # Skip //
				while _pos < _len and _text[_pos] != "\n":
					_pos += 1
				continue
			elif _text[_pos] == "/" and _text[_pos + 1] == "*":
				# Multi-line comment
				_pos += 2  # Skip /*
				while _pos + 1 < _len:
					if _text[_pos] == "*" and _text[_pos + 1] == "/":
						_pos += 2  # Skip */
						break
					_pos += 1
				continue
		break

func _skip_whitespace() -> void:
	while _pos < _len and _text[_pos].strip_edges().is_empty():
		_pos += 1

func _skip_until_semicolon() -> void:
	while _pos < _len and _text[_pos] != ";":
		_pos += 1
	if _pos < _len:
		_pos += 1  # Skip the semicolon

func _match_keyword(keyword: String) -> bool:
	_skip_whitespace_and_comments()
	if _pos + keyword.length() > _len:
		return false
	
	var word = _text.substr(_pos, keyword.length())
	if word == keyword:
		var next_char = " " if _pos + keyword.length() >= _len else _text[_pos + keyword.length()]
		if not next_char.is_valid_identifier():
			_pos += keyword.length()
			return true
	return false

func _parse_definition_type() -> String:
	_skip_whitespace_and_comments()
	for keyword in KEYWORDS:
		if _match_keyword(keyword):
			return keyword
	return ""
