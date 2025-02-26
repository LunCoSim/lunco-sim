@tool
extends SyntaxParser
class_name EquationParser

func _init() -> void:
	super._init(EquationLexer.new())

func _parse() -> ASTNode:
	var left = _parse_expression()
	if not left:
		return null
	
	if not _match(LexicalAnalyzer.TokenType.OPERATOR, "="):
		return left  # Just an expression, not an equation
	
	var right = _parse_expression()
	if not right:
		return null
	
	var node = ASTNode.new(ASTNode.NodeType.EQUATION)
	node.left = left
	node.right = right
	
	# Check for differential equation
	if _is_derivative(left):
		node.type = ASTNode.NodeType.DIFFERENTIAL_EQUATION
		node.is_differential = true
		node.state_variable = _extract_state_variable(left)
	
	return node

func _parse_expression() -> ASTNode:
	return _parse_binary_expression()

func _parse_binary_expression(precedence: int = 0) -> ASTNode:
	var left = _parse_unary_expression()
	
	while current_token and current_token.type == LexicalAnalyzer.TokenType.OPERATOR:
		var op_precedence = _get_operator_precedence(current_token.value)
		if op_precedence <= precedence:
			break
		
		var operator = current_token.value
		_advance()
		
		var right = _parse_binary_expression(op_precedence)
		
		var node = ASTNode.new(ASTNode.NodeType.OPERATOR, operator)
		node.left = left
		node.right = right
		
		# Collect dependencies
		if left.is_expression():
			for dep in left.get_dependencies():
				node.add_dependency(dep)
		if right.is_expression():
			for dep in right.get_dependencies():
				node.add_dependency(dep)
		
		left = node
	
	return left

func _parse_unary_expression() -> ASTNode:
	if _match(LexicalAnalyzer.TokenType.OPERATOR, "-"):
		var node = ASTNode.new(ASTNode.NodeType.OPERATOR, "-")
		node.operand = _parse_primary()
		
		# Collect dependencies
		if node.operand and node.operand.is_expression():
			for dep in node.operand.get_dependencies():
				node.add_dependency(dep)
		
		return node
	
	return _parse_primary()

func _parse_primary() -> ASTNode:
	if current_token == null:
		return null
	
	match current_token.type:
		LexicalAnalyzer.TokenType.NUMBER:
			var node = ASTNode.new(ASTNode.NodeType.NUMBER, float(current_token.value))
			_advance()
			return node
			
		LexicalAnalyzer.TokenType.IDENTIFIER:
			var name = current_token.value
			_advance()
			
			# Check for function call
			if _match(LexicalAnalyzer.TokenType.PUNCTUATION, "("):
				var node = ASTNode.new(ASTNode.NodeType.FUNCTION_CALL, name)
				node.arguments = _parse_function_arguments()
				_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")")
				
				# Handle derivative function
				if name == "der":
					node.is_differential = true
					if not node.arguments.is_empty():
						var arg = node.arguments[0]
						if arg.type == ASTNode.NodeType.IDENTIFIER:
							node.state_variable = arg.value
				
				# Collect dependencies from arguments
				for arg in node.arguments:
					if arg.is_expression():
						for dep in arg.get_dependencies():
							node.add_dependency(dep)
				
				return node
			
			# Regular variable reference
			var node = ASTNode.new(ASTNode.NodeType.IDENTIFIER, name)
			node.add_dependency(name)
			return node
			
		LexicalAnalyzer.TokenType.PUNCTUATION:
			if current_token.value == "(":
				_advance()
				var expr = _parse_expression()
				_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")")
				return expr
	
	_error("Unexpected token in expression")
	return null

func _parse_function_arguments() -> Array[ASTNode]:
	var arguments: Array[ASTNode] = []
	
	while current_token and current_token.type != LexicalAnalyzer.TokenType.PUNCTUATION:
		var arg = _parse_expression()
		if arg:
			arguments.append(arg)
		
		if not _match(LexicalAnalyzer.TokenType.PUNCTUATION, ","):
			break
	
	return arguments

func _get_operator_precedence(op: String) -> int:
	match op:
		"or": return 1
		"and": return 2
		"not": return 3
		"<", "<=", ">", ">=", "==", "<>": return 4
		"+", "-": return 5
		"*", "/": return 6
		"^": return 7
		_: return 0

func _is_derivative(node: ASTNode) -> bool:
	return node.type == ASTNode.NodeType.FUNCTION_CALL and node.value == "der"

func _extract_state_variable(node: ASTNode) -> String:
	if node.type == ASTNode.NodeType.FUNCTION_CALL and node.value == "der":
		if not node.arguments.is_empty():
			var arg = node.arguments[0]
			if arg.type == ASTNode.NodeType.IDENTIFIER:
				return arg.value
	return "" 