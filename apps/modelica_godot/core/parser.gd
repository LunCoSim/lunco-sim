class_name Parser
extends RefCounted

var _package_path: String = ""
var _model_cache: Dictionary = {}

# Keywords
const KEYWORDS = [
	"model",
	"class",
	"package",
	"connector",
	"extends",
	"import",
	"end"
]

func _init():
	_package_path = ""
	_model_cache = {}

func parse_file(file_path: String) -> Dictionary:
	# Check cache first
	if file_path in _model_cache:
		return _model_cache[file_path]
		
	_package_path = file_path.get_base_dir()
	
	var file = FileAccess.open(file_path, FileAccess.READ)
	if not file:
		push_error("Failed to open file: " + file_path)
		return {}
		
	var content = file.get_as_text()
	file.close()
	
	var model = _parse_model(content)
	_model_cache[file_path] = model
	return model

func _parse_model(content: String) -> Dictionary:
	var model = {
		"type": "",
		"name": "",
		"extends": [],
		"imports": [],
		"components": [],
		"connectors": []
	}
	
	var lines = content.split("\n")
	var i = 0
	while i < lines.size():
		var line = lines[i].strip_edges()
		
		# Skip empty lines and comments
		if line.is_empty() or line.begins_with("//"):
			i += 1
			continue
			
		# Handle multi-line comments
		if line.begins_with("/*"):
			while i < lines.size() and not lines[i].strip_edges().ends_with("*/"):
				i += 1
			i += 1
			continue
		
		# Parse model/class/package declaration
		if line.begins_with("model ") or line.begins_with("class ") or line.begins_with("package "):
			var parts = line.split(" ", false)
			model.type = parts[0]
			model.name = parts[1]
			i += 1
			continue
			
		# Parse extends
		if line.begins_with("extends "):
			var extends_name = line.substr(8).strip_edges()
			if extends_name.ends_with(";"):
				extends_name = extends_name.substr(0, extends_name.length() - 1)
			model.extends.append(extends_name)
			i += 1
			continue
			
		# Parse imports
		if line.begins_with("import "):
			var import_path = _parse_import(line)
			if not import_path.is_empty():
				model.imports.append(import_path)
			i += 1
			continue
			
		# Parse component declarations
		if _is_component_declaration(line):
			var component = _parse_component_declaration(line)
			if not component.is_empty():
				model.components.append(component)
			i += 1
			continue
			
		# Parse connector declarations
		if line.begins_with("connector "):
			var connector = _parse_connector_declaration(lines, i)
			if not connector.is_empty():
				model.connectors.append(connector)
			i = connector.end_line + 1
			continue
			
		i += 1
	
	return model

func _parse_import(line: String) -> String:
	# Remove 'import' and any trailing semicolon
	line = line.substr(7).strip_edges()
	if line.ends_with(";"):
		line = line.substr(0, line.length() - 1)
	return line

func _parse_component_declaration(line: String) -> Dictionary:
	# Remove any trailing semicolon or comment
	var semicolon_pos = line.find(";")
	if semicolon_pos != -1:
		line = line.substr(0, semicolon_pos)
	
	var comment_pos = line.find("//")
	if comment_pos != -1:
		line = line.substr(0, comment_pos)
	
	line = line.strip_edges()
	
	# Split into type and name
	var parts = line.split(" ", false)
	if parts.size() < 2:
		return {}
		
	var component = {
		"type": parts[0],
		"name": parts[1],
		"modifiers": {}
	}
	
	# Parse modifiers if present
	if line.find("(") != -1:
		component.modifiers = _parse_modifiers(line)
	
	return component

func _parse_connector_declaration(lines: Array, start_line: int) -> Dictionary:
	var line = lines[start_line].strip_edges()
	var parts = line.split(" ", false)
	
	var connector = {
		"type": "connector",
		"name": parts[1],
		"variables": [],
		"end_line": start_line
	}
	
	var i = start_line + 1
	while i < lines.size():
		line = lines[i].strip_edges()
		
		if line.begins_with("end"):
			connector.end_line = i
			break
			
		if not line.is_empty() and not line.begins_with("//"):
			var var_parts = line.split(" ", false)
			if var_parts.size() >= 2:
				connector.variables.append({
					"type": var_parts[0],
					"name": var_parts[1].trim_suffix(";")
				})
		
		i += 1
	
	return connector

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

func _is_component_declaration(line: String) -> bool:
	var parts = line.split(" ", false)
	if parts.size() < 2:
		return false
		
	# Check if first word is a keyword
	if parts[0] in KEYWORDS:
		return false
		
	# Must end with semicolon or have modifiers
	return line.find(";") != -1 or line.find("(") != -1 