@tool
extends SyntaxParser
class_name ModelicaParser

const NodeTypes = preload("res://apps/modelica/core/parser/ast/ast_node.gd").NodeType
const ModelicaTypeClass = preload("res://apps/modelica/core/parser/types/modelica_type.gd")

func _init() -> void:
	super._init(ModelicaLexer.new())
	print("NodeTypes.ROOT = ", NodeTypes.ROOT)  # Debug print

func _parse() -> ModelicaASTNode:
	var root = ModelicaASTNode.new(NodeTypes.ROOT, "", {"line": 1, "column": 1})
	var definitions = []
	
	while current_token and current_token.type != LexicalAnalyzer.TokenType.EOF:
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
	while current_token and current_token.type != LexicalAnalyzer.TokenType.EOF:
		if current_token.type == LexicalAnalyzer.TokenType.PUNCTUATION and current_token.value == ";":
			_advance()
			return
		if current_token.type == LexicalAnalyzer.TokenType.KEYWORD and current_token.value in ["model", "class", "connector", "package"]:
			return
		_advance()

func _parse_definition() -> ModelicaASTNode:
	if not current_token:
		return _error("Unexpected end of input")
	
	var def_type = ""
	var name = ""
	var start_location = {"line": current_token.line, "column": current_token.column}
	
	# Parse definition type
	if current_token.type == LexicalAnalyzer.TokenType.KEYWORD:
		def_type = current_token.value
		if not def_type in ["model", "connector", "package", "class"]:
			return _error("Invalid definition type: " + def_type)
		_advance() # consume type
	else:
		return _error("Expected model, connector, package, or class, got " + str(current_token.value))
	
	# Parse name
	if current_token and current_token.type == LexicalAnalyzer.TokenType.IDENTIFIER:
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
	var node = ModelicaASTNode.new(node_type, name, start_location)
	node.modelica_type = ModelicaTypeClass.new()
	node.modelica_type.kind = modelica_type_kind
	node.modelica_type.name = name
	
	# Parse components and equations
	while current_token and current_token.type != LexicalAnalyzer.TokenType.EOF:
		if current_token.type == LexicalAnalyzer.TokenType.KEYWORD:
			match current_token.value:
				"parameter", "constant", "input", "output", "Real", "Integer", "Boolean", "String":
					var component = _parse_component()
					if component:
						node.add_child(component)
					else:
						node.add_error("Failed to parse component", "syntax_error")
				"equation":
					_advance() # consume 'equation'
					while current_token and current_token.type != LexicalAnalyzer.TokenType.KEYWORD:
						var equation = _parse_equation()
						if equation:
							node.add_child(equation)
						else:
							node.add_error("Failed to parse equation", "syntax_error")
							break
				"end":
					_advance() # consume 'end'
					if current_token and current_token.type == LexicalAnalyzer.TokenType.IDENTIFIER:
						if current_token.value != name:
							node.add_error("Expected end " + name + " but got end " + current_token.value)
						_advance() # consume name
					else:
						node.add_error("Expected identifier after end")
					
					if current_token and current_token.type == LexicalAnalyzer.TokenType.PUNCTUATION and current_token.value == ";":
						_advance() # consume semicolon
						return node
					else:
						node.add_error("Expected semicolon after end " + name)
						return node
				_:
					node.add_error("Unexpected keyword: " + current_token.value)
					return node
		elif current_token.type == LexicalAnalyzer.TokenType.IDENTIFIER:
			var component = _parse_component()
			if component:
				node.add_child(component)
			else:
				node.add_error("Failed to parse component", "syntax_error")
		else:
			node.add_error("Unexpected token: " + str(current_token.value))
			_advance()
	
	return node

func _parse_component() -> ModelicaASTNode:
	var start_location = {"line": current_token.line, "column": current_token.column}
	var node = ModelicaASTNode.new(NodeTypes.COMPONENT, null, start_location)
	
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
	if not _expect(LexicalAnalyzer.TokenType.IDENTIFIER):
		return null
	
	var type_name = current_token.value
	var builtin_type = ModelicaTypeClass.get_builtin_type(type_name)
	if builtin_type:
		node.modelica_type = builtin_type
	else:
		node.add_error("Unknown type: " + type_name)
	
	_advance()
	
	# Component name
	if not _expect(LexicalAnalyzer.TokenType.IDENTIFIER):
		return null
	
	node.value = current_token.value
	_advance()
	
	# Optional array dimensions
	if current_token and current_token.type == LexicalAnalyzer.TokenType.PUNCTUATION and current_token.value == "[":
		_advance() # consume '['
		var dimensions = _parse_expression()
		if dimensions:
			if node.modelica_type:
				node.modelica_type = ModelicaTypeClass.create_array_type(node.modelica_type, [dimensions])
		if not _expect(LexicalAnalyzer.TokenType.PUNCTUATION, "]"):
			return null
	
	# Optional modification
	if current_token and current_token.type == LexicalAnalyzer.TokenType.PUNCTUATION and current_token.value == "=":
		_advance() # consume '='
		var modification = _parse_expression()
		if modification:
			node.modifications["value"] = modification
	
	if not _expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";"):
		return null
	
	return node

func _parse_equation() -> ModelicaASTNode:
	var start_location = {"line": current_token.line, "column": current_token.column}
	var node = ModelicaASTNode.new(NodeTypes.EQUATION, null, start_location)
	
	var left = _parse_expression()
	if not left:
		return null
	
	if not _expect(LexicalAnalyzer.TokenType.PUNCTUATION, "="):
		return null
	
	var right = _parse_expression()
	if not right:
		return null
	
	if not _expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";"):
		return null
	
	node.left = left
	node.right = right
	return node

func _parse_expression() -> ModelicaASTNode:
	if not current_token:
		return null
	
	var start_location = {"line": current_token.line, "column": current_token.column}
	var node = ModelicaASTNode.new(NodeTypes.UNKNOWN, null, start_location)
	
	match current_token.type:
		LexicalAnalyzer.TokenType.NUMBER:
			node.type = NodeTypes.NUMBER
			node.value = current_token.value
			# Infer type based on value
			if "." in str(current_token.value):
				node.modelica_type = ModelicaTypeClass.get_builtin_type("Real")
			else:
				node.modelica_type = ModelicaTypeClass.get_builtin_type("Integer")
			_advance()
		LexicalAnalyzer.TokenType.IDENTIFIER:
			node.type = NodeTypes.IDENTIFIER
			node.value = current_token.value
			_advance()
			# Handle function calls and array access
			if current_token and current_token.type == LexicalAnalyzer.TokenType.PUNCTUATION:
				match current_token.value:
					"(":
						node.type = NodeTypes.FUNCTION_CALL
						_advance() # consume '('
						while current_token and current_token.type != LexicalAnalyzer.TokenType.PUNCTUATION and current_token.value != ")":
							var arg = _parse_expression()
							if arg:
								node.arguments.append(arg)
							if current_token and current_token.value == ",":
								_advance()
						if not _expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")"):
							return null
					"[":
						node.type = NodeTypes.ARRAY_ACCESS
						_advance() # consume '['
						var index = _parse_expression()
						if index:
							node.arguments.append(index)
						if not _expect(LexicalAnalyzer.TokenType.PUNCTUATION, "]"):
							return null
		LexicalAnalyzer.TokenType.OPERATOR:
			node.type = NodeTypes.OPERATOR
			node.value = current_token.value
			_advance()
			node.operand = _parse_expression()
		_:
			node.type = NodeTypes.ERROR
			node.add_error("Unexpected token in expression: " + current_token.value)
			return node
	
	return node

func _error(message: String) -> ModelicaASTNode:
	var error_node = ModelicaASTNode.new(NodeTypes.ERROR, null, {"line": current_token.line if current_token else 0, "column": current_token.column if current_token else 0})
	error_node.add_error(message, "syntax_error")
	return error_node