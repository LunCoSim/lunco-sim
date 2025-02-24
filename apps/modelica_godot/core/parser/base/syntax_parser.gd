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

func parse(text: String) -> ASTNode:
	errors.clear()
	tokens = lexer.tokenize(text)
	position = 0
	current_token = _advance()
	return _parse()

func _parse() -> ASTNode:
	# To be implemented by derived classes
	push_error("_parse() must be implemented by derived classes")
	return null

func _advance() -> LexicalAnalyzer.Token:
	while position < tokens.size():
		var token = tokens[position]
		position += 1
		
		# Skip whitespace and comments by default
		if token.type in [LexicalAnalyzer.TokenType.WHITESPACE, 
						 LexicalAnalyzer.TokenType.COMMENT]:
			continue
			
		current_token = token
		return token
	
	return null

func _peek() -> LexicalAnalyzer.Token:
	var saved_pos = position
	var saved_token = current_token
	
	var next_token = _advance()
	
	position = saved_pos
	current_token = saved_token
	
	return next_token

func _expect(type: int, value: String = "") -> LexicalAnalyzer.Token:
	if current_token == null:
		_error("Expected %s but got end of input" % _token_type_to_string(type))
		return null
	
	if current_token.type != type:
		_error("Expected %s but got %s" % [
			_token_type_to_string(type),
			_token_type_to_string(current_token.type)
		])
		return null
	
	if not value.is_empty() and current_token.value != value:
		_error("Expected '%s' but got '%s'" % [value, current_token.value])
		return null
	
	var token = current_token
	_advance()
	return token

func _match(type: int, value: String = "") -> bool:
	if current_token == null:
		return false
	
	if current_token.type != type:
		return false
	
	if not value.is_empty() and current_token.value != value:
		return false
	
	_advance()
	return true

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