class_name ImprovedASTNode
extends RefCounted

var type: String
var value: String
var left: ImprovedASTNode
var right: ImprovedASTNode
var operand: ImprovedASTNode
var arguments: Array[ImprovedASTNode]
var dependencies: Array[String]
var is_differential: bool = false
var state_variable: String = ""

func _init(p_type: String, p_value: String) -> void:
	type = p_type
	value = p_value
	left = null
	right = null
	operand = null
	arguments = []
	dependencies = []
	is_differential = false
	state_variable = ""

func add_dependency(var_name: String) -> void:
	if not dependencies.has(var_name):
		dependencies.append(var_name)

func set_state_variable(var_name: String) -> void:
	state_variable = var_name
	is_differential = true
	add_dependency(var_name)

func is_state_variable() -> bool:
	return type == "VARIABLE" and state_variable != ""

func get_dependencies() -> Array[String]:
	var all_deps: Array[String] = []
	all_deps.append_array(dependencies)
	
	if left != null:
		all_deps.append_array(left.get_dependencies())
	if right != null:
		all_deps.append_array(right.get_dependencies())
	if operand != null:
		all_deps.append_array(operand.get_dependencies())
	for arg in arguments:
		all_deps.append_array(arg.get_dependencies())
	
	# Remove duplicates
	var unique_deps: Array[String] = []
	for dep in all_deps:
		if not unique_deps.has(dep):
			unique_deps.append(dep)
	
	return unique_deps

func mark_as_differential() -> void:
	is_differential = true
	# If this is a function call with arguments, try to extract state variable
	if type == "FUNCTION_CALL" and value == "der" and arguments.size() > 0:
		var arg = arguments[0]
		if arg.type == "VARIABLE":
			set_state_variable(arg.value)
		elif arg.type == "BINARY_OP" and arg.left and arg.left.type == "VARIABLE":
			set_state_variable(arg.left.value)

func _to_string() -> String:
	var result = "ASTNode(%s, '%s'" % [type, value]
	if is_differential:
		result += ", differential"
	if state_variable != "":
		result += ", state_var='%s'" % state_variable
	if dependencies.size() > 0:
		result += ", deps=%s" % str(dependencies)
	result += ")"
	return result 