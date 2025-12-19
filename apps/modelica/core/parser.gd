@tool
extends RefCounted

# Import required dependencies
const LexerImpl = preload("res://apps/modelica/core/lexer.gd")
const ModelicaNode = preload("res://apps/modelica/core/ast_node.gd")

# Forward declare the node types for reference
const NodeTypes = preload("res://apps/modelica/core/ast_node.gd").NodeType


# Helper function to parse a file directly
func parse_file(file_path: String) -> ModelicaNode:
	if not FileAccess.file_exists(file_path):
		push_error("File does not exist: " + file_path)
		return null
		
	var file = FileAccess.open(file_path, FileAccess.READ)
	if not file:
		push_error("Failed to open file: " + file_path)
		return null
		
	var content = file.get_as_text()
	file.close()
	
	# Create a parser and parse the content
	var parser = ModelicaParser.new()
	return parser.parse(content)

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

	func _peek(offset: int = 1) -> LexerImpl.Token:
		var peek_pos = position + offset
		if peek_pos >= 0 and peek_pos < tokens.size():
			return tokens[peek_pos]
		return null

	func _match(token_type, token_value = null) -> bool:
		if not current_token:
			return false
		
		if current_token.type != token_type:
			return false
			
		if token_value != null and current_token.value != token_value:
			return false
			
		return true

	func _expect(token_type, token_value = null, error_message: String = "") -> bool:
		if _match(token_type, token_value):
			_advance()
			return true
		
		if error_message.is_empty():
			if token_value != null:
				error_message = "Expected %s with value '%s'" % [str(token_type), str(token_value)]
			else:
				error_message = "Expected %s" % str(token_type)
		
		_add_error(error_message)
		return false

	func _add_error(error_message: String) -> void:
		var pos_str = ""
		if current_token:
			pos_str = " at line %d, column %d" % [current_token.line, current_token.column]
		errors.append(error_message + pos_str)

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
		return errors.size() > 0

	func get_errors() -> Array[String]:
		return errors

	# Helper to get the location dictionary from a token
	func get_token_location(token: LexerImpl.Token) -> Dictionary:
		if not token:
			return {"line": 0, "column": 0}
		return {"line": token.line, "column": token.column}

#-----------------------------------------------------------------------
# EQUATION PARSER
#-----------------------------------------------------------------------
class EquationParser extends SyntaxParser:
	func _init() -> void:
		super._init(LexerImpl.create_modelica_lexer())
		
	# TO BE IMPLEMENTED
	func _parse() -> ModelicaNode:
		# Create an empty node for now
		return ModelicaNode.new(NodeTypes.EQUATION, "", {"line": 0, "column": 0})

#-----------------------------------------------------------------------
# MODELICA PARSER
#-----------------------------------------------------------------------
class ModelicaParser extends SyntaxParser:
	func _init() -> void:
		super._init(LexerImpl.create_modelica_lexer())
		print("NodeTypes.ROOT = ", NodeTypes.ROOT)  # Debug print

	# Helper to create error nodes
	func _create_error_node(error_msg: String, location: Dictionary) -> ModelicaNode:
		print("Error: " + error_msg)
		var error_node = ModelicaNode.new(NodeTypes.ERROR, error_msg, location)
		error_node.add_error(error_msg, "syntax_error")
		return error_node

	# Main parsing entry point for Modelica code
	func _parse() -> ModelicaNode:
		var start_token = current_token
		var start_loc = get_token_location(start_token)
		
		print("Starting parse with token: type=" + str(current_token.type) + ", value='" + current_token.value + "'")
		
		# Reset position to beginning to ensure we don't miss anything
		position = 0
		current_token = tokens[position]
		
		# Look for 'within' statement first
		var within_package = ""
		
		# First token could be the 'within' keyword
		if current_token and current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value == "within":
			_advance() # Consume 'within' keyword
			
			# If there's an identifier after 'within', it's a package path
			if current_token and current_token.type == LexerImpl.TokenType.IDENTIFIER:
				var path_parts = []
				
				# Collect all parts of the package path (identifiers separated by dots)
				while current_token and current_token.type == LexerImpl.TokenType.IDENTIFIER:
					path_parts.append(current_token.value)
					_advance() # Consume identifier
					
					# Check for dot separator
					if current_token and current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value == ".":
						_advance() # Consume dot
					else:
						break
				
				within_package = ".".join(path_parts)
				print("Found within package: " + within_package)
			
			# Expect semicolon after within
			if current_token and current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value == ";":
				_advance() # Consume semicolon
			else:
				print("Warning: Expected ';' after within statement")
		
		# Skip whitespace and comments at the beginning of the file
		while current_token and (
			current_token.type == LexerImpl.TokenType.WHITESPACE or
			current_token.type == LexerImpl.TokenType.COMMENT or
			current_token.type == LexerImpl.TokenType.NEWLINE
		):
			_advance()
		
		print("After skipping initial whitespace: token type=" + str(current_token.type) + ", value='" + current_token.value + "'")
		
		# Check if we have a model keyword
		if current_token and current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value.to_lower() == "model":
			print("Found model keyword!")
			var model_node = _parse_model()
			if model_node:
				# Set qualified name based on the within package
				if not within_package.is_empty():
					model_node.qualified_name = within_package + "." + model_node.value
				else:
					model_node.qualified_name = model_node.value
				
				print("Model node created with type: " + str(model_node.type) + ", qualified name: " + model_node.qualified_name)
				return model_node
		elif current_token:
			# Look ahead to find model keyword
			var saved_position = position
			var saved_token = current_token
			var max_lookahead = 10
			var i = 0
			
			print("Model keyword not found at start, searching ahead...")
			while i < max_lookahead and position < tokens.size():
				if current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value.to_lower() == "model":
					print("Found model keyword at position " + str(position))
					var model_node = _parse_model()
					if model_node:
						return model_node
					break
				_advance()
				i += 1
			
			# Reset position if model not found
			position = saved_position
			current_token = saved_token
			
			# Try to recover by parsing it as a model anyway if we have an identifier
			if current_token.type == LexerImpl.TokenType.IDENTIFIER:
				print("Attempting to parse as model starting with identifier: " + current_token.value)
				var error_node = ModelicaNode.new(NodeTypes.MODEL, current_token.value, start_loc)
				error_node.add_error("Expected model keyword before model name", "syntax_error", start_loc)
				
				_advance() # Consume the identifier
				
				# Continue parsing the rest of the model
				_parse_model_body(error_node)
				
				return error_node
			
			var error_msg = "Expected 'model' keyword, found: " + str(current_token.type) + " - " + current_token.value
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		else:
			var error_msg = "Unexpected end of file"
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
		return ModelicaNode.new(NodeTypes.ERROR, "Failed to parse model", start_loc)

	# Parse a Modelica model definition - improved version
	func _parse_model() -> ModelicaNode:
		print("Parsing model definition")
		var start_loc = get_token_location(current_token)
		
		# Consume the 'model' keyword
		_advance()
		
		# Skip any whitespace after the model keyword
		while current_token and (
			current_token.type == LexerImpl.TokenType.WHITESPACE or
			current_token.type == LexerImpl.TokenType.COMMENT or
			current_token.type == LexerImpl.TokenType.NEWLINE
		):
			_advance()
		
		# Get the model name
		if not current_token or current_token.type != LexerImpl.TokenType.IDENTIFIER:
			var error_msg = "Expected model name after 'model' keyword"
			if current_token:
				error_msg += ", got " + str(current_token.type) + " (" + current_token.value + ")"
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
		var name = current_token.value
		print("Model name: " + name)
		
		# Create the model node with the correct type and name
		var model_node = ModelicaNode.new(NodeTypes.MODEL, name, start_loc)
		print("Created model node with type:", model_node.type, " and name:", model_node.value)
		
		# Consume model name
		_advance()
		
		# Parse the model body
		return _parse_model_body(model_node)
	
	# Parse the body of a model - extracted to a separate method for reuse
	func _parse_model_body(model_node: ModelicaNode) -> ModelicaNode:
		# Parse model body (parameters, variables, equations, etc.)
		while current_token and current_token.type != LexerImpl.TokenType.EOF:
			var token_type = current_token.type
			var token_value = current_token.value
			
			# End of model
			if token_type == LexerImpl.TokenType.KEYWORD and token_value == "end":
				_advance()  # Consume 'end'
				
				# Check if model name is repeated after 'end'
				if current_token and current_token.type == LexerImpl.TokenType.IDENTIFIER and current_token.value == model_node.value:
					_advance()  # Consume model name
				
				# Expect semicolon after end statement
				if current_token and current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value == ";":
					_advance()  # Consume semicolon
				
				break  # End of model definition
			
			# Parameter declaration
			elif token_type == LexerImpl.TokenType.KEYWORD and token_value == "parameter":
				var param_node = _parse_parameter()
				if param_node:
					model_node.add_child(param_node)
			
			# Variable declaration
			elif token_type == LexerImpl.TokenType.IDENTIFIER:
				if token_value in ["Real", "Integer", "Boolean", "String"]:
					var var_node = _parse_variable()
					if var_node:
						model_node.add_child(var_node)
				else:
					print("Unexpected identifier in model body: " + token_value)
					_advance()  # Skip unknown token
			
			# Initial equation section
			elif token_type == LexerImpl.TokenType.KEYWORD and token_value == "initial":
				_advance()  # Consume 'initial'
				
				if current_token and current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value == "equation":
					var eq_section = _parse_equation(true)  # true for initial
					if eq_section:
						model_node.add_child(eq_section)
			
			# Regular equation section
			elif token_type == LexerImpl.TokenType.KEYWORD and token_value == "equation":
				var eq_section = _parse_equation(false)  # false for regular
				if eq_section:
					model_node.add_child(eq_section)
			
			# Skip unknown or whitespace tokens
			else:
				print("Skipping token in model body: " + str(token_type) + " - " + str(token_value))
				_advance()
		
		return model_node

	# Parse a parameter declaration
	func _parse_parameter() -> ModelicaNode:
		print("Parsing parameter")
		var start_loc = get_token_location(current_token)
		
		# Consume "parameter" keyword
		_advance()
		
		# Get parameter type (Real, Integer, etc.)
		if current_token.type != LexerImpl.TokenType.IDENTIFIER:
			var error_msg = "Expected type after 'parameter' keyword"
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
		var param_type = current_token.value
		_advance()  # Consume type
		
		# Get parameter name
		if current_token.type != LexerImpl.TokenType.IDENTIFIER:
			var error_msg = "Expected parameter name"
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
		var param_name = current_token.value
		var param_node = ModelicaNode.new(NodeTypes.PARAMETER, param_name, start_loc)
		
		# Add type information
		var type_node = ModelicaNode.new(NodeTypes.TYPE_REFERENCE, param_type, start_loc)
		param_node.add_child(type_node)
		
		_advance()  # Consume parameter name
		
		# Check for initialization
		if current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == "=":
			_advance()  # Consume equals sign
			
			# Get parameter value
			if current_token.type == LexerImpl.TokenType.NUMBER:
				var value_node = ModelicaNode.new(NodeTypes.NUMBER, current_token.value, 
					get_token_location(current_token))
				param_node.add_child(value_node)
				_advance()  # Consume value
			else:
				var error_msg = "Expected number for parameter value"
				print("Error: " + error_msg)
				param_node.add_error(error_msg, "syntax_error")
		
		# Check for description string
		if current_token.type == LexerImpl.TokenType.STRING:
			var comment_node = ModelicaNode.new(NodeTypes.STRING, current_token.value, 
				get_token_location(current_token))
			param_node.add_child(comment_node)
			_advance()  # Consume string
		
		# Check for semicolon
		if current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == ";":
			_advance()  # Consume semicolon
		else:
			var error_msg = "Expected ';' after parameter declaration"
			print("Error: " + error_msg)
			param_node.add_error(error_msg, "syntax_error")
			
			# Skip until we find a semicolon or a new token that might start a new declaration
			while current_token and not (
				current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == ";" or
				current_token.type == LexerImpl.TokenType.KEYWORD or
				current_token.type == LexerImpl.TokenType.IDENTIFIER and _is_type_name(current_token.value) or
				current_token.value == "equation" or current_token.value == "end"
			):
				print("Skipping token in model body: " + str(current_token.type) + " - " + str(current_token.value))
				_advance()
			
			# If we found a semicolon, consume it
			if current_token and current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == ";":
				_advance()
			
		return param_node
		
	# Parse a variable declaration
	func _parse_variable() -> ModelicaNode:
		print("Parsing variable")
		var start_loc = get_token_location(current_token)
		
		# Get variable type (Real, Integer, etc.)
		var var_type = current_token.value
		_advance()  # Consume type
		
		# Get variable name
		if not current_token or current_token.type != LexerImpl.TokenType.IDENTIFIER:
			var error_msg = "Expected variable name"
			if current_token:
				error_msg += ", got " + str(current_token.type) + " (" + current_token.value + ")"
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
		var var_name = current_token.value
		var var_node = ModelicaNode.new(NodeTypes.VARIABLE, var_name, start_loc)
		
		# Add type information
		var type_node = ModelicaNode.new(NodeTypes.TYPE_REFERENCE, var_type, start_loc)
		var_node.add_child(type_node)
		
		_advance()  # Consume variable name
		
		# Check for description string
		if current_token and current_token.type == LexerImpl.TokenType.STRING:
			var comment_node = ModelicaNode.new(NodeTypes.STRING, current_token.value, 
				get_token_location(current_token))
			var_node.add_child(comment_node)
			_advance()  # Consume string
		
		# Check for semicolon
		if current_token and (
			(current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == ";") or
			(current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value == ";")
		):
			_advance()  # Consume semicolon
		else:
			var error_msg = "Expected ';' after variable declaration"
			print("Error: " + error_msg)
			var_node.add_error(error_msg, "syntax_error")
			
		return var_node
		
	# Parse an equation section (initial or regular)
	func _parse_equation(is_initial: bool) -> ModelicaNode:
		print("Parsing equation section (initial=" + str(is_initial) + ")")
		var start_loc = get_token_location(current_token)
		
		# Create equation section node
		var eq_section
		if is_initial:
			eq_section = ModelicaNode.new(NodeTypes.EQUATION, "initial", start_loc)
		else:
			eq_section = ModelicaNode.new(NodeTypes.EQUATION, "section", start_loc)
		
		# Consume "equation" keyword
		_advance()
		
		# Skip any whitespace after the equation keyword
		while current_token and (
			current_token.type == LexerImpl.TokenType.WHITESPACE or
			current_token.type == LexerImpl.TokenType.COMMENT or
			current_token.type == LexerImpl.TokenType.NEWLINE
		):
			_advance()
		
		# Parse equations until we reach end, equation, or another keyword
		while current_token and current_token.type != LexerImpl.TokenType.EOF:
			if current_token.type == LexerImpl.TokenType.KEYWORD:
				if current_token.value in ["end", "equation", "algorithm", "initial"]:
					break  # End of equation section
			
			# Skip any whitespace before equations
			while current_token and (
				current_token.type == LexerImpl.TokenType.WHITESPACE or
				current_token.type == LexerImpl.TokenType.COMMENT or
				current_token.type == LexerImpl.TokenType.NEWLINE
			):
				_advance()
				
			# Check if we've reached a section end
			if not current_token or current_token.type == LexerImpl.TokenType.EOF:
				break
			if current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value in ["end", "equation", "algorithm", "initial"]:
				break
			
			# Parse a single equation
			var eq_node = _parse_single_equation(is_initial)
			if eq_node:
				eq_section.add_child(eq_node)
			else:
				print("Failed to parse equation, skipping to next semicolon")
				# Skip to next equation (after semicolon)
				while current_token and current_token.value != ";":
					_advance()
				if current_token and current_token.value == ";":
					_advance()  # Consume semicolon
		
		return eq_section

	# Parse a single equation
	func _parse_single_equation(initial: bool = false) -> ModelicaNode:
		print("Parsing single equation")
		var start_loc = get_token_location(current_token)
		
		# Skip comments before equation
		while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
			print("Skipping comment before equation: " + current_token.value)
			_advance()
		
		# Parse left-hand side expression
		var lhs = _parse_expression()
		if not lhs:
			print("Failed to parse left-hand side of equation")
			var error_node = ModelicaNode.new(NodeTypes.ERROR, "Failed to parse equation", start_loc)
			
			# Skip to semicolon to recover
			_recover_to_semicolon()
			
			return error_node
		
		# Skip comments before the equals sign
		while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
			print("Skipping comment before equals sign: " + current_token.value)
			_advance()
		
		# Expect equals sign
		if not current_token or (
			current_token.value != "=" or 
			(current_token.type != LexerImpl.TokenType.PUNCTUATION and 
			 current_token.type != LexerImpl.TokenType.OPERATOR)
		):
			var error_msg = "Expected '=' in equation"
			if current_token:
				error_msg += ", got " + str(current_token.type) + " (" + current_token.value + ")"
			print("Error: " + error_msg)
			var error_node = ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
			error_node.add_child(lhs)  # Attach the left-hand side for context
			
			# Skip to semicolon to recover
			_recover_to_semicolon()
			
			return error_node
		
		_advance()  # Consume equals sign
		
		# Skip comments after the equals sign
		while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
			print("Skipping comment after equals sign: " + current_token.value)
			_advance()
		
		# Parse right-hand side expression
		var rhs = _parse_expression()
		if not rhs:
			print("Failed to parse right-hand side of equation")
			var error_msg = "Failed to parse right side of equation"
			var error_node = ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
			error_node.add_child(lhs)  # Attach the left-hand side for context
			
			# Skip to semicolon to recover
			_recover_to_semicolon()
			
			return error_node
		
		# Create equation node
		var eq_node = ModelicaNode.new(NodeTypes.EQUATION, "=", start_loc)
		eq_node.add_child(lhs)
		eq_node.add_child(rhs)
		
		# Skip comments before the semicolon
		while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
			print("Skipping comment before semicolon: " + current_token.value)
			_advance()
		
		# Expect semicolon
		if current_token and (
			(current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == ";") or
			(current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value == ";")
		):
			_advance()  # Consume semicolon
		else:
			var error_msg = "Expected ';' after equation"
			if current_token:
				error_msg += ", got " + str(current_token.type) + " (" + current_token.value + ")"
			print("Error: " + error_msg)
			eq_node.add_error(error_msg, "syntax_error")
			
			# Attempt recovery - advance until we find a semicolon or something that looks like the end of this equation
			_recover_to_semicolon()
		
		return eq_node

	# Helper method to recover from syntax errors by advancing to the next semicolon or equation boundary
	func _recover_to_semicolon():
		var recovery_limit = 10 # Limit the number of tokens we'll look ahead to prevent infinite loops
		var count = 0
		
		while current_token and count < recovery_limit:
			count += 1
			
			# Check for semicolon
			if current_token.type in [LexerImpl.TokenType.PUNCTUATION, LexerImpl.TokenType.OPERATOR] and current_token.value == ";":
				_advance() # Consume the semicolon
				return
				
			# Check for other equation boundary markers
			if current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value in ["equation", "end", "algorithm", "initial"]:
				return # Don't consume these tokens, but stop recovery
			
			_advance() # Move to next token
		
		print("Warning: Recovery reached limit without finding semicolon or equation boundary")

	# Parse an expression
	func _parse_expression() -> ModelicaNode:
		print("Parsing expression: " + str(current_token.value if current_token else "null"))
		return _parse_addition()
		
	# Parse addition and subtraction (lowest precedence)
	func _parse_addition() -> ModelicaNode:
		var start_loc = get_token_location(current_token)
		
		# First parse the left operand (higher precedence)
		var left = _parse_term()
		if not left:
			return null
			
		# Skip any comments before checking for operators
		while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
			print("Skipping comment in addition expression: " + current_token.value)
			_advance()
		
		# Special handling for adjacent terms which are missing an operator - this is handled in _parse_term now
		# but we keep a check here in case something slips through
		if (left.type == NodeTypes.IDENTIFIER or left.type == NodeTypes.NUMBER) and current_token:
			# Check if the next token is an identifier or number without an operator in between
			if current_token.type == LexerImpl.TokenType.IDENTIFIER or current_token.type == LexerImpl.TokenType.NUMBER:
				var error_msg = "Missing operator between '" + str(left.value) + "' and '" + str(current_token.value) + "'"
				print("Error: " + error_msg)
				
				var error_node = ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
				error_node.add_child(left)
				return error_node
			
		# Continue parsing binary operations as long as we have +/- operators
		while current_token and current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value in ["+", "-"]:
			var op = current_token.value
			var op_loc = get_token_location(current_token)
			_advance()  # Consume operator
			
			# Skip any comments after operator before parsing right term
			while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
				print("Skipping comment after operator in addition: " + current_token.value)
				_advance()
			
			# Parse the right operand (higher precedence)
			var right = _parse_term()
			if not right:
				var error_msg = "Expected expression after '" + op + "' operator"
				print("Error: " + error_msg)
				var error_node = ModelicaNode.new(NodeTypes.ERROR, error_msg, op_loc)
				error_node.add_child(left)  # Preserve the left side
				return error_node
				
			# Create binary operation node
			var op_node = ModelicaNode.new(NodeTypes.OPERATOR, op, op_loc)
			op_node.add_child(left)
			op_node.add_child(right)
			
			# Update left for the next iteration (for chained operations)
			left = op_node
			
			# Skip any comments before checking for more operators
			while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
				print("Skipping comment after term in addition: " + current_token.value)
				_advance()
			
		return left
		
	# Parse multiplication and division (higher precedence)
	func _parse_term() -> ModelicaNode:
		var start_loc = get_token_location(current_token)
		
		# First parse the left operand (higher precedence)
		var left = _parse_factor()
		if not left:
			return null
			
		# Skip any comments before checking for operators
		while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
			print("Skipping comment in term expression: " + current_token.value)
			_advance()
			
		# Special handling for adjacent identifiers or numbers which are missing an operator
		if (left.type == NodeTypes.IDENTIFIER or left.type == NodeTypes.NUMBER) and current_token:
			# Check if the next token is an identifier or number without an operator in between
			if current_token.type == LexerImpl.TokenType.IDENTIFIER or current_token.type == LexerImpl.TokenType.NUMBER:
				var error_msg = "Missing operator between '" + str(left.value) + "' and '" + str(current_token.value) + "'"
				print("Error: " + error_msg)
				
				var error_node = ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
				error_node.add_child(left)
				return error_node
			
		# Continue parsing binary operations as long as we have */ operators
		while current_token and current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value in ["*", "/"]:
			var op = current_token.value
			var op_loc = get_token_location(current_token)
			_advance()  # Consume operator
			
			# Skip any comments after operator before parsing right factor
			while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
				print("Skipping comment after operator in term: " + current_token.value)
				_advance()
			
			# Parse the right operand (higher precedence)
			var right = _parse_factor()
			if not right:
				var error_msg = "Expected expression after '" + op + "' operator"
				print("Error: " + error_msg)
				var error_node = ModelicaNode.new(NodeTypes.ERROR, error_msg, op_loc)
				error_node.add_child(left)  # Preserve the left side
				return error_node
				
			# Create binary operation node
			var op_node = ModelicaNode.new(NodeTypes.OPERATOR, op, op_loc)
			op_node.add_child(left)
			op_node.add_child(right)
			
			# Update left for the next iteration (for chained operations)
			left = op_node
			
			# Skip any comments before checking for more operators
			while current_token and current_token.type == LexerImpl.TokenType.COMMENT:
				print("Skipping comment after factor in term: " + current_token.value)
				_advance()
			
		return left
		
	func _parse_factor() -> ModelicaNode:
		print("Parsing factor: " + str(current_token.value if current_token else "null"))
		var start_loc = get_token_location(current_token)
		var error_msg: String
		
		if not current_token:
			error_msg = "Unexpected end of input in expression"
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
		# Early check for semicolons, which should not appear inside expressions
		if current_token and (
			(current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == ";") or 
			(current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value == ";")
		):
			error_msg = "Unexpected semicolon in expression"
			print("Error: " + error_msg)
			_advance()  # Consume the semicolon to prevent infinite loops
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
		# Skip any whitespace or comments in expression
		while current_token and (current_token.type == LexerImpl.TokenType.WHITESPACE or current_token.type == LexerImpl.TokenType.COMMENT):
			print("Skipping token in expression: " + str(current_token.type) + " - " + current_token.value)
			_advance()
			
		if not current_token:
			error_msg = "Unexpected end of input after whitespace/comment in expression"
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
		# Handle conditional expressions
		if current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value == "if":
			# Handle conditional expressions (if-then-else)
			return _parse_conditional_expression()
			
		# Handle different token types
		if current_token.type == LexerImpl.TokenType.IDENTIFIER:
			# Function call or identifier
			var id = current_token.value
			var id_start_loc = get_token_location(current_token)
			_advance()  # Consume identifier
			
			# Check if it's a function call
			if current_token and (
				(current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == "(") or
				(current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value == "(")
			):
				var func_node = ModelicaNode.new(NodeTypes.FUNCTION_CALL, id, id_start_loc)
				_advance()  # Consume opening parenthesis
				
				# Check for empty function call
				if current_token and (
					(current_token.type == LexerImpl.TokenType.PUNCTUATION and current_token.value == ")") or
					(current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value == ")")
				):
					_advance()  # Consume closing parenthesis
					return func_node
				
				# Parse function arguments
				while current_token and current_token.value != ")":
					var arg = _parse_expression()
					if arg:
						func_node.add_child(arg)
					
					# Check for comma
					if current_token and current_token.value == ",":
						_advance()  # Consume comma
					elif current_token and current_token.value == ")":
						break  # End of arguments
					else:
						error_msg = "Expected ',' or ')' in function call arguments"
						print("Error: " + error_msg)
						func_node.add_error(error_msg, "syntax_error")
						break
				
				# Expect closing parenthesis
				if current_token and current_token.value == ")":
					_advance()  # Consume closing parenthesis
				else:
					error_msg = "Expected ')' in function call"
					print("Error: " + error_msg)
					func_node.add_error(error_msg, "syntax_error")
				
				return func_node
			else:
				# Simple identifier
				return ModelicaNode.new(NodeTypes.IDENTIFIER, id, id_start_loc)
		elif current_token.type == LexerImpl.TokenType.NUMBER:
			var num_node = ModelicaNode.new(NodeTypes.NUMBER, current_token.value, start_loc)
			_advance()  # Consume number
			return num_node
		elif current_token.type == LexerImpl.TokenType.STRING:
			var str_node = ModelicaNode.new(NodeTypes.STRING, current_token.value, start_loc)
			_advance()  # Consume string
			return str_node
		elif current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value == "der":
			# Handle derivative function
			_advance()  # Consume 'der'
			
			# Expect opening parenthesis
			if not current_token or current_token.value != "(":
				error_msg = "Expected '(' after 'der'"
				print("Error: " + error_msg)
				return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
			
			_advance()  # Consume opening parenthesis
			
			# Parse variable inside der()
			var var_expr = _parse_expression()
			if not var_expr:
				error_msg = "Expected expression inside der()"
				print("Error: " + error_msg)
				return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
			
			# Create der() function call node
			var der_node = ModelicaNode.new(NodeTypes.FUNCTION_CALL, "der", start_loc)
			der_node.add_child(var_expr)
			
			# Expect closing parenthesis
			if not current_token or current_token.value != ")":
				error_msg = "Expected ')' after der function argument"
				print("Error: " + error_msg)
				der_node.add_error(error_msg, "syntax_error")
				return der_node
			
			_advance()  # Consume closing parenthesis
			return der_node
		elif current_token.type == LexerImpl.TokenType.OPERATOR or current_token.type == LexerImpl.TokenType.PUNCTUATION:
			if current_token.value == "(":
				_advance()  # Consume opening parenthesis
				var expr = _parse_expression()
				
				# Expect closing parenthesis
				if current_token and current_token.value == ")":
					_advance()  # Consume closing parenthesis
					return expr
				else:
					error_msg = "Expected ')' in parenthesized expression"
					print("Error: " + error_msg)
					if expr:
						expr.add_error(error_msg, "syntax_error")
						return expr
					else:
						return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
			elif current_token.value in ["+", "-"]:
				# Unary operators
				var op = current_token.value
				var op_loc = get_token_location(current_token)
				_advance()  # Consume operator
				
				var expr = _parse_factor()
				if not expr:
					error_msg = "Expected expression after unary " + op
					print("Error: " + error_msg)
					return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
				
				var op_node = ModelicaNode.new(NodeTypes.OPERATOR, op, op_loc)
				op_node.add_child(expr)
				return op_node
		
		# If we get here, we couldn't parse the expression
		error_msg = "Unexpected token in expression: " + str(current_token.type) + " - " + current_token.value
		print("Error: " + error_msg)
		return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)

	# Parse conditional expression (if-then-else)
	func _parse_conditional_expression() -> ModelicaNode:
		var start_loc = get_token_location(current_token)
		_advance() # Consume 'if'
		
		var cond_node = ModelicaNode.new(NodeTypes.CONDITIONAL_EXPRESSION, "if", start_loc)
		
		# Parse condition
		var condition = _parse_expression()
		if not condition:
			var error_msg = "Expected condition after 'if'"
			print("Error: " + error_msg)
			cond_node.add_error(error_msg, "syntax_error")
			return cond_node
		
		cond_node.add_child(condition)
		
		# Expect 'then' keyword
		if not current_token or current_token.type != LexerImpl.TokenType.KEYWORD or current_token.value != "then":
			var error_msg = "Expected 'then' after condition in if expression"
			print("Error: " + error_msg)
			cond_node.add_error(error_msg, "syntax_error")
			return cond_node
		
		_advance() # Consume 'then'
		
		# Parse then-expression
		var then_expr = _parse_expression()
		if not then_expr:
			var error_msg = "Expected expression after 'then'"
			print("Error: " + error_msg)
			cond_node.add_error(error_msg, "syntax_error")
			return cond_node
		
		cond_node.add_child(then_expr)
		
		# Handle elseif branches
		while current_token and current_token.type == LexerImpl.TokenType.KEYWORD and current_token.value == "elseif":
			_advance() # Consume 'elseif'
			
			var elseif_condition = _parse_expression()
			if not elseif_condition:
				var error_msg = "Expected condition after 'elseif'"
				print("Error: " + error_msg)
				cond_node.add_error(error_msg, "syntax_error")
				return cond_node
			
			# Expect 'then' keyword
			if not current_token or current_token.type != LexerImpl.TokenType.KEYWORD or current_token.value != "then":
				var error_msg = "Expected 'then' after condition in elseif"
				print("Error: " + error_msg)
				cond_node.add_error(error_msg, "syntax_error")
				return cond_node
			
			_advance() # Consume 'then'
			
			var elseif_expr = _parse_expression()
			if not elseif_expr:
				var error_msg = "Expected expression after 'then' in elseif"
				print("Error: " + error_msg)
				cond_node.add_error(error_msg, "syntax_error")
				return cond_node
			
			# Add elseif branch as a pair of condition and expression
			var elseif_branch = ModelicaNode.new(NodeTypes.CONDITIONAL_EXPRESSION, "elseif", get_token_location(current_token))
			elseif_branch.add_child(elseif_condition)
			elseif_branch.add_child(elseif_expr)
			cond_node.add_child(elseif_branch)
		
		# Expect 'else' keyword
		if not current_token or current_token.type != LexerImpl.TokenType.KEYWORD or current_token.value != "else":
			var error_msg = "Expected 'else' in conditional expression"
			print("Error: " + error_msg)
			cond_node.add_error(error_msg, "syntax_error")
			return cond_node
		
		_advance() # Consume 'else'
		
		# Parse else-expression
		var else_expr = _parse_expression()
		if not else_expr:
			var error_msg = "Expected expression after 'else'"
			print("Error: " + error_msg)
			cond_node.add_error(error_msg, "syntax_error")
			return cond_node
		
		cond_node.add_child(else_expr)
		
		return cond_node

	# Helper to check if a token represents a valid type name
	func _is_type_name(name: String) -> bool:
		# Add common Modelica types
		var built_in_types = ["Real", "Integer", "Boolean", "String"]
		return name in built_in_types

# Factory functions to create parsers
static func create_modelica_parser() -> ModelicaParser:
	return ModelicaParser.new()

static func create_equation_parser() -> EquationParser:
	return EquationParser.new() 
