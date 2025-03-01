extends BaseTest

const Parser = preload("res://apps/modelica/core/parser.gd")
const ASTNode = preload("res://apps/modelica/core/ast_node.gd")

var parser: Parser

func setup():
	parser = Parser.create_modelica_parser()

func test_arithmetic_expressions():
	var source = "a + b * c"
	var ast = parser.parse_expression(source)
	
	assert_not_null(ast, "AST should not be null")
	assert_equal(ast.type, ASTNode.NodeType.BINARY_OP, "Root should be a binary operation")
	assert_equal(ast.operator, "+", "Root operator should be +")
	
	# Left operand should be a variable reference
	assert_equal(ast.left.type, ASTNode.NodeType.VARIABLE_REF, "Left child should be a variable reference")
	assert_equal(ast.left.name, "a", "Left variable should be 'a'")
	
	# Right operand should be a binary operation
	assert_equal(ast.right.type, ASTNode.NodeType.BINARY_OP, "Right child should be a binary operation")
	assert_equal(ast.right.operator, "*", "Right operation should be *")
	assert_equal(ast.right.left.name, "b", "Left operand of * should be 'b'")
	assert_equal(ast.right.right.name, "c", "Right operand of * should be 'c'")

func test_function_call_expression():
	var source = "sin(x + y)"
	var ast = parser.parse_expression(source)
	
	assert_not_null(ast, "AST should not be null")
	assert_equal(ast.type, ASTNode.NodeType.FUNCTION_CALL, "Root should be a function call")
	assert_equal(ast.name, "sin", "Function name should be 'sin'")
	
	# Check argument
	assert_equal(ast.arguments.size(), 1, "Function should have 1 argument")
	assert_equal(ast.arguments[0].type, ASTNode.NodeType.BINARY_OP, "Argument should be a binary operation")
	assert_equal(ast.arguments[0].operator, "+", "Argument operation should be +")

func test_conditional_expression():
	var source = "if a > b then c else d"
	var ast = parser.parse_expression(source)
	
	assert_not_null(ast, "AST should not be null")
	assert_equal(ast.type, ASTNode.NodeType.CONDITIONAL, "Root should be a conditional")
	
	# Check condition
	assert_equal(ast.condition.type, ASTNode.NodeType.BINARY_OP, "Condition should be a binary operation")
	assert_equal(ast.condition.operator, ">", "Condition operator should be >")
	
	# Check then and else branches
	assert_equal(ast.then_branch.type, ASTNode.NodeType.VARIABLE_REF, "Then branch should be a variable reference")
	assert_equal(ast.then_branch.name, "c", "Then variable should be 'c'")
	assert_equal(ast.else_branch.type, ASTNode.NodeType.VARIABLE_REF, "Else branch should be a variable reference") 
	assert_equal(ast.else_branch.name, "d", "Else variable should be 'd'")

func test_parenthesized_expression():
	var source = "(a + b) * c"
	var ast = parser.parse_expression(source)
	
	assert_not_null(ast, "AST should not be null")
	assert_equal(ast.type, ASTNode.NodeType.BINARY_OP, "Root should be a binary operation")
	assert_equal(ast.operator, "*", "Root operator should be *")
	
	# Left operand should be a binary operation
	assert_equal(ast.left.type, ASTNode.NodeType.BINARY_OP, "Left child should be a binary operation")
	assert_equal(ast.left.operator, "+", "Left operation should be +")
	
	# Right operand should be a variable reference
	assert_equal(ast.right.type, ASTNode.NodeType.VARIABLE_REF, "Right child should be a variable reference")
	assert_equal(ast.right.name, "c", "Right variable should be 'c'")

func test_array_expression():
	var source = "{1, 2, 3 + 4}"
	var ast = parser.parse_expression(source)
	
	assert_not_null(ast, "AST should not be null")
	assert_equal(ast.type, ASTNode.NodeType.ARRAY, "Root should be an array")
	assert_equal(ast.values.size(), 3, "Array should have 3 elements")
	
	# Check elements
	assert_equal(ast.values[0].type, ASTNode.NodeType.LITERAL, "First element should be a literal")
	assert_equal(ast.values[0].value, 1, "First element should be 1")
	
	assert_equal(ast.values[1].type, ASTNode.NodeType.LITERAL, "Second element should be a literal")
	assert_equal(ast.values[1].value, 2, "Second element should be 2")
	
	assert_equal(ast.values[2].type, ASTNode.NodeType.BINARY_OP, "Third element should be a binary operation")
	assert_equal(ast.values[2].operator, "+", "Third element operator should be +")

func test_range_expression():
	var source = "1:2:10"
	var ast = parser.parse_expression(source)
	
	assert_not_null(ast, "AST should not be null")
	assert_equal(ast.type, ASTNode.NodeType.RANGE, "Root should be a range")
	
	# Check start, step, and end
	assert_equal(ast.start.type, ASTNode.NodeType.LITERAL, "Start should be a literal")
	assert_equal(ast.start.value, 1, "Start should be 1")
	
	assert_equal(ast.step.type, ASTNode.NodeType.LITERAL, "Step should be a literal")
	assert_equal(ast.step.value, 2, "Step should be 2")
	
	assert_equal(ast.end.type, ASTNode.NodeType.LITERAL, "End should be a literal")
	assert_equal(ast.end.value, 10, "End should be 10") 