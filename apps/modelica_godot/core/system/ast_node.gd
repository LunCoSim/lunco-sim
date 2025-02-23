class_name ASTNode
extends RefCounted

enum NodeType {
    NUMBER,         # Numeric literal
    VARIABLE,       # Variable reference
    BINARY_OP,     # Binary operation (+, -, *, /, ^)
    UNARY_OP,      # Unary operation (-)
    FUNCTION_CALL, # Function call (sin, cos, etc.)
    EQUATION,      # Equation (=)
}

var type: NodeType
var value: Variant  # The actual value/operator/function name
var left: ASTNode   # Left operand for binary ops
var right: ASTNode  # Right operand for binary ops
var operand: ASTNode # Single operand for unary ops
var arguments: Array[ASTNode] # Arguments for function calls
var dependencies: Array[String]
var is_differential: bool = false
var state_variable: String = ""

func _init(node_type: NodeType, node_value: Variant) -> void:
    type = node_type
    value = node_value
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
    return type == NodeType.VARIABLE and state_variable != ""

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
    if type == NodeType.FUNCTION_CALL and value == "der" and arguments.size() > 0:
        var arg = arguments[0]
        if arg.type == NodeType.VARIABLE:
            set_state_variable(arg.value)
        elif arg.type == NodeType.BINARY_OP and arg.left and arg.left.type == NodeType.VARIABLE:
            set_state_variable(arg.left.value)

func to_string() -> String:
    match type:
        NodeType.NUMBER:
            return str(value)
            
        NodeType.VARIABLE:
            return str(value)
            
        NodeType.BINARY_OP:
            var left_str = left.to_string() if left else ""
            var right_str = right.to_string() if right else ""
            return "(" + left_str + " " + str(value) + " " + right_str + ")"
            
        NodeType.UNARY_OP:
            var operand_str = operand.to_string() if operand else ""
            return str(value) + "(" + operand_str + ")"
            
        NodeType.FUNCTION_CALL:
            var args_str = []
            for arg in arguments:
                args_str.append(arg.to_string())
            return str(value) + "(" + ", ".join(args_str) + ")"
            
        NodeType.EQUATION:
            var left_str = left.to_string() if left else ""
            var right_str = right.to_string() if right else ""
            return left_str + " = " + right_str
            
        _:
            return "[Unknown Node Type]"

func _to_string() -> String:
    return to_string() 