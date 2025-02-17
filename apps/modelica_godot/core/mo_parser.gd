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
			
		print("Current position: ", _pos, ", Current char: '", _peek(), "'")
		
		# Check for end of definition
		if _match_keyword("end"):
			print("Found 'end' keyword")
			# Parse the end name if present
			_skip_whitespace_and_comments()
			var end_name = _parse_identifier()
			if not end_name.is_empty():
				print("Found end name: ", end_name)
				if end_name == result.name:
					print("End name matches definition name")
				else:
					push_error("End name '" + end_name + "' does not match definition name '" + result.name + "'")
			# Skip any remaining content until semicolon
			_skip_until_semicolon()
			print("Found end of definition")
			return result
		
		# Check for equation sections
		if _match_keyword("equation"):
			print("Found equation section")
			in_equation_section = true
			in_initial_equation_section = false
			continue
		
		if _match_keyword("initial equation"):
			print("Found initial equation section")
			in_initial_equation_section = true
			in_equation_section = false
			continue
		
		# Check for model-level annotation
		if _match_keyword("annotation"):
			print("\nParsing model annotation...")
			var annotation = _parse_model_annotation()
			if not annotation.is_empty():
				result.annotations = annotation
				print("Found model annotation: ", annotation)
			_skip_until_semicolon()
			continue
		
		# Parse equations or components based on current section
		if in_equation_section:
			var equation = _parse_equation()
			if not equation.is_empty():
				result.equations.append(equation)
				print("Found equation: ", equation)
			_skip_whitespace_and_comments()
			if _peek() == ";":
				_next()  # Skip semicolon
		elif in_initial_equation_section:
			var equation = _parse_equation()
			if not equation.is_empty():
				result.initial_equations.append(equation)
				print("Found initial equation: ", equation)
			_skip_whitespace_and_comments()
			if _peek() == ";":
				_next()  # Skip semicolon
		else:
			var component = _parse_component()
			if not component.is_empty():
				result.components.append(component)
				print("Found component: ", component)
			if _peek() == ";":
				_next()  # Skip semicolon
	
	print("\nParsing complete")
	print("Components found: ", result.components.size())
	print("Initial equations found: ", result.initial_equations.size())
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

func _parse_component() -> Dictionary:
	print("\nParsing component at position: ", _pos)
	var component = {
		"type": "",
		"name": "",
		"modifications": {},
		"description": "",
		"annotation": {},
		"is_parameter": false,
		"attributes": [],  # For flow, input, output etc.
		"value": "",
		"default": ""
	}
	
	# Parse attributes (flow, input, output, etc.)
	while true:
		_skip_whitespace_and_comments()
		var next_word = _peek_word()
		print("Checking word: ", next_word)
		if next_word in ["flow", "input", "output", "stream"]:
			component.attributes.append(next_word)
			_advance(next_word.length())
			_skip_whitespace_and_comments()
		elif next_word == "parameter":
			component.is_parameter = true
			_advance(next_word.length())
			_skip_whitespace_and_comments()
		else:
			break
	
	# Parse type
	component.type = _parse_identifier()
	if component.type.is_empty():
		print("No type found, returning empty component")
		return {}
	print("Found type: ", component.type)
	
	# Parse name
	_skip_whitespace_and_comments()
	component.name = _parse_identifier()
	if component.name.is_empty():
		print("No name found, returning empty component")
		return {}
	print("Found name: ", component.name)
	
	# Parse array dimensions if any
	_skip_whitespace_and_comments()
	if _peek() == "[":
		component.dimensions = _parse_array_dimensions()
	
	# Parse modifications if any
	_skip_whitespace_and_comments()
	if _peek() == "(":
		print("Found modifications")
		component.modifications = _parse_modifications()
	
	# Parse default value if any
	_skip_whitespace_and_comments()
	if _peek() == "=":
		print("Found default value")
		_next()  # Skip =
		_skip_whitespace_and_comments()
		component.default = _parse_value()
		component.value = component.default
	
	# Parse description if any
	_skip_whitespace_and_comments()
	if _peek() == "\"":
		print("Found description")
		component.description = _parse_string()
	
	# Parse annotation if any
	_skip_whitespace_and_comments()
	if _match_keyword("annotation"):
		print("Found annotation")
		component.annotation = _parse_model_annotation()
		_skip_whitespace_and_comments()
	
	# Must end with a semicolon
	if _peek() == ";":
		_next()  # Skip semicolon
		print("Found end of component declaration")
	else:
		print("Warning: Component declaration did not end with semicolon")
		
	print("Finished parsing component: ", component)
	return component

func _parse_array_dimensions() -> Array:
	var dimensions = []
	_next()  # Skip [
	
	while _pos < _len:
		_skip_whitespace_and_comments()
		var dimension = ""
		
		# Parse the dimension value
		while _pos < _len:
			var c = _peek()
			if c.is_valid_int() or c == ":":
				dimension += _next()
			else:
				break
		
		if not dimension.is_empty():
			dimensions.append(dimension)
		
		_skip_whitespace_and_comments()
		if _peek() == "]":
			_next()  # Skip ]
			break
		elif _peek() == ",":
			_next()  # Skip ,
			continue
	
	return dimensions

func _parse_equation() -> String:
	_skip_whitespace_and_comments()
	var equation = ""
	var parentheses_count = 0
	
	print("Parsing equation at position: ", _pos, ", Current char: '", _peek(), "'")
	
	# Check for annotation
	if _match_keyword("annotation"):
		print("Found equation annotation")
		var annotation = _parse_model_annotation()
		return "annotation" + annotation.get("content", "")
	
	# Handle empty lines or end of section
	if _peek() == ";" or _match_keyword("end") or _match_keyword("annotation"):
		print("Found end of equation or empty line")
		return ""
	
	while _pos < _len:
		var c = _peek()
		print("Equation char: '", c, "', Parentheses count: ", parentheses_count)
		
		# Handle parentheses counting
		if c == "(":
			parentheses_count += 1
			equation += _next()
			continue
		elif c == ")":
			parentheses_count -= 1
			equation += _next()
			continue
		
		# Break on semicolon if not inside parentheses
		if c == ";" and parentheses_count == 0:
			_next()  # Skip semicolon
			print("Found end of equation with semicolon")
			break
		
		# Break on end or annotation if not inside parentheses
		if parentheses_count == 0 and c.strip_edges().is_empty():
			var save_pos = _pos
			_skip_whitespace_and_comments()
			if _match_keyword("end") or _match_keyword("annotation"):
				_pos = save_pos
				print("Found end or annotation after equation")
				break
			_pos = save_pos
		
		equation += _next()
	
	var result = equation.strip_edges()
	print("Parsed equation: ", result)
	return result

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
		if c.is_valid_identifier() or c == "_" or c.is_valid_int() or c == ".":
			identifier += _next()
		else:
			break
	
	return identifier.strip_edges()

func _parse_string() -> String:
	var string = ""
	
	if _peek() != "\"":
		return ""
	
	_next()  # Skip opening quote
	
	while _pos < _len:
		var c = _peek()
		
		# Handle escaped quotes
		if c == "\\" and _peek(1) == "\"":
			string += "\""
			_next()  # Skip backslash
			_next()  # Skip quote
			continue
		
		# Handle normal quote
		if c == "\"":
			_next()  # Skip closing quote
			break
			
		string += _next()
	
	return string

func _parse_modifications() -> Dictionary:
	print("\nParsing modifications at position: ", _pos)
	var modifications = {}
	var parentheses_count = 1  # We've already consumed the opening parenthesis
	
	_next()  # Skip opening parenthesis
	
	while _pos < _len:
		_skip_whitespace_and_comments()
		
		# Check for end of modifications
		if _peek() == ")":
			if parentheses_count == 1:
				_next()  # Skip closing parenthesis
				print("Found end of modifications")
				break
			parentheses_count -= 1
			_next()
			continue
			
		# Handle nested parentheses
		if _peek() == "(":
			parentheses_count += 1
			_next()
			continue
		
		# Parse modification name
		var name = _parse_identifier()
		if name.is_empty():
			print("No valid identifier found, breaking")
			break
		
		print("Parsing modification: ", name)
		
		_skip_whitespace_and_comments()
		if _peek() == "=":
			_next()  # Skip =
			_skip_whitespace_and_comments()
			
			# Parse the value, which could be complex
			var value = ""
			var in_string = false
			
			while _pos < _len:
				var c = _peek()
				
				# Handle string literals
				if c == "\"":
					if not in_string:
						in_string = true
					else:
						if _peek(-1) != "\\":  # Not an escaped quote
							in_string = false
					value += _next()
					continue
				
				# Only check for delimiters if not in a string
				if not in_string:
					if c == "," and parentheses_count == 1:
						_next()  # Skip comma
						break
					elif c == ")":
						if parentheses_count == 1:
							break
						parentheses_count -= 1
					elif c == "(":
						parentheses_count += 1
				
				value += _next()
			
			modifications[name] = value.strip_edges()
			print("Added modification ", name, " = ", value.strip_edges())
		
		_skip_whitespace_and_comments()
		if _peek() == ",":
			_next()  # Skip comma
	
	print("Finished parsing modifications: ", modifications)
	return modifications

func _parse_value() -> String:
	_skip_whitespace_and_comments()
	var value = ""
	var parentheses_count = 0
	var in_string = false
	
	# Handle string literals
	if _peek() == "\"":
		return _parse_string()
	
	while _pos < _len:
		var c = _peek()
		
		# Break on delimiters if not in a string or parentheses
		if not in_string and parentheses_count == 0:
			if c in [",", ")", ";"]:
				break
		
		# Handle string literals
		if c == "\"":
			if not in_string:
				in_string = true
			else:
				if _peek(-1) != "\\":  # Not an escaped quote
					in_string = false
			value += _next()
			continue
		
		# Handle parentheses
		if not in_string:
			if c == "(":
				parentheses_count += 1
			elif c == ")":
				parentheses_count -= 1
				if parentheses_count < 0:  # Found closing parenthesis of outer context
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
			print("Skipping single-line comment")
			while _pos < _len and _peek() != "\n":
				_next()
			if _pos < _len:
				_next()  # Skip the newline
			continue
		
		# Skip multi-line comments
		if c == "/" and _peek(1) == "*":
			print("Skipping multi-line comment")
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
	print("Skipping until semicolon from position: ", _pos)
	var parentheses_count = 0
	var in_string = false
	
	# If we're already at a semicolon, just skip it and return
	if _peek() == ";":
		_next()  # Skip the semicolon
		print("Already at semicolon, skipped it")
		return
	
	# Otherwise, search for the next semicolon
	while _pos < _len:
		var c = _peek()
		
		# Handle string literals
		if c == "\"":
			if not in_string:
				in_string = true
			else:
				if _peek(-1) != "\\":  # Not an escaped quote
					in_string = false
			_next()
			continue
		
		# Only count parentheses if not in a string
		if not in_string:
			if c == "(":
				parentheses_count += 1
			elif c == ")":
				parentheses_count -= 1
		
		# Break on semicolon if not in string and not in parentheses
		if c == ";" and not in_string and parentheses_count <= 0:
			_next()  # Skip the semicolon
			print("Found semicolon at position: ", _pos)
			return
		
		_next()  # Skip current character
	
	print("Reached end of text without finding semicolon")

func _match_keyword(keyword: String) -> bool:
	_skip_whitespace_and_comments()
	var start_pos = _pos
	
	# Try to match each character of the keyword
	for c in keyword:
		if _pos >= _len or _peek() != c:
			_pos = start_pos
			return false
		_next()
	
	# Check that the next character is not a valid identifier character
	# This prevents matching "model" in "modeling" for example
	if _pos < _len:
		var next_char = _peek()
		if next_char.is_valid_identifier() or next_char == "_" or next_char.is_valid_int():
			_pos = start_pos
			return false
	
	print("Matched keyword: ", keyword)
	return true

func _peek(offset: int = 0) -> String:
	if _pos + offset >= _len or _pos + offset < 0:
		return ""
	return _text[_pos + offset]

func _next() -> String:
	if _pos >= _len:
		return ""
	var c = _text[_pos]
	_pos += 1
	print("Moving from position ", _pos - 1, " to ", _pos, ", char: '", c, "'")
	return c

func _parse_model_annotation() -> Dictionary:
	print("Parsing model annotation")
	var annotation = {}
	
	_skip_whitespace_and_comments()
	if _peek() != "(":
		push_error("Expected '(' after annotation keyword")
		return annotation
	
	_next()  # Skip opening parenthesis
	var content = ""
	var parentheses_count = 1
	var in_string = false
	
	while _pos < _len:
		var c = _peek()
		
		# Handle string literals
		if c == "\"":
			if not in_string:
				in_string = true
			else:
				if _peek(-1) != "\\":  # Not an escaped quote
					in_string = false
			content += _next()
			continue
		
		# Only count parentheses if not in a string
		if not in_string:
			if c == "(":
				parentheses_count += 1
			elif c == ")":
				parentheses_count -= 1
				if parentheses_count == 0:
					_next()  # Skip closing parenthesis
					break
		
		content += _next()
	
	annotation["content"] = content.strip_edges()
	print("Parsed annotation content: ", content)
	return annotation

func _peek_word() -> String:
	var word = ""
	var save_pos = _pos
	
	while _pos < _len:
		var c = _peek()
		if c.is_valid_identifier():
			word += _next()
		else:
			break
	
	_pos = save_pos
	return word

func _advance(count: int) -> void:
	for i in range(count):
		if _pos < _len:
			_next()

func _parse_annotation() -> Dictionary:
	_skip_whitespace_and_comments()
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
