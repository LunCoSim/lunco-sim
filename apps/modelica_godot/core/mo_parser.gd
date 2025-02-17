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

# Add new constants for syntax rules
const MODELICA_KEYWORDS = {
	"algorithm": true, "and": true, "annotation": true, "block": true,
	"break": true, "class": true, "connect": true, "connector": true,
	"constant": true, "constrainedby": true, "der": true, "discrete": true,
	"each": true, "else": true, "elseif": true, "elsewhen": true,
	"encapsulated": true, "end": true, "enumeration": true, "equation": true,
	"expandable": true, "extends": true, "external": true, "false": true,
	"final": true, "flow": true, "for": true, "function": true,
	"if": true, "import": true, "in": true, "initial": true,
	"inner": true, "input": true, "loop": true, "model": true,
	"not": true, "operator": true, "or": true, "outer": true,
	"output": true, "package": true, "parameter": true, "partial": true,
	"protected": true, "public": true, "record": true, "redeclare": true,
	"replaceable": true, "return": true, "stream": true, "then": true,
	"true": true, "type": true, "when": true, "while": true,
	"within": true
}

const MODELICA_FUNCTIONS = {
	"der": true,
	"sin": true,
	"cos": true,
	"tan": true,
	"asin": true,
	"acos": true,
	"atan": true,
	"atan2": true,
	"sinh": true,
	"cosh": true,
	"tanh": true,
	"exp": true,
	"log": true,
	"log10": true,
	"sqrt": true,
	"abs": true,
	"sign": true,
	"max": true,
	"min": true,
	"integer": true,
	"floor": true,
	"ceil": true,
	"pre": true,
	"edge": true,
	"change": true,
	"sample": true,
	"initial": true,
	"terminal": true,
	"noEvent": true,
	"smooth": true,
	"delay": true
}

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
	
	if _pos >= _len:
		return ""
		
	print("Parsing equation at position: ", _pos, ", Current char: '", _peek(), "'")
	
	var equation = ""
	var parentheses_count = 0
	var bracket_count = 0
	
	while _pos < _len:
		var c = _peek()
		
		# Handle parentheses and brackets
		if c == "(":
			parentheses_count += 1
			equation += _next()
			continue
		elif c == ")":
			parentheses_count -= 1
			equation += _next()
			continue
		elif c == "[":
			bracket_count += 1
			equation += _next()
			continue
		elif c == "]":
			bracket_count -= 1
			equation += _next()
			continue
			
		# Break on semicolon if not inside any grouping
		if c == ";" and parentheses_count == 0 and bracket_count == 0:
			_next()  # Skip semicolon
			print("Found end of equation with semicolon")
			break
			
		# Handle string literals
		if c == "\"":
			equation += _parse_string()
			continue
			
		# Handle operators
		if c in ["+", "-", "*", "/", "^", "=", "<", ">", ".", ","]:
			equation += _next()
			continue
			
		# Handle identifiers and numbers
		if _is_letter(c) or _is_digit(c) or c == "_":
			var save_pos = _pos
			var word = _peek_word()
			
			# Check if it's a built-in function
			if MODELICA_FUNCTIONS.has(word):
				equation += word
				_advance(word.length())
				continue
				
			# Otherwise try to parse as a value
			_pos = save_pos
			var value = _parse_value()
			if not value.is_empty():
				equation += value
				continue
			
		# Skip whitespace
		if _is_whitespace(c):
			_next()
			continue
			
		# If we get here, we have an invalid character
		push_error("Invalid character in equation: " + c)
		break
	
	var result = equation.strip_edges()
	if not result.is_empty():
		print("Parsed equation: ", result)
	return result

func _parse_definition_type() -> String:
	_skip_whitespace_and_comments()
	var word = _peek_word()
	
	if word in ["model", "connector", "package", "class", "record", "block", "type", "function"]:
		_advance(word.length())
		return word
	return ""

func _parse_identifier() -> String:
	_skip_whitespace_and_comments()
	var id = ""
	
	# First character must be a letter or underscore
	var first = _peek()
	if not (_is_letter(first) or first == "_"):
		return ""
		
	id += _next()
	
	# Rest can be letters, digits or underscore
	while _pos < _len:
		var c = _peek()
		if _is_letter(c) or _is_digit(c) or c == "_":
			id += _next()
		else:
			break
			
	# Check if it's not a keyword
	if _is_keyword(id):
		return ""
		
	return id

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
	
	# Handle string literal
	if _peek() == "\"":
		return _parse_string()
		
	# Handle array constructor
	if _peek() == "{":
		return _parse_array()
		
	# Handle numeric literal or identifier
	var value = ""
	var has_digit = false
	var has_decimal = false
	var has_exponent = false
	
	# Handle sign
	if _peek() in ["+", "-"]:
		value += _next()
	
	while _pos < _len:
		var c = _peek()
		
		# Break on delimiters
		if c in [",", ")", ";", " ", "\t", "\n"]:
			break
			
		# Handle digits
		if c.is_valid_int():
			has_digit = true
			value += _next()
			continue
			
		# Handle decimal point
		if c == "." and not has_decimal and not has_exponent:
			has_decimal = true
			value += _next()
			continue
			
		# Handle exponent
		if (c == "e" or c == "E") and has_digit and not has_exponent:
			has_exponent = true
			value += _next()
			# Handle exponent sign
			if _peek() in ["+", "-"]:
				value += _next()
			continue
			
		# If not a valid numeric character, try parsing as identifier
		if not has_digit:
			return _parse_identifier()
			
		# Invalid character in number
		break
	
	return value.strip_edges()

func _parse_array() -> String:
	var array = "{"
	_next()  # Skip opening brace
	
	while _pos < _len:
		_skip_whitespace_and_comments()
		
		if _peek() == "}":
			array += "}"
			_next()
			break
			
		var value = _parse_value()
		if not value.is_empty():
			array += value
			
		_skip_whitespace_and_comments()
		if _peek() == ",":
			array += ","
			_next()
			
	return array

func _skip_whitespace_and_comments() -> void:
	while _pos < _len:
		var c = _peek()
		
		# Skip whitespace
		if _is_whitespace(c):
			_next()
			continue
			
		# Skip single-line comment
		if c == "/" and _peek(1) == "/":
			while _pos < _len and _peek() != "\n":
				_next()
			continue
			
		# Skip multi-line comment
		if c == "/" and _peek(1) == "*":
			_next() # Skip /
			_next() # Skip *
			while _pos < _len:
				if _peek() == "*" and _peek(1) == "/":
					_next() # Skip *
					_next() # Skip /
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
	var save_pos = _pos
	var word = ""
	
	while _pos < _len:
		var c = _peek()
		if not (_is_letter(c) or _is_digit(c) or c == "_"):
			break
		word += _next()
	
	_pos = save_pos
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

func _is_keyword(word: String) -> bool:
	return MODELICA_KEYWORDS.has(word.to_lower())

func _is_valid_modelica_identifier(id: String) -> bool:
	if id.is_empty():
		return false
		
	# First character must be a letter or underscore
	var first = id[0]
	if not (first.is_valid_identifier() and (first.is_letter() or first == "_")):
		return false
		
	# Remaining characters must be letters, digits, or underscore
	for i in range(1, id.length()):
		var c = id[i]
		if not (c.is_valid_identifier() and (c.is_letter() or c.is_digit() or c == "_")):
			return false
			
	# Check if it's not a keyword
	return not _is_keyword(id)

func _is_whitespace(c: String) -> bool:
	return c == " " or c == "\t" or c == "\n" or c == "\r"

func _is_letter(c: String) -> bool:
	var code = c.unicode_at(0)
	return (code >= 65 and code <= 90) or (code >= 97 and code <= 122)  # A-Z or a-z

func _is_digit(c: String) -> bool:
	var code = c.unicode_at(0)
	return code >= 48 and code <= 57  # 0-9
