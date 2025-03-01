@tool
extends RefCounted

# Import required dependencies
const LexerImpl = preload("res://apps/modelica/core/lexer.gd")
const ModelicaNode = preload("res://apps/modelica/core/ast_node.gd")

# Forward declare the node types for reference
const NodeTypes = preload("res://apps/modelica/core/ast_node.gd").NodeType
const ModelicaTypeClass = preload("res://apps/modelica/core/modelica_type.gd")

#-----------------------------------------------------------------------
# BASE SYNTAX PARSER
#-----------------------------------------------------------------------
class SyntaxParser:
	var lexer: LexerImpl
	var tokens: Array[LexerImpl.Token] = []
	var position: int = 0
	var current_token: LexerImpl.Token = null
	var errors: Array[String] = []

	func _init(p_lexer = null) -> void:
		lexer = p_lexer if p_lexer else LexerImpl.new()

	func parse(text: String) -> ModelicaNode:
		# Virtual method to be implemented by derived classes
		errors.clear()
		tokens = lexer.tokenize(text)
		position = 0
		current_token = _advance()
		
		var ast = _parse()
		
		if _has_errors():
			var error_str = ""
			for error in errors:
				if error_str.length() > 0:
					error_str += "\n"
				error_str += error
			
			var location = {
				"line": current_token.line if current_token else 0,
				"column": current_token.column if current_token else 0
			}
			var error_node = ModelicaNode.new(ModelicaNode.NodeType.ERROR, error_str, location)
			error_node.add_error(error_str, "syntax_error", location)
			return error_node
		
		return ast

	func _parse() -> ModelicaNode:
		# To be implemented by derived classes
		push_error("_parse() must be implemented by derived classes")
		return null

	func _advance() -> LexerImpl.Token:
		position += 1
		if position < tokens.size():
			current_token = tokens[position]
		else:
			current_token = null
		return current_token

	func _peek() -> LexerImpl.Token:
		var saved_pos = position
		var saved_token = current_token
		
		var next_token = _advance()
		
		position = saved_pos
		current_token = saved_token
		
		return next_token

	func _match(type: int, value: String = "") -> bool:
		if not current_token:
			return false
		
		if current_token.type == type:
			if value.is_empty() or current_token.value == value:
				_advance()
				return true
		return false

	func _expect(type: int, value: String = "") -> bool:
		if _match(type, value):
			return true
		
		var error = "Expected "
		if not value.is_empty():
			error += "'" + value + "'"
		else:
			error += str(type)
		error += " but got "
		if current_token:
			error += "'" + current_token.value + "'"
		else:
			error += "end of input"
		
		errors.append(error)
		return false

	func _error(message: String) -> ModelicaNode:
		var location = {
			"line": current_token.line if current_token else 0,
			"column": current_token.column if current_token else 0
		}
		var error_node = ModelicaNode.new(ModelicaNode.NodeType.ERROR, message, location)
		error_node.add_error(message, "syntax_error", location)
		return error_node

	func _token_type_to_string(type: int) -> String:
		return LexerImpl.TokenType.keys()[type]

	func _has_errors() -> bool:
		return not errors.is_empty()

	func get_errors() -> Array[String]:
		return errors

#-----------------------------------------------------------------------
# MODELICA PARSER
#-----------------------------------------------------------------------
class ModelicaParser extends SyntaxParser:
	func _init() -> void:
		super._init(LexerImpl.create_modelica_lexer())
		print("NodeTypes.ROOT = ", NodeTypes.ROOT)  # Debug print

	func _parse() -> ModelicaNode:
		var root = ModelicaNode.new(NodeTypes.ROOT, "", {"line": 1, "column": 1})
		var definitions = []
		
		while current_token and current_token.type != LexerImpl.TokenType.EOF:
			var definition = _parse_definition()
			if definition:
				definitions.append(definition)
			else:
				# Create error node with specific message
				var error_msg = "Failed to parse definition at line %d, column %d" % [current_token.line if current_token else 0, current_token.column if current_token else 0]
				var error_node = _error(error_msg)
				definitions.append(error_node)
				_synchronize()  # Try to recover
		
		# Add all definitions to root
		for def in definitions:
			root.add_child(def)
		
		if definitions.is_empty():
			root.add_error("No definitions found in the input", "syntax_error")
		
		return root

	func _synchronize() -> void:
		# Skip tokens until we find a synchronization point (like end of statement or new definition)
		while current_token and current_token.type != LexerImpl.TokenType.EOF:
			if current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == ";":
				_advance()
				return
			if current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value in ["model", "class", "connector", "package"]:
				return
			_advance()

	func _parse_definition() -> ModelicaNode:
		if not current_token:
			return _error("Unexpected end of input")
		
		var def_type = ""
		var name = ""
		var start_location = {"line": current_token.line, "column": current_token.column}
		
		# Parse definition type
		if current_token.type == LexerImpl.TokenType.KEYWORD:
			def_type = current_token.value
			if not def_type in ["model", "connector", "package", "class"]:
				return _error("Invalid definition type: " + def_type)
			_advance() # consume type
		else:
			return _error("Expected model, connector, package, or class, got " + str(current_token.value))
		
		# Parse name
		if current_token and current_token.type == LexerImpl.TokenType.IDENTIFIER:
			name = current_token.value
			start_location = {"line": current_token.line, "column": current_token.column}
			_advance() # consume name
		else:
			return _error("Expected identifier after " + def_type)
		
		var node_type = NodeTypes.UNKNOWN
		var modelica_type_kind = ModelicaTypeClass.TypeKind.UNKNOWN
		
		match def_type:
			"model":
				node_type = NodeTypes.MODEL
				modelica_type_kind = ModelicaTypeClass.TypeKind.MODEL
			"connector":
				node_type = NodeTypes.CONNECTOR
				modelica_type_kind = ModelicaTypeClass.TypeKind.CONNECTOR
			"package":
				node_type = NodeTypes.PACKAGE
				modelica_type_kind = ModelicaTypeClass.TypeKind.TYPE
			"class":
				node_type = NodeTypes.CLASS
				modelica_type_kind = ModelicaTypeClass.TypeKind.CLASS
		
		print("Creating node with type: ", node_type, " (", NodeTypes.keys()[node_type], ")")  # Debug print
		var node = ModelicaNode.new(node_type, name, start_location)
		node.modelica_type = ModelicaTypeClass.new()
		node.modelica_type.kind = modelica_type_kind
		node.modelica_type.name = name
		
		# Parse components and equations
		while current_token and current_token.type != LexerImpl.TokenType.EOF:
			if current_token.type == LexerImpl.TokenType.KEYWORD:
				match current_token.value:
					"parameter", "constant", "input", "output", "Real", "Integer", "Boolean", "String":
						var component = _parse_component()
						if component:
							node.add_child(component)
						else:
							node.add_error("Failed to parse component", "syntax_error")
					"equation":
						_advance() # consume 'equation'
						while current_token and current_token.type != LexerImpl.TokenType.KEYWORD:
							var equation = _parse_equation()
							if equation:
								node.add_child(equation)
							else:
								node.add_error("Failed to parse equation", "syntax_error")
								break
					"end":
						_advance() # consume 'end'
						if current_token and current_token.type == LexerImpl.TokenType.IDENTIFIER:
							if current_token.value != name:
								node.add_error("Expected end " + name + " but got end " + current_token.value)
							_advance() # consume name
						else:
							node.add_error("Expected identifier after end")
						
						if current_token and current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == ";":
							_advance() # consume semicolon
							return node
						else:
							node.add_error("Expected semicolon after end " + name)
							return node
					_:
						node.add_error("Unexpected keyword: " + current_token.value)
						return node
			elif current_token.type == LexerImpl.TokenType.IDENTIFIER:
				var component = _parse_component()
				if component:
					node.add_child(component)
				else:
					node.add_error("Failed to parse component", "syntax_error")
			else:
				node.add_error("Unexpected token: " + str(current_token.value))
				_advance()
		
		return node

	func _parse_component() -> ModelicaNode:
		var start_location = {"line": current_token.line, "column": current_token.column}
		var node = ModelicaNode.new(NodeTypes.COMPONENT, null, start_location)
		
		# Handle variability and causality
		match current_token.value:
			"parameter":
				node.variability = "parameter"
				_advance()
			"constant":
				node.variability = "constant"
				_advance()
			"input":
				node.causality = "input"
				_advance()
			"output":
				node.causality = "output"
				_advance()
		
		# Type name
		if not _expect(LexerImpl.TokenType.IDENTIFIER):
			return null
		
		var type_name = current_token.value
		var builtin_type = ModelicaTypeClass.get_builtin_type(type_name)
		if builtin_type:
			node.modelica_type = builtin_type
		else:
			node.add_error("Unknown type: " + type_name)
		
		_advance()
		
		# Component name
		if not _expect(LexerImpl.TokenType.IDENTIFIER):
			return null
		
		node.value = current_token.value
		_advance()
		
		# Optional array dimensions
		if current_token and current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == "[":
			_advance() # consume '['
			var dimensions = _parse_expression()
			if dimensions:
				if node.modelica_type:
					node.modelica_type = ModelicaTypeClass.create_array_type(node.modelica_type, [dimensions])
			if not _expect(LexerImpl.TokenType.PUNCTUATION, "]"):
				return null
		
		# Optional modification
		if current_token and current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == "=":
			_advance() # consume '='
			var modification = _parse_expression()
			if modification:
				node.modifications["value"] = modification
		
		if not _expect(LexerImpl.TokenType.PUNCTUATION, ";"):
			return null
		
		return node

	func _parse_equation() -> ModelicaNode:
		var start_location = {"line": current_token.line, "column": current_token.column}
		var node = ModelicaNode.new(NodeTypes.EQUATION, null, start_location)
		
		var left = _parse_expression()
		if not left:
			return null
		
		if not _expect(LexerImpl.TokenType.PUNCTUATION, "="):
			return null
		
		var right = _parse_expression()
		if not right:
			return null
		
		if not _expect(LexerImpl.TokenType.PUNCTUATION, ";"):
			return null
		
		node.left = left
		node.right = right
		return node

	func _parse_expression() -> ModelicaNode:
		if not current_token:
			return null
		
		var start_location = {"line": current_token.line, "column": current_token.column}
		var node = ModelicaNode.new(NodeTypes.UNKNOWN, null, start_location)
		
		match current_token.type:
			LexerImpl.TokenType.NUMBER:
				node.type = NodeTypes.NUMBER
				node.value = current_token.value
				# Infer type based on value
				if "." in str(current_token.value):
					node.modelica_type = ModelicaTypeClass.get_builtin_type("Real")
				else:
					node.modelica_type = ModelicaTypeClass.get_builtin_type("Integer")
				_advance()
			LexerImpl.TokenType.IDENTIFIER:
				node.type = NodeTypes.IDENTIFIER
				node.value = current_token.value
				_advance()
				# Handle function calls and array access
				if current_token and current_token.type == LexerImpl.TokenType.PUNCTUATION:
					match current_token.value:
						"(":
							node.type = NodeTypes.FUNCTION_CALL
							_advance() # consume '('
							while current_token and current_token.type != LexerImpl.TokenType.PUNCTUATION and current_token.value != ")":
								var arg = _parse_expression()
								if arg:
									node.arguments.append(arg)
								if current_token and current_token.value == ",":
									_advance()
							if not _expect(LexerImpl.TokenType.PUNCTUATION, ")"):
								return null
						"[":
							node.type = NodeTypes.ARRAY_ACCESS
							_advance() # consume '['
							var index = _parse_expression()
							if index:
								node.arguments.append(index)
							if not _expect(LexerImpl.TokenType.PUNCTUATION, "]"):
								return null
			LexerImpl.TokenType.OPERATOR:
				node.type = NodeTypes.OPERATOR
				node.value = current_token.value
				_advance()
				node.operand = _parse_expression()
			_:
				node.type = NodeTypes.ERROR
				node.add_error("Unexpected token in expression: " + current_token.value)
				return node
		
		return node

#-----------------------------------------------------------------------
# EQUATION PARSER
#-----------------------------------------------------------------------
class EquationParser extends SyntaxParser:
	func _init() -> void:
		super._init(LexerImpl.create_equation_lexer())

	func _parse() -> ModelicaNode:
		var left = _parse_expression()
		if not left:
			return null
		
		if not _match(LexerImpl.TokenType.OPERATOR, "="):
			return left  # Just an expression, not an equation
		
		var right = _parse_expression()
		if not right:
			return null
		
		var node = ModelicaNode.new(ModelicaNode.NodeType.EQUATION)
		node.left = left
		node.right = right
		
		# Check for differential equation
		if _is_derivative(left):
			node.type = ModelicaNode.NodeType.DIFFERENTIAL_EQUATION
			node.is_differential = true
			node.state_variable = _extract_state_variable(left)
		
		return node

	func _parse_expression() -> ModelicaNode:
		return _parse_binary_expression()

	func _parse_binary_expression(precedence: int = 0) -> ModelicaNode:
		var left = _parse_unary_expression()
		
		while current_token and current_token.type == LexerImpl.TokenType.OPERATOR:
			var op_precedence = _get_operator_precedence(current_token.value)
			if op_precedence <= precedence:
				break
			
			var operator = current_token.value
			_advance()
			
			var right = _parse_binary_expression(op_precedence)
			
			var node = ModelicaNode.new(ModelicaNode.NodeType.OPERATOR, operator)
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

	func _parse_unary_expression() -> ModelicaNode:
		if _match(LexerImpl.TokenType.OPERATOR, "-"):
			var node = ModelicaNode.new(ModelicaNode.NodeType.OPERATOR, "-")
			node.operand = _parse_primary()
			
			# Collect dependencies
			if node.operand and node.operand.is_expression():
				for dep in node.operand.get_dependencies():
					node.add_dependency(dep)
			
			return node
		
		return _parse_primary()

	func _parse_primary() -> ModelicaNode:
		if current_token == null:
			return null
		
		match current_token.type:
			LexerImpl.TokenType.NUMBER:
				var node = ModelicaNode.new(ModelicaNode.NodeType.NUMBER, float(current_token.value))
				_advance()
				return node
				
			LexerImpl.TokenType.IDENTIFIER:
				var name = current_token.value
				_advance()
				
				# Check for function call
				if _match(LexerImpl.TokenType.PUNCTUATION, "("):
					var node = ModelicaNode.new(ModelicaNode.NodeType.FUNCTION_CALL, name)
					node.arguments = _parse_function_arguments()
					_expect(LexerImpl.TokenType.PUNCTUATION, ")")
					
					# Handle derivative function
					if name == "der":
						node.is_differential = true
						if not node.arguments.is_empty():
							var arg = node.arguments[0]
							if arg.type == ModelicaNode.NodeType.IDENTIFIER:
								node.state_variable = arg.value
					
					# Collect dependencies from arguments
					for arg in node.arguments:
						if arg.is_expression():
							for dep in arg.get_dependencies():
								node.add_dependency(dep)
					
					return node
				
				# Regular variable reference
				var node = ModelicaNode.new(ModelicaNode.NodeType.IDENTIFIER, name)
				node.add_dependency(name)
				return node
				
			LexerImpl.TokenType.PUNCTUATION:
				if current_token.value == "(":
					_advance()
					var expr = _parse_expression()
					_expect(LexerImpl.TokenType.PUNCTUATION, ")")
					return expr
		
		_error("Unexpected token in expression")
		return null

	func _parse_function_arguments() -> Array[ModelicaNode]:
		var arguments: Array[ModelicaNode] = []
		
		while current_token and current_token.type != LexerImpl.TokenType.PUNCTUATION:
			var arg = _parse_expression()
			if arg:
				arguments.append(arg)
			
			if not _match(LexerImpl.TokenType.PUNCTUATION, ","):
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

	func _is_derivative(node: ModelicaNode) -> bool:
		return node.type == ModelicaNode.NodeType.FUNCTION_CALL and node.value == "der"

	func _extract_state_variable(node: ModelicaNode) -> String:
		if node.type == ModelicaNode.NodeType.FUNCTION_CALL and node.value == "der":
			if not node.arguments.is_empty():
				var arg = node.arguments[0]
				if arg.type == ModelicaNode.NodeType.IDENTIFIER:
					return arg.value
		return ""

# Factory functions to create parsers
static func create_modelica_parser() -> ModelicaParser:
	return ModelicaParser.new()

static func create_equation_parser() -> EquationParser:
	return EquationParser.new() 