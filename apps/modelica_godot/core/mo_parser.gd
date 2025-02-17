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
			
		# Check for model-level annotation
		if _match_keyword("annotation"):
			print("\nParsing model annotation...")
			var annotation = _parse_model_annotation()
			if not annotation.is_empty():
				result.annotations = annotation
				print("Found model annotation: ", annotation)
			continue
			
		# Check for initial equation section
		if _match_keyword("initial equation"):
			print("\nParsing initial equations...")
			while _pos < _len:
				_skip_whitespace_and_comments()
				if _pos >= _len:
					break
					
				print("Initial equation - Current position: ", _pos, ", Current char: '", _peek(), "'")
				
				# Check for end of equation section
				if _match_keyword("equation") or _match_keyword("end") or _match_keyword("annotation"):
					print("Found end of initial equation section")
					break
					
				var equation = _parse_equation()
				if not equation.is_empty():
					result.initial_equations.append(equation)
					print("Found initial equation: ", equation)
			continue
			
		# Check for equation section
		if _match_keyword("equation"):
			print("\nParsing equations...")
			while _pos < _len:
				_skip_whitespace_and_comments()
				if _pos >= _len:
					break
					
				print("Equation - Current position: ", _pos, ", Current char: '", _peek(), "'")
				
				# Check for end of equation section
				if _match_keyword("end") or _match_keyword("annotation"):
					print("Found end of equation section")
					break
				
				var equation = _parse_equation()
				if not equation.is_empty():
					result.equations.append(equation)
					print("Found equation: ", equation)
			continue
		
		# Parse component declaration
		var component = _parse_component()
		if not component.is_empty():
			# Only add if it's a real component (not an end name)
			if component.type != result.name:
				result.components.append(component)
				print("Found component: ", component.get("type", ""), " ", component.get("name", ""))
	
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
		return {}
	
	# Parse name
	_skip_whitespace_and_comments()
	component.name = _parse_identifier()
	
	# Parse modifications if any
	_skip_whitespace_and_comments()
	if _peek() == "(":
		component.modifications = _parse_modifications()
	
	# Parse default value if any
	_skip_whitespace_and_comments()
	if _peek() == "=":
		_advance(1)  # Skip =
		_skip_whitespace_and_comments()
		component.default = _parse_value()
		component.value = component.default
	
	# Parse description if any
	_skip_whitespace_and_comments()
	if _peek() == "\"":
		component.description = _parse_string()
	
	# Parse annotation if any
	_skip_whitespace_and_comments()
	if _peek() == "a" and _peek_word() == "annotation":
		component.annotation = _parse_annotation()
	
	_skip_until_semicolon()
	return component

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
		if c.is_valid_identifier() or c == "_" or c.is_valid_int():
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
		
		break

func _skip_until_semicolon() -> void:
	print("Skipping until semicolon from position: ", _pos)
	
	# If we're already at a semicolon, just skip it and return
	if _peek() == ";":
		_next()  # Skip the semicolon
		print("Already at semicolon, skipped it")
		return
	
	# Otherwise, search for the next semicolon
	while _pos < _len:
		var c = _peek()
		if c == ";":
			_next()  # Skip the semicolon
			print("Found semicolon at position: ", _pos)
			return
		_next()  # Skip current character
	
	print("Reached end of text without finding semicolon")

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
	print("Moving from position ", _pos - 1, " to ", _pos, ", char: '", c, "'")
	return c

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

func _peek_word() -> String:
	var start = _pos
	var word = ""
	while start < _len and _text[start].is_valid_identifier():
		word += _text[start]
		start += 1
	return word

func _advance(length: int) -> void:
	_pos += length

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
