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
	# To be implemented by derived classes
	push_error("parse() must be implemented by derived classes")
	return null

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

func _error(message: String) -> void:
	var error = "Parse error"
	if current_token:
		error += " at line %d, column %d" % [current_token.line, current_token.column]
	error += ": " + message
	errors.append(error)
	push_error(error)

func _token_type_to_string(type: int) -> String:
	return LexicalAnalyzer.TokenType.keys()[type]

func _has_errors() -> bool:
	return not errors.is_empty()

func get_errors() -> Array[String]:
	return errors 

func _match_keyword(keyword: String) -> bool:
	return _match(LexicalAnalyzer.TokenType.KEYWORD, keyword) 