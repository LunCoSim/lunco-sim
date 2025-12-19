@tool
extends RefCounted
class_name ModelicaASTNode



enum NodeType {
	# Common nodes
	UNKNOWN,
	ERROR,
	ROOT,  # Root node representing the entire compilation unit
	
	# Type system nodes
	TYPE_DEFINITION,
	TYPE_REFERENCE,
	BUILTIN_TYPE,
	
	# Expression nodes
	NUMBER,
	IDENTIFIER,
	STRING,
	OPERATOR,
	FUNCTION_CALL,
	ARRAY_ACCESS,
	CONDITIONAL_EXPRESSION,  # For if-then-else expressions
	
	# Equation nodes
	EQUATION,
	DIFFERENTIAL_EQUATION,
	WHEN_EQUATION,
	IF_EQUATION,
	FOR_EQUATION,
	CONNECT_EQUATION,
	
	# Definition nodes
	MODEL,
	CONNECTOR,
	CLASS,
	PACKAGE,
	COMPONENT,
	PARAMETER,
	VARIABLE,
	EXTENDS,
	IMPORT,
	ANNOTATION
}

# Basic node properties
var type: NodeType = NodeType.UNKNOWN
var value: Variant  # The actual value/operator/function name
var children: Array[ModelicaASTNode] = []
var parent: ModelicaASTNode = null  # Parent node reference
var source_location := {"line": 0, "column": 0}  # Source code location

# Error handling
var errors: Array[Dictionary] = []
var has_errors: bool:
	get: return not errors.is_empty()

# Type system
var modelica_type = null  # Reference to type information (ModelicaTypeClass instance)
var is_type_checked: bool = false

# Scope and symbol management
var scope: Dictionary = {}  # Local symbol table
var qualified_name: String = ""  # Full path to this node (e.g. Modelica.Mechanics.Mass)

# Expression-specific fields
var left: ModelicaASTNode   # Left operand for binary ops
var right: ModelicaASTNode  # Right operand for binary ops
var operand: ModelicaASTNode # Single operand for unary ops
var arguments: Array[ModelicaASTNode] = [] # Arguments for function calls

# Modelica-specific fields
var visibility: String = "public"  # public, protected
var variability: String = ""  # parameter, constant, discrete
var causality: String = ""   # input, output
var modifications: Dictionary = {}

func _init(node_type: NodeType = NodeType.UNKNOWN, node_value: Variant = null, location: Dictionary = {}) -> void:
	type = node_type
	value = node_value
	if not location.is_empty():
		source_location = location

func add_child(node: ModelicaASTNode) -> void:
	if node:
		node.parent = self
		children.append(node)
		if node.has_errors:
			_propagate_errors(node)

func add_error(message: String, error_type: String = "error", location: Dictionary = {}) -> void:
	var error = {
		"message": message,
		"type": error_type,
		"location": location if not location.is_empty() else source_location
	}
	errors.append(error)
	if parent:
		parent._propagate_errors(self)

func _propagate_errors(from_node: ModelicaASTNode) -> void:
	for error in from_node.errors:
		if not errors.has(error):  # Avoid duplicates
			errors.append(error)
	if parent:
		parent._propagate_errors(self)

func get_root() -> ModelicaASTNode:
	if parent:
		return parent.get_root()
	return self

func find_child_by_name(name: String) -> ModelicaASTNode:
	for child in children:
		if child.value == name:
			return child
	return null

func get_full_name() -> String:
	if parent and parent.type != NodeType.ROOT:
		var parent_name = parent.get_full_name()
		return parent_name + "." + str(value) if parent_name else str(value)
	return str(value)

func is_definition() -> bool:
	return type in [NodeType.MODEL, NodeType.CONNECTOR, NodeType.CLASS, NodeType.PACKAGE]

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

func is_type() -> bool:
	return type in [
		NodeType.TYPE_DEFINITION,
		NodeType.TYPE_REFERENCE,
		NodeType.BUILTIN_TYPE
	]

func _to_string() -> String:
	var result = "ModelicaASTNode(%s, type=%s)" % [str(value), NodeType.keys()[type]]
	if has_errors:
		result += " [HAS ERRORS]"
	return result 
