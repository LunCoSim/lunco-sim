@tool
extends Node
class_name EquationSystem

const ASTNodeClass = preload("res://apps/modelica_godot/core/system/ast_node.gd")
const ModelicaVariableClass = preload("res://apps/modelica_godot/core/modelica/modelica_variable.gd")

var variables: Dictionary = {}  # name -> ModelicaVariable
var equations: Array = []      # List of equation strings
var ast_nodes: Array[ASTNode] = []  # List of AST nodes for equations
var time: float = 0.0         # Current simulation time
var _parse_pos: int = 0       # Current position during parsing

func _init() -> void:
    process_mode = Node.PROCESS_MODE_ALWAYS

func add_variable(name: String, kind: ModelicaVariable.VariableKind = ModelicaVariable.VariableKind.REGULAR) -> ModelicaVariable:
    var var_obj = ModelicaVariable.new(name, kind)
    variables[name] = var_obj
    return var_obj

func add_equation(equation: String, ast: ASTNode = null) -> void:
    equations.append(equation)
    ast_nodes.append(ast)

func get_variable(name: String) -> ModelicaVariable:
    return variables.get(name)

func tokenize(expression: String) -> Array:
    var tokens = []
    var i = 0
    var expr = expression.strip_edges()
    
    while i < expr.length():
        var c = expr[i]
        
        # Skip whitespace
        if c == " " or c == "\t":
            i += 1
            continue
            
        # Numbers
        if c.is_valid_integer() or c == "." or c == "-":
            var num = ""
            var has_decimal = false
            
            if c == "-":
                num += c
                i += 1
                if i >= expr.length():
                    tokens.append(num)
                    break
                c = expr[i]
            
            while i < expr.length() and (c.is_valid_integer() or c == "."):
                if c == ".":
                    if has_decimal:
                        break
                    has_decimal = true
                num += c
                i += 1
                if i >= expr.length():
                    break
                c = expr[i]
            tokens.append(num)
            continue
            
        # Operators
        if c in ["+", "-", "*", "/", "^", "(", ")", "=", "<", ">", "!"]:
            if i + 1 < expr.length():
                var next_c = expr[i + 1]
                if (c + next_c) in ["<=", ">=", "==", "!="]:
                    tokens.append(c + next_c)
                    i += 2
                    continue
            tokens.append(c)
            i += 1
            continue
            
        # Variables and functions
        if c.is_valid_identifier():
            var name = ""
            while i < expr.length() and (c.is_valid_identifier() or c == "." or c == "_"):
                name += c
                i += 1
                if i >= expr.length():
                    break
                c = expr[i]
            tokens.append(name)
            continue
            
        # Unknown character
        push_error("Unknown character in expression: " + c)
        i += 1
    
    return tokens

func parse_expression(tokens: Array) -> ASTNode:
    _parse_pos = 0
    return _parse_expression(tokens)

func _parse_primary(tokens: Array) -> ASTNode:
    if _parse_pos >= tokens.size():
        return null
        
    var token = tokens[_parse_pos]
    
    if token.is_valid_float():
        _parse_pos += 1
        return ASTNodeClass.new(ASTNodeClass.NodeType.NUMBER, float(token))
        
    elif token == "(":
        _parse_pos += 1
        var expr = _parse_expression(tokens)
        if _parse_pos >= tokens.size() or tokens[_parse_pos] != ")":
            push_error("Expected closing parenthesis")
            return null
        _parse_pos += 1
        return expr
        
    elif token == "-":
        _parse_pos += 1
        var operand = _parse_primary(tokens)
        var node = ASTNodeClass.new(ASTNodeClass.NodeType.UNARY_OP, "-")
        node.operand = operand
        return node
        
    elif token.is_valid_identifier():
        _parse_pos += 1
        # Check if it's a function call
        if _parse_pos < tokens.size() and tokens[_parse_pos] == "(":
            var func_node = ASTNode.new(ASTNode.NodeType.FUNCTION_CALL, token)
            _parse_pos += 1
            while _parse_pos < tokens.size() and tokens[_parse_pos] != ")":
                var arg = _parse_expression(tokens)
                func_node.arguments.append(arg)
                if _parse_pos < tokens.size() and tokens[_parse_pos] == ",":
                    _parse_pos += 1
            if _parse_pos >= tokens.size() or tokens[_parse_pos] != ")":
                push_error("Expected closing parenthesis in function call")
                return null
            _parse_pos += 1
            return func_node
        else:
            return ASTNode.new(ASTNode.NodeType.VARIABLE, token)
    
    push_error("Unexpected token: " + token)
    return null

func _parse_term(tokens: Array) -> ASTNode:
    var left = _parse_primary(tokens)
    while _parse_pos < tokens.size() and tokens[_parse_pos] in ["*", "/"]:
        var op = tokens[_parse_pos]
        _parse_pos += 1
        var right = _parse_primary(tokens)
        var node = ASTNode.new(ASTNode.NodeType.BINARY_OP, op)
        node.left = left
        node.right = right
        left = node
    return left

func _parse_expression(tokens: Array) -> ASTNode:
    var left = _parse_term(tokens)
    while _parse_pos < tokens.size() and tokens[_parse_pos] in ["+", "-"]:
        var op = tokens[_parse_pos]
        _parse_pos += 1
        var right = _parse_term(tokens)
        var node = ASTNode.new(ASTNode.NodeType.BINARY_OP, op)
        node.left = left
        node.right = right
        left = node
    return left

func evaluate_ast(node: ASTNode) -> float:
    match node.type:
        ASTNode.NodeType.NUMBER:
            return float(node.value)
            
        ASTNode.NodeType.VARIABLE:
            var var_obj = get_variable(node.value)
            if var_obj != null:
                return float(var_obj.value)
            push_error("Unknown variable: " + str(node.value))
            return 0.0
            
        ASTNode.NodeType.BINARY_OP:
            var left = evaluate_ast(node.left)
            var right = evaluate_ast(node.right)
            match node.value:
                "+": return left + right
                "-": return left - right
                "*": return left * right
                "/": return left / right if right != 0 else INF
                "^": return pow(left, right)
                _: 
                    push_error("Unknown operator: " + str(node.value))
                    return 0.0
                    
        ASTNode.NodeType.UNARY_OP:
            var val = evaluate_ast(node.operand)
            match node.value:
                "-": return -val
                _:
                    push_error("Unknown unary operator: " + str(node.value))
                    return 0.0
                    
        ASTNode.NodeType.FUNCTION_CALL:
            match node.value:
                "sin": return sin(evaluate_ast(node.arguments[0]))
                "cos": return cos(evaluate_ast(node.arguments[0]))
                "tan": return tan(evaluate_ast(node.arguments[0]))
                "exp": return exp(evaluate_ast(node.arguments[0]))
                "log": return log(evaluate_ast(node.arguments[0]))
                "sqrt": return sqrt(evaluate_ast(node.arguments[0]))
                "abs": return abs(evaluate_ast(node.arguments[0]))
                "der":
                    # Derivative evaluation requires numerical methods
                    push_error("Derivative evaluation not implemented")
                    return 0.0
                _:
                    push_error("Unknown function: " + str(node.value))
                    return 0.0
    
    push_error("Unknown node type")
    return 0.0

func solve() -> bool:
    # Simple equation solver - needs to be expanded for more complex systems
    for i in range(equations.size()):
        var eq = equations[i]
        var ast = ast_nodes[i]
        
        if ast == null:
            # Parse equation if no AST provided
            var parts = eq.split("=")
            if parts.size() != 2:
                push_error("Invalid equation format: " + eq)
                return false
            
            var left_tokens = tokenize(parts[0])
            var right_tokens = tokenize(parts[1])
            
            var left_ast = parse_expression(left_tokens)
            var right_ast = parse_expression(right_tokens)
            
            if left_ast == null or right_ast == null:
                return false
            
            # Create equation node
            ast = ASTNode.new(ASTNode.NodeType.EQUATION, "=")
            ast.left = left_ast
            ast.right = right_ast
            ast_nodes[i] = ast
        
        # Evaluate equation
        var left_val = evaluate_ast(ast.left)
        var right_val = evaluate_ast(ast.right)
        
        # Update variables based on equation
        # This is a very simple solver that only works for basic equations
        # A more sophisticated solver would be needed for complex systems
        if ast.left.type == ASTNode.NodeType.VARIABLE:
            var var_obj = get_variable(ast.left.value)
            if var_obj != null:
                var_obj.set_value(right_val)
        elif ast.right.type == ASTNode.NodeType.VARIABLE:
            var var_obj = get_variable(ast.right.value)
            if var_obj != null:
                var_obj.set_value(left_val)
    
    return true

func solve_initialization() -> bool:
    # Initialize all variables that need initialization
    for var_name in variables:
        var var_obj = variables[var_name]
        if var_obj.is_state_variable():
            # Use start value for state variables
            var_obj.set_value(var_obj.start)
    
    return solve()  # Solve initial system

func _to_string() -> String:
    var result = "EquationSystem:\n"
    result += "  Time: %f\n" % time
    result += "  Variables:\n"
    for var_name in variables:
        result += "    %s = %s\n" % [var_name, str(variables[var_name].value)]
    result += "  Equations:\n"
    for i in range(equations.size()):
        result += "    %s\n" % equations[i]
    return result 