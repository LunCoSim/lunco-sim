@tool
extends RefCounted
class_name SyntaxParser

var lexer: LexicalAnalyzer
var tokens: Array[LexicalAnalyzer.Token] = []
var position: int = 0
var current_token: LexicalAnalyzer.Token = null
var errors: Array[String] = []

func _init(p_lexer: LexicalAnalyzer = null) -> void:
	lexer = p_lexer if p_lexer else LexicalAnalyzer.new()

func parse(text: String) -> ModelicaASTNode:
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
		var error_node = ModelicaASTNode.new(ModelicaASTNode.NodeType.ERROR, error_str, location)
		error_node.add_error(error_str, "syntax_error", location)
		return error_node
	
	return ast

func _parse() -> ModelicaASTNode:
	# To be implemented by derived classes
	push_error("_parse() must be implemented by derived classes")
	return null

func _advance() -> LexicalAnalyzer.Token:
	position += 1
	if position < tokens.size():
		current_token = tokens[position]
	else:
		current_token = null
	return current_token

func _peek() -> LexicalAnalyzer.Token:
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

func _error(message: String) -> ModelicaASTNode:
	var location = {
		"line": current_token.line if current_token else 0,
		"column": current_token.column if current_token else 0
	}
	var error_node = ModelicaASTNode.new(ModelicaASTNode.NodeType.ERROR, message, location)
	error_node.add_error(message, "syntax_error", location)
	return error_node

func _token_type_to_string(type: int) -> String:
	return LexicalAnalyzer.TokenType.keys()[type]

func _has_errors() -> bool:
	return not errors.is_empty()

func get_errors() -> Array[String]:
	return errors 