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
				print("Model node created with type: " + str(model_node.type))
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
			var eq_node = _parse_single_equation()
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
	func _parse_single_equation() -> ModelicaNode:
		print("Parsing single equation")
		var start_loc = get_token_location(current_token)
		
		# Parse left-hand side expression
		var lhs = _parse_expression()
		if not lhs:
			print("Failed to parse left-hand side of equation")
			return null
		
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
			return error_node
		
		_advance()  # Consume equals sign
		
		# Parse right-hand side expression
		var rhs = _parse_expression()
		if not rhs:
			print("Failed to parse right-hand side of equation")
			var error_msg = "Failed to parse right side of equation"
			var error_node = ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
			error_node.add_child(lhs)  # Attach the left-hand side for context
			return error_node
		
		# Create equation node
		var eq_node = ModelicaNode.new(NodeTypes.EQUATION, "=", start_loc)
		eq_node.add_child(lhs)
		eq_node.add_child(rhs)
		
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
		
		return eq_node

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
			
		# Continue parsing binary operations as long as we have +/- operators
		while current_token and current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value in ["+", "-"]:
			var op = current_token.value
			var op_loc = get_token_location(current_token)
			_advance()  # Consume operator
			
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
			
		return left
		
	# Parse multiplication and division (higher precedence)
	func _parse_term() -> ModelicaNode:
		var start_loc = get_token_location(current_token)
		
		# First parse the left operand (higher precedence)
		var left = _parse_factor()
		if not left:
			return null
			
		# Continue parsing binary operations as long as we have */ operators
		while current_token and current_token.type == LexerImpl.TokenType.OPERATOR and current_token.value in ["*", "/"]:
			var op = current_token.value
			var op_loc = get_token_location(current_token)
			_advance()  # Consume operator
			
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
			
		return left
		
	# Parse atomic expressions and highest precedence operations
	func _parse_factor() -> ModelicaNode:
		print("Parsing factor: " + str(current_token.value if current_token else "null"))
		var start_loc = get_token_location(current_token)
		
		if not current_token:
			var error_msg = "Unexpected end of input in expression"
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
		# Skip any whitespace in expression
		while current_token and current_token.type == LexerImpl.TokenType.WHITESPACE:
			_advance()
			
		if not current_token:
			var error_msg = "Unexpected end of input after whitespace in expression"
			print("Error: " + error_msg)
			return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
		
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
						var error_msg = "Expected ',' or ')' in function call arguments"
						print("Error: " + error_msg)
						func_node.add_error(error_msg, "syntax_error")
						break
				
				# Expect closing parenthesis
				if current_token and current_token.value == ")":
					_advance()  # Consume closing parenthesis
				else:
					var error_msg = "Expected ')' in function call"
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
				var error_msg = "Expected '(' after 'der'"
				print("Error: " + error_msg)
				return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
			
			_advance()  # Consume opening parenthesis
			
			# Parse variable inside der()
			var var_expr = _parse_expression()
			if not var_expr:
				var error_msg = "Expected expression inside der()"
				print("Error: " + error_msg)
				return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
			
			# Create der() function call node
			var der_node = ModelicaNode.new(NodeTypes.FUNCTION_CALL, "der", start_loc)
			der_node.add_child(var_expr)
			
			# Expect closing parenthesis
			if not current_token or current_token.value != ")":
				var error_msg = "Expected ')' after der function argument"
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
					var error_msg = "Expected ')' in parenthesized expression"
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
					var error_msg = "Expected expression after unary " + op
					print("Error: " + error_msg)
					return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)
				
				var op_node = ModelicaNode.new(NodeTypes.OPERATOR, op, op_loc)
				op_node.add_child(expr)
				return op_node
		
		# If we get here, we couldn't parse the expression
		var error_msg = "Unexpected token in expression: " + str(current_token.type) + " - " + current_token.value
		print("Error: " + error_msg)
		return ModelicaNode.new(NodeTypes.ERROR, error_msg, start_loc)

# Factory functions to create parsers
static func create_modelica_parser() -> ModelicaParser:
	return ModelicaParser.new()

static func create_equation_parser() -> EquationParser:
	return EquationParser.new() 