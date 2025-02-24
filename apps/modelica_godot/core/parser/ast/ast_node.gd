@tool
extends RefCounted
class_name ASTNode

enum NodeType {
	# Common nodes
	UNKNOWN,
	ERROR,
	
	# Expression nodes
	NUMBER,
	IDENTIFIER,
	STRING,
	OPERATOR,
	FUNCTION_CALL,
	ARRAY_ACCESS,
	
	# Equation nodes
	EQUATION,
	DIFFERENTIAL_EQUATION,
	WHEN_EQUATION,
	IF_EQUATION,
	FOR_EQUATION,
	CONNECT_EQUATION,
	
	# Modelica nodes
	MODEL,
	CONNECTOR,
	COMPONENT,
	PARAMETER,
	VARIABLE,
	CLASS,
	EXTENDS,
	IMPORT,
	ANNOTATION
}

var type: NodeType
var value: Variant  # The actual value/operator/function name
var children: Array[ASTNode]
var metadata: Dictionary  # Additional information like line numbers, comments, etc.

# Expression-specific fields
var left: ASTNode   # Left operand for binary ops
var right: ASTNode  # Right operand for binary ops
var operand: ASTNode # Single operand for unary ops
var arguments: Array[ASTNode] # Arguments for function calls

# Equation-specific fields
var is_initial: bool = false
var is_differential: bool = false
var state_variable: String = ""
var dependencies: Array[String] = []

# Modelica-specific fields
var visibility: String = "public"  # public, protected
var variability: String = ""  # parameter, constant, discrete
var causality: String = ""   # input, output
var modifications: Dictionary = {}

func _init(p_type: NodeType, p_value: Variant = null) -> void:
	type = p_type
	value = p_value
	children = []
	metadata = {}

func add_child(node: ASTNode) -> void:
	if node:
		children.append(node)

func add_metadata(key: String, value: Variant) -> void:
	metadata[key] = value

func get_metadata(key: String, default: Variant = null) -> Variant:
	return metadata.get(key, default)

func add_dependency(var_name: String) -> void:
	if not dependencies.has(var_name):
		dependencies.append(var_name)

func set_state_variable(var_name: String) -> void:
	state_variable = var_name
	is_differential = true
	add_dependency(var_name)

func is_state_variable() -> bool:
	return not state_variable.is_empty()

func get_dependencies() -> Array[String]:
	var all_deps: Array[String] = []
	all_deps.append_array(dependencies)
	
	# Collect dependencies from children
	for child in children:
		all_deps.append_array(child.get_dependencies())
	
	# Collect from expression-specific fields
	if left:
		all_deps.append_array(left.get_dependencies())
	if right:
		all_deps.append_array(right.get_dependencies())
	if operand:
		all_deps.append_array(operand.get_dependencies())
	for arg in arguments:
		all_deps.append_array(arg.get_dependencies())
	
	# Remove duplicates while preserving order
	var unique_deps: Array[String] = []
	for dep in all_deps:
		if not unique_deps.has(dep):
			unique_deps.append(dep)
	
	return unique_deps

func is_equation() -> bool:
	return type in [
		NodeType.EQUATION,
		NodeType.DIFFERENTIAL_EQUATION,
		NodeType.WHEN_EQUATION,
		NodeType.IF_EQUATION,
		NodeType.FOR_EQUATION,
		NodeType.CONNECT_EQUATION
	]

func is_expression() -> bool:
	return type in [
		NodeType.NUMBER,
		NodeType.IDENTIFIER,
		NodeType.STRING,
		NodeType.OPERATOR,
		NodeType.FUNCTION_CALL,
		NodeType.ARRAY_ACCESS
	]

func is_declaration() -> bool:
	return type in [
		NodeType.MODEL,
		NodeType.CONNECTOR,
		NodeType.COMPONENT,
		NodeType.PARAMETER,
		NodeType.VARIABLE,
		NodeType.CLASS
	]

# Format node as a mathematical expression
func format_expression() -> String:
	match type:
		NodeType.NUMBER:
			return str(value)
			
		NodeType.IDENTIFIER:
			return str(value)
			
		NodeType.STRING:
			return "\"%s\"" % value
			
		NodeType.OPERATOR:
			var left_str = left.format_expression() if left else ""
			var right_str = right.format_expression() if right else ""
			return "(" + left_str + " " + str(value) + " " + right_str + ")"
			
		NodeType.FUNCTION_CALL:
			var args_str = []
			for arg in arguments:
				args_str.append(arg.format_expression())
			return str(value) + "(" + ", ".join(args_str) + ")"
			
		NodeType.EQUATION:
			var left_str = left.format_expression() if left else ""
			var right_str = right.format_expression() if right else ""
			return left_str + " = " + right_str
			
		_:
			return "[Unknown Node Type]"

# Format node as Modelica code
func format_modelica() -> String:
	match type:
		NodeType.MODEL:
			var result = "model " + str(value) + "\n"
			for child in children:
				result += "  " + child.format_modelica().replace("\n", "\n  ")
			result += "end " + str(value) + ";"
			return result
			
		NodeType.CONNECTOR:
			var result = "connector " + str(value) + "\n"
			for child in children:
				result += "  " + child.format_modelica().replace("\n", "\n  ")
			result += "end " + str(value) + ";"
			return result
			
		NodeType.COMPONENT, NodeType.PARAMETER, NodeType.VARIABLE:
			var result = ""
			if variability:
				result += variability + " "
			if causality:
				result += causality + " "
			result += str(value)
			if not modifications.is_empty():
				result += "(" + _format_modifications() + ")"
			result += ";"
			return result
			
		_:
			return format_expression()

func _format_modifications() -> String:
	var mods = []
	for key in modifications:
		var value = modifications[key]
		if value is String:
			mods.append("%s=\"%s\"" % [key, value])
		else:
			mods.append("%s=%s" % [key, str(value)])
	return ", ".join(mods)

func _to_string() -> String:
	var result = "ASTNode(%s, '%s'" % [NodeType.keys()[type], str(value)]
	
	if is_differential:
		result += ", differential"
	if not state_variable.is_empty():
		result += ", state_var='%s'" % state_variable
	if not dependencies.is_empty():
		result += ", deps=%s" % str(dependencies)
	if not modifications.is_empty():
		result += ", mods=%s" % str(modifications)
	
	result += ")"
	return result 