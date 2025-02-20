class_name ImprovedASTNode
extends Node

var type: String
var value: String
var left: ImprovedASTNode
var right: ImprovedASTNode
var operand: ImprovedASTNode
var arguments: Array[ImprovedASTNode]
var is_differential: bool = false
var state_variable: String = ""
var dependencies: Array[String] = []

func _init(type_: String = "", value_: String = ""):
	type = type_
	value = value_
	left = null
	right = null
	operand = null
	arguments = []
	dependencies = []

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

func get_dependencies() -> Array[String]:
	return dependencies

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