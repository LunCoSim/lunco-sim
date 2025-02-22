class_name ImprovedASTNode
extends RefCounted

var type: String
var value: String
var left: ImprovedASTNode
var right: ImprovedASTNode
var operand: ImprovedASTNode
var arguments: Array
var dependencies: Array
var is_differential: bool
var state_variable: String

func _init(p_type: String = "", p_value: String = ""):
	type = p_type
	value = p_value
	left = null
	right = null
	operand = null
	arguments = []
	dependencies = []
	is_differential = false
	state_variable = ""

func get_dependencies() -> Array:
	var deps = []
	if type == "VARIABLE":
		deps.append(value)
	if left:
		deps.append_array(left.get_dependencies())
	if right:
		deps.append_array(right.get_dependencies())
	if operand:
		deps.append_array(operand.get_dependencies())
	for arg in arguments:
		deps.append_array(arg.get_dependencies())
	return deps

func add_dependency(var_name: String) -> void:
	if not dependencies.has(var_name):
		dependencies.append(var_name)

func collect_dependencies() -> void:
	# First collect direct dependencies from children
	if left != null:
		left.collect_dependencies()
		for dep in left.dependencies:
			if not dependencies.has(dep):
				dependencies.append(dep)
	
	if right != null:
		right.collect_dependencies()
		for dep in right.dependencies:
			if not dependencies.has(dep):
				dependencies.append(dep)
	
	if operand != null:
		operand.collect_dependencies()
		for dep in operand.dependencies:
			if not dependencies.has(dep):
				dependencies.append(dep)
	
	for arg in arguments:
		arg.collect_dependencies()
		for dep in arg.dependencies:
			if not dependencies.has(dep):
				dependencies.append(dep)

func _to_string() -> String:
	match type:
		"NUMBER":
			return value
		"VARIABLE":
			return value
		"BINARY_OP":
			return "(" + str(left) + " " + value + " " + str(right) + ")"
		"UNARY_OP":
			return value + "(" + str(operand) + ")"
		"FUNCTION_CALL":
			var args_str = ""
			for i in range(arguments.size()):
				if i > 0:
					args_str += ", "
				args_str += str(arguments[i])
			return value + "(" + args_str + ")"
		_:
			return "[Unknown AST Node]" 