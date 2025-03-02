extends SyntaxHighlighter

# Import Modelica lexer
const ModelicaLexer = preload("res://apps/modelica/core/lexer.gd")

# Syntax highlighting colors
var keyword_color = Color(0.94, 0.4, 0.55)  # Pink
var type_color = Color(0.4, 0.7, 0.9)       # Light Blue
var comment_color = Color(0.5, 0.5, 0.5)    # Gray
var string_color = Color(0.6, 0.8, 0.4)     # Green
var number_color = Color(0.94, 0.76, 0.4)   # Orange
var function_color = Color(0.67, 0.8, 0.96) # Lighter Blue
var operator_color = Color(0.8, 0.8, 0.8)   # Light Gray
var variable_color = Color(0.9, 0.9, 0.9)   # Almost White
var annotation_color = Color(0.6, 0.5, 0.8) # Purple
var transparent_color = Color(0, 0, 0, 0)   # Transparent (no highlighting)

# Lexer instance
var lexer = null

# Cache for tokenized lines to improve performance
var line_cache = {}
var last_text = ""

func _init():
	lexer = ModelicaLexer.new()

# The _get_line_syntax_highlighting function is called for each visible line
func _get_line_syntax_highlighting(line: int) -> Dictionary:
	var result = {}
	
	# Get the text for the line
	var text = get_text_edit().get_line(line)
	
	# Skip empty lines
	if text.strip_edges().is_empty():
		return result
	
	# Check if we need to refresh the token cache
	var full_text = get_text_edit().text
	if full_text != last_text:
		line_cache.clear()
		last_text = full_text
	
	# Use cached tokens if available
	if line in line_cache:
		return line_cache[line]
	
	# Tokenize the line using the Modelica lexer
	var tokens = _tokenize_line(text)
	
	# Apply colors based on token types
	for token in tokens:
		var color = _get_token_color(token)
		if color != transparent_color:
			result[token.start_column] = {
				"color": color,
				"end": token.start_column + token.text.length()
			}
	
	# Cache the result
	line_cache[line] = result
	
	return result

# Tokenize a single line using the Modelica lexer
func _tokenize_line(line_text: String) -> Array:
	# Since the lexer works on full text, we need to tokenize just this line
	# We add a newline to ensure proper tokenization of the last token
	var tokens = []
	
	# Use the tokenize method to get lexer tokens
	var lexer_tokens = lexer.tokenize(line_text + "\n")
	
	# Convert the lexer tokens to our expected format
	for lexer_token in lexer_tokens:
		if lexer_token.type != ModelicaLexer.TokenType.EOF and lexer_token.type != ModelicaLexer.TokenType.WHITESPACE:
			tokens.append({
				"type": _convert_token_type(lexer_token.type),
				"text": lexer_token.value,
				"start_column": lexer_token.column - 1  # Adjust to 0-based indexing
			})
	
	return tokens

# Helper function to convert token type enum to string representation
func _convert_token_type(token_type: int) -> String:
	match token_type:
		ModelicaLexer.TokenType.KEYWORD: 
			return "KEYWORD"
		ModelicaLexer.TokenType.IDENTIFIER: 
			return "IDENTIFIER"
		ModelicaLexer.TokenType.NUMBER:
			return "NUMBER" 
		ModelicaLexer.TokenType.STRING:
			return "STRING"
		ModelicaLexer.TokenType.OPERATOR:
			return "OPERATOR"
		ModelicaLexer.TokenType.COMMENT:
			return "COMMENT"
		ModelicaLexer.TokenType.PUNCTUATION:
			return "OPERATOR"  # Treat punctuation as operators
		_:
			return "UNKNOWN"

# Get color for a token based on its type
func _get_token_color(token) -> Color:
	match token.type:
		"KEYWORD":
			return keyword_color
		"IDENTIFIER":
			# Check if it's a built-in type
			if token.text in ["Real", "Integer", "Boolean", "String"]:
				return type_color
			else:
				return variable_color
		"NUMBER":
			return number_color
		"STRING":
			return string_color
		"OPERATOR", "ASSIGN", "LPAREN", "RPAREN", "LBRACE", "RBRACE", "LBRACKET", "RBRACKET", "COMMA", "SEMICOLON", "COLON", "DOT":
			return operator_color
		"COMMENT", "MULTILINE_COMMENT":
			return comment_color
		"ANNOTATION":
			return annotation_color
	
	# Return a transparent color for unknown token types
	return transparent_color

# Additional helper functions for advanced editor features
func get_word_at_position(text: String, position: int) -> String:
	# Find the word boundaries at the given position
	var start = position
	var end = position
	
	# Find start of word
	while start > 0 and _is_identifier_char(text[start - 1]):
		start -= 1
	
	# Find end of word
	while end < text.length() and _is_identifier_char(text[end]):
		end += 1
	
	return text.substr(start, end - start)

func _is_identifier_char(c: String) -> bool:
	return (c >= 'a' and c <= 'z') or (c >= 'A' and c <= 'Z') or (c >= '0' and c <= '9') or c == '_'
