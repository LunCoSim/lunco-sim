@tool
extends SyntaxParser
class_name ModelicaParser

const NodeTypes = preload("res://apps/modelica_godot/core/parser/ast/ast_node.gd").NodeType

var _current_visibility: String = "public"
var _current_package: String = ""

func _init() -> void:
	super._init(ModelicaLexer.new())

func parse(text: String) -> Dictionary:
	errors.clear()
	tokens = lexer.tokenize(text)
	position = 0
	current_token = _advance()
	
	var ast = _parse()
	return {
		"error": "\n".join(PackedStringArray(errors)) if not errors.is_empty() else "",
		"ast": _convert_ast_to_dict(ast) if ast else {}
	}

func _convert_ast_to_dict(ast: ModelicaASTNode) -> Dictionary:
	var result = {}
	
	if ast.type == NodeTypes.MODEL:
		result["classes"] = {}
		result["classes"][ast.value] = {
			"components": _extract_components(ast),
			"equations": _extract_equations(ast)
		}
	
	return result

func _extract_components(ast: ModelicaASTNode) -> Array:
	var components = []
	for child in ast.children:
		if child.type == NodeTypes.COMPONENT:
			components.append({
				"name": child.value,
				"type": child.metadata.get("type", ""),
				"variability": child.metadata.get("variability", ""),
				"value": child.metadata.get("value", null)
			})
	return components

func _extract_equations(ast: ModelicaASTNode) -> Array:
	var equations = []
	for child in ast.children:
		if child.type == NodeTypes.EQUATION:
			equations.append({
				"left": _equation_to_string(child.left),
				"right": _equation_to_string(child.right)
			})
	return equations

func _equation_to_string(node: ModelicaASTNode) -> String:
	if not node:
		return ""
	
	match node.type:
		NodeTypes.FUNCTION_CALL:
			if node.value == "der":
				return "der(" + _equation_to_string(node.arguments[0]) + ")"
			return str(node.value)
		NodeTypes.IDENTIFIER:
			return str(node.value)
		NodeTypes.NUMBER:
			return str(node.value)
		NodeTypes.OPERATOR:
			return _equation_to_string(node.left) + " " + str(node.value) + " " + _equation_to_string(node.right)
		_:
			return str(node.value)

func _create_node(type: int, value: Variant = null) -> ModelicaASTNode:
	var node = ModelicaASTNode.new()
	node.type = type
	node.value = value
	node.children = []
	node.metadata = {}
	return node

func _parse() -> ModelicaASTNode:
	# Parse optional within clause
	var within = _parse_within()
	
	# Parse imports
	var imports = []
	while _match_keyword("import"):
		imports.append(_parse_import())
	
	# Parse main definition
	var definition = _parse_definition()
	if definition:
		# Add within and imports as metadata
		if within:
			definition.add_metadata("within", within)
		if not imports.is_empty():
			definition.add_metadata("imports", imports)
	
	return definition

func _parse_within() -> String:
	if _match_keyword("within"):
		var name = _parse_qualified_name()
		_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
		return name
	return ""

func _parse_import() -> Dictionary:
	var import_info = {}
	
	# Parse import name
	var name = _parse_qualified_name()
	if name.is_empty():
		return {}
	
	import_info["name"] = name
	
	# Check for alias
	if _match(LexicalAnalyzer.TokenType.OPERATOR, "="):
		import_info["alias"] = _parse_identifier()
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	return import_info

func _parse_definition() -> ModelicaASTNode:
	var def_type = _parse_definition_type()
	if def_type.is_empty():
		return null
	
	var node_type = _get_node_type_for_definition(def_type)
	var name = _parse_identifier()
	var node = _create_node(node_type, name)
	
	# Parse description string if present
	if current_token and current_token.type == LexicalAnalyzer.TokenType.STRING:
		node.add_metadata("description", current_token.value)
		_advance()
	
	# Parse extends clauses
	while _match_keyword("extends"):
		var extends_info = _parse_extends()
		if not extends_info.is_empty():
			var extends_node = _create_node(NodeTypes.EXTENDS, extends_info.base_class)
			if extends_info.has("modifications"):
				extends_node.modifications = extends_info.modifications
			node.add_child(extends_node)
	
	# Parse body until 'end'
	while current_token and not _match_keyword("end"):
		# Handle visibility
		if _match_keyword("public"):
			_current_visibility = "public"
			continue
		elif _match_keyword("protected"):
			_current_visibility = "protected"
			continue
		
		# Parse component declarations and equations
		if _match_keyword("equation"):
			var equations = _parse_equation_section()
			for eq in equations:
				node.add_child(eq)
		else:
			var component = _parse_component()
			if component:
				node.add_child(component)
	
	# Parse end name and verify it matches
	var end_name = _parse_identifier()
	if end_name != name:
		_error("End name '%s' does not match definition name '%s'" % [end_name, name])
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	return node

func _parse_definition_type() -> String:
	for keyword in ModelicaLexer.MODELICA_KEYWORDS:
		if _match_keyword(keyword):
			return keyword
	return ""

func _get_node_type_for_definition(def_type: String) -> int:
	match def_type:
		"model": return NodeTypes.MODEL
		"connector": return NodeTypes.CONNECTOR
		"class": return NodeTypes.CLASS
		"record": return NodeTypes.CLASS
		"block": return NodeTypes.CLASS
		_: return NodeTypes.UNKNOWN

func _parse_extends() -> Dictionary:
	var extends_info = {}
	
	# Parse base class name
	extends_info["base_class"] = _parse_qualified_name()
	
	# Parse modifications if any
	if _match(LexicalAnalyzer.TokenType.PUNCTUATION, "("):
		extends_info["modifications"] = _parse_modifications()
		_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")")
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	return extends_info

func _parse_component() -> ModelicaASTNode:
	var node: ModelicaASTNode = null
	var variability = ""
	var causality = ""
	
	# Parse prefixes
	while true:
		if _match_keyword("parameter"):
			variability = "parameter"
		elif _match_keyword("constant"):
			variability = "constant"
		elif _match_keyword("input"):
			causality = "input"
		elif _match_keyword("output"):
			causality = "output"
		elif _match_keyword("flow"):
			causality = "flow"
		else:
			break
	
	# Parse type name
	var type_name = _parse_qualified_name()
	if type_name.is_empty():
		return null
	
	# Parse component name
	var name = _parse_identifier()
	if name.is_empty():
		return null
	
	# Create appropriate node
	if variability == "parameter":
		node = _create_node(NodeTypes.PARAMETER, name)
	else:
		node = _create_node(NodeTypes.COMPONENT, name)
	
	node.visibility = _current_visibility
	node.variability = variability
	node.causality = causality
	node.add_metadata("type", type_name)
	
	# Parse array dimensions if any
	if _match(LexicalAnalyzer.TokenType.PUNCTUATION, "["):
		var dimensions = _parse_array_dimensions()
		node.add_metadata("dimensions", dimensions)
		_expect(LexicalAnalyzer.TokenType.PUNCTUATION, "]")
	
	# Parse modifications
	if _match(LexicalAnalyzer.TokenType.PUNCTUATION, "("):
		node.modifications = _parse_modifications()
		_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")")
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	return node

func _parse_equation_section() -> Array[ModelicaASTNode]:
	var equations: Array[ModelicaASTNode] = []
	
	while current_token and not _is_section_keyword():
		var equation = _parse_equation()
		if equation:
			equations.append(equation)
	
	return equations

func _parse_equation() -> ModelicaASTNode:
	# Handle special equation types
	if _match_keyword("when"):
		return _parse_when_equation()
	elif _match_keyword("if"):
		return _parse_if_equation()
	elif _match_keyword("for"):
		return _parse_for_equation()
	elif _match_keyword("connect"):
		return _parse_connect_equation()
	
	# Parse simple equation
	var left = _parse_expression()
	if not left:
		return null
	
	_expect(LexicalAnalyzer.TokenType.OPERATOR, "=")
	
	var right = _parse_expression()
	if not right:
		return null
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	
	var node = _create_node(NodeTypes.EQUATION)
	node.left = left
	node.right = right
	return node

func _parse_expression() -> ModelicaASTNode:
	return _parse_binary_expression()

func _parse_binary_expression(precedence: int = 0) -> ModelicaASTNode:
	var left = _parse_unary_expression()
	
	while current_token and current_token.type == LexicalAnalyzer.TokenType.OPERATOR:
		var op_precedence = _get_operator_precedence(current_token.value)
		if op_precedence <= precedence:
			break
		
		var operator = current_token.value
		_advance()
		
		var right = _parse_binary_expression(op_precedence)
		
		var node = _create_node(NodeTypes.OPERATOR, operator)
		node.left = left
		node.right = right
		left = node
	
	return left

func _parse_unary_expression() -> ModelicaASTNode:
	if _match(LexicalAnalyzer.TokenType.OPERATOR, "-"):
		var node = _create_node(NodeTypes.OPERATOR, "-")
		node.operand = _parse_primary()
		return node
	
	return _parse_primary()

func _parse_primary() -> ModelicaASTNode:
	if current_token == null:
		return null
	
	match current_token.type:
		LexicalAnalyzer.TokenType.NUMBER:
			var node = _create_node(NodeTypes.NUMBER, float(current_token.value))
			_advance()
			return node
			
		LexicalAnalyzer.TokenType.IDENTIFIER:
			var name = current_token.value
			_advance()
			
			# Check for function call
			if _match(LexicalAnalyzer.TokenType.PUNCTUATION, "("):
				var node = _create_node(NodeTypes.FUNCTION_CALL, name)
				node.arguments = _parse_function_arguments()
				_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")")
				return node
			
			return _create_node(NodeTypes.IDENTIFIER, name)
			
		LexicalAnalyzer.TokenType.STRING:
			var node = _create_node(NodeTypes.STRING, current_token.value)
			_advance()
			return node
			
		LexicalAnalyzer.TokenType.PUNCTUATION:
			if current_token.value == "(":
				_advance()
				var expr = _parse_expression()
				_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")")
				return expr
	
	_error("Unexpected token in expression")
	return null

func _parse_qualified_name() -> String:
	var name = _parse_identifier()
	
	while _match(LexicalAnalyzer.TokenType.OPERATOR, "."):
		name += "."
		name += _parse_identifier()
	
	return name

func _parse_identifier() -> String:
	if current_token and current_token.type == LexicalAnalyzer.TokenType.IDENTIFIER:
		var name = current_token.value
		_advance()
		return name
	return ""

func _parse_modifications() -> Dictionary:
	var modifications = {}
	
	while current_token and current_token.type != LexicalAnalyzer.TokenType.PUNCTUATION:
		var name = _parse_identifier()
		if name.is_empty():
			break
		
		_expect(LexicalAnalyzer.TokenType.OPERATOR, "=")
		
		var value
		if current_token.type == LexicalAnalyzer.TokenType.STRING:
			value = current_token.value
			_advance()
		else:
			value = _parse_expression()
		
		modifications[name] = value
		
		if not _match(LexicalAnalyzer.TokenType.PUNCTUATION, ","):
			break
	
	return modifications

func _parse_array_dimensions() -> Array:
	var dimensions = []
	
	while current_token and current_token.type != LexicalAnalyzer.TokenType.PUNCTUATION:
		var expr = _parse_expression()
		if expr:
			dimensions.append(expr)
		
		if not _match(LexicalAnalyzer.TokenType.PUNCTUATION, ","):
			break
	
	return dimensions

func _parse_function_arguments() -> Array[ModelicaASTNode]:
	var arguments: Array[ModelicaASTNode] = []
	
	while current_token and current_token.type != LexicalAnalyzer.TokenType.PUNCTUATION:
		var arg = _parse_expression()
		if arg:
			arguments.append(arg)
		
		if not _match(LexicalAnalyzer.TokenType.PUNCTUATION, ","):
			break
	
	return arguments

func _match_keyword(keyword: String) -> bool:
	return current_token and current_token.type == LexicalAnalyzer.TokenType.KEYWORD and current_token.value == keyword

func _is_section_keyword() -> bool:
	return current_token and current_token.type == LexicalAnalyzer.TokenType.KEYWORD and current_token.value in [
		"equation", "algorithm", "protected", "public", "end"
	]

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

func _parse_when_equation() -> ModelicaASTNode:
	var node = _create_node(NodeTypes.WHEN_EQUATION)
	
	# Parse condition
	var condition = _parse_expression()
	if not condition:
		return null
	
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "then")
	
	# Parse equations
	while current_token and not _is_when_terminator():
		var equation = _parse_equation()
		if equation:
			node.add_child(equation)
	
	# Handle elsewhen clauses
	while _match_keyword("elsewhen"):
		var elsewhen = _create_node(NodeTypes.WHEN_EQUATION)
		
		# Parse condition
		condition = _parse_expression()
		if not condition:
			return null
		
		_expect(LexicalAnalyzer.TokenType.KEYWORD, "then")
		
		# Parse equations
		while current_token and not _is_when_terminator():
			var equation = _parse_equation()
			if equation:
				elsewhen.add_child(equation)
		
		node.add_child(elsewhen)
	
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "end")
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "when")
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	
	return node

func _parse_if_equation() -> ModelicaASTNode:
	var node = _create_node(NodeTypes.IF_EQUATION)
	
	# Parse condition
	var condition = _parse_expression()
	if not condition:
		return null
	
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "then")
	
	# Parse then branch
	var then_branch = _create_node(NodeTypes.EQUATION)
	while current_token and not _is_if_terminator():
		var equation = _parse_equation()
		if equation:
			then_branch.add_child(equation)
	node.add_child(then_branch)
	
	# Handle elseif branches
	while _match_keyword("elseif"):
		var elseif = _create_node(NodeTypes.IF_EQUATION)
		
		# Parse condition
		condition = _parse_expression()
		if not condition:
			return null
		
		_expect(LexicalAnalyzer.TokenType.KEYWORD, "then")
		
		# Parse equations
		while current_token and not _is_if_terminator():
			var equation = _parse_equation()
			if equation:
				elseif.add_child(equation)
		
		node.add_child(elseif)
	
	# Handle else branch
	if _match_keyword("else"):
		var else_branch = _create_node(NodeTypes.EQUATION)
		while current_token and not _is_if_terminator():
			var equation = _parse_equation()
			if equation:
				else_branch.add_child(equation)
		node.add_child(else_branch)
	
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "end")
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "if")
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	
	return node

func _parse_for_equation() -> ModelicaASTNode:
	var node = _create_node(NodeTypes.FOR_EQUATION)
	
	# Parse indices
	var indices = _parse_for_indices()
	node.add_metadata("indices", indices)
	
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "loop")
	
	# Parse equations
	while current_token and not _match_keyword("end"):
		var equation = _parse_equation()
		if equation:
			node.add_child(equation)
	
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "for")
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	
	return node

func _parse_connect_equation() -> ModelicaASTNode:
	var node = _create_node(NodeTypes.CONNECT_EQUATION)
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, "(")
	
	# Parse first connector reference
	var from_ref = _parse_component_reference()
	if not from_ref:
		return null
	node.add_metadata("from", from_ref)
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ",")
	
	# Parse second connector reference
	var to_ref = _parse_component_reference()
	if not to_ref:
		return null
	node.add_metadata("to", to_ref)
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")")
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	
	return node

func _parse_for_indices() -> Array:
	var indices = []
	
	while current_token:
		var index = {}
		
		# Parse index name
		var name = _parse_identifier()
		if name.is_empty():
			break
		index["name"] = name
		
		# Parse optional range
		if _match_keyword("in"):
			index["range"] = _parse_expression()
		
		indices.append(index)
		
		if not _match(LexicalAnalyzer.TokenType.PUNCTUATION, ","):
			break
	
	return indices

func _parse_component_reference() -> String:
	return _parse_qualified_name()

func _is_when_terminator() -> bool:
	return current_token and current_token.type == LexicalAnalyzer.TokenType.KEYWORD and current_token.value in [
		"elsewhen", "end"
	]

func _is_if_terminator() -> bool:
	return current_token and current_token.type == LexicalAnalyzer.TokenType.KEYWORD and current_token.value in [
		"elseif", "else", "end"
	] 