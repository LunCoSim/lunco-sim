class_name MOParser
extends RefCounted

enum TokenType {
	KEYWORD,
	IDENTIFIER,
	NUMBER,
	OPERATOR,
	STRING,
	COMMENT,
	NEWLINE,
	EOF
}

const KEYWORDS = [
	"model", "end", "parameter", "equation", "der", "input", "output",
	"flow", "stream", "connector", "package", "class", "type", "constant",
	"discrete", "Real", "Integer", "Boolean", "String", "extends"
]

var _text: String
var _pos: int = 0
var _line: int = 1
var _column: int = 1
var _package_path: String = ""

# Cache for parsed models to avoid re-parsing
var _model_cache: Dictionary = {}

func parse_file(path: String) -> Dictionary:
	_package_path = path.get_base_dir()
	
	# Check cache first
	if _model_cache.has(path):
		return _model_cache[path]
		
	print("Opening file: ", path)
	var file = FileAccess.open(path, FileAccess.READ)
	if file == null:
		push_error("Could not open file: " + path)
		return {}
		
	_text = file.get_as_text()
	_pos = 0
	_line = 1
	_column = 1
	
	var model = _parse_model()
	_model_cache[path] = model
	return model

func _parse_model() -> Dictionary:
	var model = {
		"type": "",  # model, connector, package, etc.
		"name": "",
		"description": "",
		"extends": [],
		"imports": [],
		"parameters": [],
		"variables": [],
		"equations": [],
		"annotations": {},
		"components": [],
		"connectors": []
	}
	
	print("Starting model parsing")
	while _pos < _text.length():
		var token = _next_token()
		
		match token.type:
			TokenType.KEYWORD:
				match token.value:
					"model", "connector", "package", "class":
						model.type = token.value
						var name_token = _next_token()
						if name_token.type == TokenType.IDENTIFIER:
							model.name = name_token.value
						print("Found model type: ", model.type, " name: ", model.name)
					
					"extends":
						var base_token = _next_token()
						if base_token.type == TokenType.IDENTIFIER:
							model.extends.append(base_token.value)
					
					"import":
						var import_path = _parse_import()
						if import_path:
							model.imports.append(import_path)
					
					"parameter":
						var param = _parse_parameter()
						model.parameters.append(param)
					
					"equation":
						var eq = _parse_equation()
						model.equations.append(eq)
					
					"end":
						print("Found end of model")
						break
			
			TokenType.COMMENT:
				if token.value.begins_with("\""):
					# Description string
					model.description = token.value.trim_prefix("\"").trim_suffix("\"")
				elif token.value.begins_with("annotation"):
					var annotation = _parse_annotation()
					model.annotations.merge(annotation)
			
			TokenType.IDENTIFIER:
				# This might be a component declaration
				var component = _parse_component_declaration(token.value)
				if component:
					if component.is_connector:
						model.connectors.append(component)
					else:
						model.components.append(component)
			
			TokenType.NEWLINE:
				continue
			
			TokenType.EOF:
				break
	
	return model

func _parse_import() -> String:
	var import_path = ""
	while true:
		var token = _next_token()
		match token.type:
			TokenType.IDENTIFIER:
				import_path += token.value
			TokenType.OPERATOR:
				if token.value == ".":
					import_path += "."
				else:
					break
			_:
				break
	return import_path

func _parse_component_declaration(type_name: String) -> Dictionary:
	var component = {
		"type": type_name,
		"name": "",
		"is_connector": false,
		"modifiers": {},
		"description": ""
	}
	
	var token = _next_token()
	if token.type == TokenType.IDENTIFIER:
		component.name = token.value
		
		# Check if this is a connector type
		component.is_connector = type_name.begins_with("Flange") or \
							   type_name.begins_with("Pin") or \
							   type_name.begins_with("Port")
		
		# Look for modifiers
		token = _next_token()
		if token.type == TokenType.OPERATOR and token.value == "(":
			component.modifiers = _parse_modifiers(token.value)
		
		# Look for description
		token = _next_token()
		if token.type == TokenType.STRING:
			component.description = token.value
		
		return component
	
	return {}

func _parse_modifiers(line: String) -> Dictionary:
	var modifiers = {}
	
	var start = line.find("(")
	var end = line.find(")")
	if start == -1 or end == -1:
		return modifiers
		
	var modifier_str = line.substr(start + 1, end - start - 1)
	var parts = modifier_str.split(",")
	
	for part in parts:
		var key_value = part.split("=")
		if key_value.size() == 2:
			modifiers[key_value[0].strip_edges()] = key_value[1].strip_edges()
	
	return modifiers

func _parse_annotation() -> Dictionary:
	var annotation = {}
	
	# Skip until we find "annotation("
	while _pos < _text.length():
		if _text.substr(_pos).begins_with("annotation("):
			_pos += 11  # Skip "annotation("
			break
		_pos += 1
	
	# Parse the annotation content
	var parentheses_count = 1
	var content = ""
	
	while _pos < _text.length() and parentheses_count > 0:
		var char = _text[_pos]
		if char == "(":
			parentheses_count += 1
		elif char == ")":
			parentheses_count -= 1
		
		if parentheses_count > 0:
			content += char
		_pos += 1
	
	# Parse Icon section if present
	if content.find("Icon(") != -1:
		var icon_start = content.find("Icon(") + 5
		var icon_end = content.find(")", icon_start)
		var icon_content = content.substr(icon_start, icon_end - icon_start)
		
		annotation["Icon"] = {
			"graphics": _parse_graphics(icon_content)
		}
	
	return annotation

func _parse_graphics(content: String) -> Array:
	var graphics = []
	var lines = content.split("\n")
	
	for line in lines:
		line = line.strip_edges()
		if line.begins_with("Line("):
			graphics.append(_parse_line(line))
		elif line.begins_with("Rectangle("):
			graphics.append(_parse_rectangle(line))
		elif line.begins_with("Text("):
			graphics.append(_parse_text(line))
	
	return graphics

func _parse_line(line: String) -> Dictionary:
	var graphic = {"type": "Line"}
	
	# Extract points
	var points_start = line.find("points={{") + 8
	var points_end = line.find("}}", points_start)
	var points_str = line.substr(points_start, points_end - points_start)
	
	var points = []
	for point in points_str.split("},{"):
		var coords = point.replace("{", "").replace("}", "").split(",")
		points.append(float(coords[0]))
		points.append(float(coords[1]))
	
	graphic["points"] = points
	return graphic

func _parse_rectangle(line: String) -> Dictionary:
	var graphic = {"type": "Rectangle"}
	
	# Extract extent
	var extent_start = line.find("extent={{") + 8
	var extent_end = line.find("}}", extent_start)
	var extent_str = line.substr(extent_start, extent_end - extent_start)
	
	var extent = []
	for coord in extent_str.split("},{"):
		var coords = coord.replace("{", "").replace("}", "").split(",")
		extent.append(float(coords[0]))
		extent.append(float(coords[1]))
	
	graphic["extent"] = extent
	
	# Extract fillColor if present
	var fill_color_start = line.find("fillColor={")
	if fill_color_start != -1:
		fill_color_start += 10
		var fill_color_end = line.find("}", fill_color_start)
		var color_str = line.substr(fill_color_start, fill_color_end - fill_color_start)
		var colors = color_str.split(",")
		graphic["fillColor"] = [int(colors[0]), int(colors[1]), int(colors[2])]
	
	return graphic

func _parse_text(line: String) -> Dictionary:
	var graphic = {"type": "Text"}
	
	# Extract extent
	var extent_start = line.find("extent={{") + 8
	var extent_end = line.find("}}", extent_start)
	var extent_str = line.substr(extent_start, extent_end - extent_start)
	
	var extent = []
	for coord in extent_str.split("},{"):
		var coords = coord.replace("{", "").replace("}", "").split(",")
		extent.append(float(coords[0]))
		extent.append(float(coords[1]))
	
	graphic["extent"] = extent
	
	# Extract text string
	var text_start = line.find("textString=\"") + 12
	var text_end = line.find("\"", text_start)
	graphic["textString"] = line.substr(text_start, text_end - text_start)
	
	return graphic

func _parse_parameter() -> Dictionary:
	var param = {
		"type": "",
		"name": "",
		"value": 0.0,
		"unit": "",
		"description": ""
	}
	
	while true:
		var token = _next_token()
		match token.type:
			TokenType.KEYWORD:
				param.type = token.value
			
			TokenType.IDENTIFIER:
				param.name = token.value
			
			TokenType.OPERATOR:
				if token.value == "=":
					var value_token = _next_token()
					if value_token.type == TokenType.NUMBER:
						param.value = float(value_token.value)
			
			TokenType.STRING:
				param.description = token.value
			
			TokenType.NEWLINE:
				break
	
	return param

func _parse_equation() -> String:
	var equation = ""
	
	while true:
		var token = _next_token()
		match token.type:
			TokenType.NEWLINE:
				break
			TokenType.COMMENT:
				continue
			_:
				equation += token.value + " "
	
	return equation.strip_edges()

func _next_token() -> Dictionary:
	_skip_whitespace()
	
	if _pos >= _text.length():
		return {"type": TokenType.EOF, "value": ""}
	
	var char = _text[_pos]
	
	# Handle comments
	if char == "/" and _pos + 1 < _text.length() and _text[_pos + 1] == "/":
		return _read_line_comment()
	
	# Handle string literals
	if char == "\"":
		return _read_string()
	
	# Handle numbers
	if _is_digit(char) or char == "-":
		return _read_number()
	
	# Handle identifiers and keywords
	if _is_letter(char):
		return _read_identifier()
	
	# Handle operators
	if "=+-*/()<>".contains(char):
		_pos += 1
		return {"type": TokenType.OPERATOR, "value": char}
	
	# Handle newlines
	if char == "\n":
		_line += 1
		_column = 1
		_pos += 1
		return {"type": TokenType.NEWLINE, "value": "\n"}
	
	# Skip unrecognized characters
	_pos += 1
	return _next_token()

func _skip_whitespace() -> void:
	while _pos < _text.length() and _text[_pos] in [" ", "\t", "\r"]:
		_pos += 1
		_column += 1

func _read_line_comment() -> Dictionary:
	var comment = ""
	_pos += 2  # Skip //
	
	while _pos < _text.length() and _text[_pos] != "\n":
		comment += _text[_pos]
		_pos += 1
	
	return {"type": TokenType.COMMENT, "value": comment.strip_edges()}

func _read_string() -> Dictionary:
	var string = ""
	_pos += 1  # Skip opening quote
	
	while _pos < _text.length() and _text[_pos] != "\"":
		string += _text[_pos]
		_pos += 1
	
	_pos += 1  # Skip closing quote
	return {"type": TokenType.STRING, "value": string}

func _read_number() -> Dictionary:
	var number = ""
	var has_decimal = false
	
	if _text[_pos] == "-":
		number += "-"
		_pos += 1
	
	while _pos < _text.length():
		var char = _text[_pos]
		if _is_digit(char):
			number += char
		elif char == "." and not has_decimal:
			number += char
			has_decimal = true
		else:
			break
		_pos += 1
	
	return {"type": TokenType.NUMBER, "value": number}

func _read_identifier() -> Dictionary:
	var identifier = ""
	
	while _pos < _text.length() and (_is_letter(_text[_pos]) or _is_digit(_text[_pos]) or _text[_pos] == "_"):
		identifier += _text[_pos]
		_pos += 1
	
	if identifier in KEYWORDS:
		return {"type": TokenType.KEYWORD, "value": identifier}
	else:
		return {"type": TokenType.IDENTIFIER, "value": identifier}

func _is_letter(c: String) -> bool:
	return (c >= "a" and c <= "z") or (c >= "A" and c <= "Z")

func _is_digit(c: String) -> bool:
	return c >= "0" and c <= "9" 