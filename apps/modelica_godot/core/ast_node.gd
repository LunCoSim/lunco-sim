class_name ImprovedASTNode
extends RefCounted

var type: String
var value: String
var dependencies: Array[String]
var is_differential: bool
var state_variable: String
var left: ImprovedASTNode
var right: ImprovedASTNode
var operand: ImprovedASTNode
var arguments: Array[ImprovedASTNode]

func _init(p_type: String, p_value: String = ""):
	type = p_type
	value = p_value
	dependencies = []
	is_differential = false
	state_variable = ""
	arguments = []

func add_dependency(var_name: String) -> void:
	if not dependencies.has(var_name):
		dependencies.append(var_name)

func get_dependencies() -> Array[String]:
	var all_deps: Array[String] = []
	all_deps.append_array(dependencies)
	
	if left:
		all_deps.append_array(left.get_dependencies())
	if right:
		all_deps.append_array(right.get_dependencies())
	if operand:
		all_deps.append_array(operand.get_dependencies())
	for arg in arguments:
		all_deps.append_array(arg.get_dependencies())
	
	var unique_deps: Array[String] = []
	for dep in all_deps:
		if not unique_deps.has(dep):
			unique_deps.append(dep)
	
	return unique_deps

func _to_string() -> String:
	match type:
		"NUMBER":
			return value
		"BINARY_OP":
			return "(%s %s %s)" % [left, value, right]
		"UNARY_OP":
			return "(%s%s)" % [value, operand]
		"VARIABLE":
			return value
		"DERIVATIVE":
			return "der(%s)" % state_variable
		"FUNCTION_CALL":
			var args_str = ""
			for arg in arguments:
				if args_str:
					args_str += ", "
				args_str += str(arg)
			return "%s(%s)" % [value, args_str]
	return "Node(%s, %s)" % [type, value] 