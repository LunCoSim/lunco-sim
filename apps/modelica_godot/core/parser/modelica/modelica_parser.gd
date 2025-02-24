@tool
extends SyntaxParser
class_name ModelicaParser

const NodeTypes = ModelicaASTNode.NodeType

var _current_visibility: String = "public"
var _current_package: String = ""

func _init() -> void:
	super._init(ModelicaLexer.new())

func parse(text: String) -> ModelicaASTNode:
	print("\nStarting to parse text:")
	print("-------------------")
	print(text)
	print("-------------------")
	
	errors.clear()
	tokens = lexer.tokenize(text)
	position = 0
	current_token = _advance()
	
	print("\nStarting AST construction")
	var ast = _parse()
	
	print("\nParsing complete")
	if not errors.is_empty():
		print("Errors encountered:")
		for error in errors:
			print("- " + error)
	
	return ast

func _parse() -> ModelicaASTNode:
	print("\nParsing model structure")
	# Parse optional within clause
	var within = _parse_within()
	if within:
		print("Found within clause: " + within)
	
	# Parse optional import statements
	var imports = _parse_imports()
	if not imports.is_empty():
		print("Found imports: ", imports)
	
	# Parse the main definition
	print("Parsing main definition")
	var definition = _parse_definition()
	if definition:
		definition.metadata["within"] = within
		definition.metadata["imports"] = imports
		print("Successfully parsed definition of type: ", definition.type)
	else:
		print("Failed to parse definition")
	
	return definition

func _parse_within() -> String:
	if _match_keyword("within"):
		_current_package = _parse_identifier()
		_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
		return _current_package
	return ""

func _parse_imports() -> Array:
	var imports = []
	while _match_keyword("import"):
		var import_info = _parse_import()
		if not import_info.is_empty():
			imports.append(import_info)
	return imports

func _parse_definition() -> ModelicaASTNode:
	print("\nParsing definition")
	
	# Parse optional encapsulated keyword
	var is_encapsulated = _match_keyword("encapsulated")
	if is_encapsulated:
		print("Found encapsulated keyword")
	
	# Parse optional partial keyword
	var is_partial = _match_keyword("partial")
	if is_partial:
		print("Found partial keyword")
	
	# Get the definition type (model, class, etc.)
	var def_type = _parse_definition_type()
	if def_type.is_empty():
		print("No definition type found")
		return null
	
	# Get the definition name
	if not current_token:
		print("No token found for definition name")
		return null
		
	if current_token.type != LexicalAnalyzer.TokenType.IDENTIFIER:
		print("Expected identifier for definition name, got: " + current_token._to_string())
		return null
		
	var name = current_token.value
	print("Found definition name: " + name)
	_advance()
	
	var node_type = _get_node_type_for_definition(def_type)
	var node = _create_node(node_type, name)
	
	# Store encapsulated and partial flags in metadata
	if is_encapsulated:
		node.metadata["encapsulated"] = true
	if is_partial:
		node.metadata["partial"] = true
	
	# Parse description string if present
	if current_token and current_token.type == LexicalAnalyzer.TokenType.STRING:
		node.metadata["description"] = current_token.value
		print("Found description: " + current_token.value)
		_advance()
	
	# Parse extends clause if present
	if _match_keyword("extends"):
		print("Parsing extends clause")
		var extends_info = _parse_extends()
		if not extends_info.is_empty():
			var extends_node = _create_node(NodeTypes.EXTENDS, extends_info.base_class)
			if extends_info.has("modifications"):
				extends_node.metadata["modifications"] = extends_info.modifications
			node.children.append(extends_node)
			print("Added extends node: " + extends_info.base_class)
	
	print("Parsing components and equations")
	# Parse components and equations
	while current_token and not _match_keyword("end"):
		if current_token == null:
			print("ERROR: Unexpected end of tokens while parsing components")
			break
			
		print("Current token: ", current_token._to_string())
		
		if _match_keyword("equation"):
			print("Parsing equation section")
			var equations = _parse_equation_section()
			node.children.append_array(equations)
			print("Added %d equations" % equations.size())
		else:
			print("Parsing component")
			var component = _parse_component()
			if component:
				node.children.append(component)
	
	# Parse end name
	if current_token and current_token.type == LexicalAnalyzer.TokenType.IDENTIFIER:
		var end_name = current_token.value
		print("Found end name: " + end_name)
		if end_name != name:
			var error = "End name '%s' does not match definition name '%s'" % [end_name, name]
			print("ERROR: " + error)
			errors.append(error)
		_advance()
	
	if not _expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";"):
		print("ERROR: Missing semicolon after end")
		return null
	
	print("Completed parsing definition")
	return node

func _parse_definition_type() -> String:
	print("Looking for definition type")
	if not current_token:
		print("No current token")
		return ""
	
	var definition_types = ["model", "connector", "class", "record", "block", "type", "package", "function"]
	for def_type in definition_types:
		if _match(LexicalAnalyzer.TokenType.KEYWORD, def_type):
			print("Found definition type: " + def_type)
			return def_type
	
	print("Current token is not a valid definition type: " + current_token._to_string())
	return ""

func _get_node_type_for_definition(def_type: String) -> int:
	match def_type:
		"model": return NodeTypes.MODEL
		"connector": return NodeTypes.CONNECTOR
		"class": return NodeTypes.CLASS
		"record": return NodeTypes.CLASS
		"block": return NodeTypes.CLASS
		"type": return NodeTypes.CLASS
		"package": return NodeTypes.PACKAGE
		"function": return NodeTypes.CLASS
		_: return NodeTypes.UNKNOWN

func _parse_extends() -> Dictionary:
	var extends_info = {}
	extends_info.base_class = _parse_identifier()
	
	if _match(LexicalAnalyzer.TokenType.PUNCTUATION, "("):
		extends_info.modifications = _parse_modifications()
		_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")")
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	return extends_info

func _parse_component() -> ModelicaASTNode:
	print("Starting to parse component")
	var node: ModelicaASTNode = null
	var variability = ""
	var causality = ""
	
	# Check if we're at a definition keyword
	if current_token and current_token.type == LexicalAnalyzer.TokenType.KEYWORD:
		var keyword = current_token.value
		if keyword in ["model", "connector", "class", "record", "block", "equation", "end"]:
			print("Found definition keyword '%s', not a component" % keyword)
			return null
	
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
		print("No type name found")
		return null
	
	print("Found type name: " + type_name)
	
	# Parse component name
	var name = _parse_identifier()
	if name.is_empty():
		print("No component name found")
		return null
	
	print("Found component name: " + name)
	
	# Create appropriate node
	if variability == "parameter":
		node = _create_node(NodeTypes.PARAMETER, name)
	else:
		node = _create_node(NodeTypes.COMPONENT, name)
	
	node.metadata["type"] = type_name
	node.metadata["variability"] = variability
	node.metadata["causality"] = causality
	
	# Parse array dimensions if any
	if _match(LexicalAnalyzer.TokenType.PUNCTUATION, "["):
		var dimensions = _parse_array_dimensions()
		node.metadata["dimensions"] = dimensions
		_expect(LexicalAnalyzer.TokenType.PUNCTUATION, "]")
	
	# Parse modifications
	if _match(LexicalAnalyzer.TokenType.PUNCTUATION, "("):
		node.metadata["modifications"] = _parse_modifications()
		_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ")")
	
	if not _expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";"):
		print("Missing semicolon after component declaration")
		return null
	
	print("Successfully parsed component: " + name)
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
	return _match(LexicalAnalyzer.TokenType.KEYWORD, keyword)

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
			node.children.append(equation)
	
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
				elsewhen.children.append(equation)
		
		node.children.append(elsewhen)
	
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
			then_branch.children.append(equation)
	node.children.append(then_branch)
	
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
				elseif.children.append(equation)
		
		node.children.append(elseif)
	
	# Handle else branch
	if _match_keyword("else"):
		var else_branch = _create_node(NodeTypes.EQUATION)
		while current_token and not _is_if_terminator():
			var equation = _parse_equation()
			if equation:
				else_branch.children.append(equation)
		node.children.append(else_branch)
	
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "end")
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "if")
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	
	return node

func _parse_for_equation() -> ModelicaASTNode:
	var node = _create_node(NodeTypes.FOR_EQUATION)
	
	# Parse indices
	var indices = _parse_for_indices()
	node.metadata["indices"] = indices
	
	_expect(LexicalAnalyzer.TokenType.KEYWORD, "loop")
	
	# Parse equations
	while current_token and not _match_keyword("end"):
		var equation = _parse_equation()
		if equation:
			node.children.append(equation)
	
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
	node.metadata["from"] = from_ref
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ",")
	
	# Parse second connector reference
	var to_ref = _parse_component_reference()
	if not to_ref:
		return null
	node.metadata["to"] = to_ref
	
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

func _convert_ast_to_dict(ast: ModelicaASTNode) -> Dictionary:
	if ast == null:
		print("AST is null, returning empty result")
		return {"classes": {}}
		
	print("Converting AST node of type: ", ast.type)
	var result = {"classes": {}}
	
	match ast.type:
		NodeTypes.MODEL, NodeTypes.CLASS, NodeTypes.CONNECTOR:
			result["classes"][ast.value] = {
				"type": ast.type,
				"components": _extract_components(ast),
				"equations": _extract_equations(ast)
			}
		NodeTypes.PACKAGE:
			result["classes"][ast.value] = {
				"type": ast.type,
				"components": [],
				"equations": []
			}
			# Add nested classes if any
			for child in ast.children:
				if child.type in [NodeTypes.MODEL, NodeTypes.CLASS, NodeTypes.CONNECTOR, NodeTypes.PACKAGE]:
					var nested = _convert_ast_to_dict(child)
					result["classes"].merge(nested["classes"])
	
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
	return node

func _parse_import() -> Dictionary:
	var import_info = {}
	
	# Parse import name
	var name = _parse_identifier()
	if name.is_empty():
		return {}
	
	import_info["name"] = name
	
	# Check for alias
	if _match(LexicalAnalyzer.TokenType.OPERATOR, "="):
		import_info["alias"] = _parse_identifier()
	
	_expect(LexicalAnalyzer.TokenType.PUNCTUATION, ";")
	return import_info 