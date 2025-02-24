@tool
extends LexicalAnalyzer
class_name ModelicaLexer

const MODELICA_KEYWORDS: Array[String] = [
	"model", "connector", "package", "class", "record", "block",
	"type", "function", "extends", "parameter", "constant", "input",
	"output", "flow", "stream", "equation", "algorithm", "end",
	"if", "then", "else", "elseif", "for", "loop", "in", "while",
	"when", "elsewhen", "connect", "der", "initial", "inner", "outer",
	"public", "protected", "final", "each", "partial", "redeclare",
	"replaceable", "import", "within", "encapsulated", "annotation",
	"external", "and", "or", "not", "true", "false"
]

const MODELICA_OPERATORS: Array[String] = [
	"+", "-", "*", "/", "^", "=", "<", ">", "<=", ">=", "==", "<>",
	":=", ".", ",", ";", "(", ")", "[", "]", "{", "}", ":", ".."
]

func _init() -> void:
	super._init()
	set_keywords(MODELICA_KEYWORDS)

func _handle_operator() -> Token:
	var start_pos = _position
	var start_col = _column
	var c = _current_char()
	
	# Handle Modelica-specific operators
	var two_char = c + _peek_next()
	if two_char in ["<=", ">=", "==", "<>", ":=", ".."]:
		_position += 2
		_column += 2
		return Token.new(TokenType.OPERATOR, two_char, _line, start_col, start_pos)
	
	# Handle single-character operators and punctuation
	_position += 1
	_column += 1
	
	var type = TokenType.OPERATOR if c in MODELICA_OPERATORS else TokenType.PUNCTUATION
	return Token.new(type, c, _line, start_col, start_pos)

# Override to handle Modelica-specific number formats
func _handle_number() -> Token:
	var token = super._handle_number()
	
	# Check for units after numbers (if needed)
	if _current_char() == " ":
		var next = _peek_next()
		if next.is_valid_identifier():
			# Handle units here if needed
			pass
	
	return token 