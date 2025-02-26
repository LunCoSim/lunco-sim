@tool
extends LexicalAnalyzer
class_name EquationLexer

const EQUATION_KEYWORDS = [
	"der", "sin", "cos", "tan", "asin", "acos", "atan", "atan2",
	"sinh", "cosh", "tanh", "exp", "log", "log10", "sqrt",
	"pre", "initial", "terminal", "sample", "edge", "change",
	"reinit", "delay", "cardinality", "and", "or", "not"
]

const EQUATION_OPERATORS = [
	"+", "-", "*", "/", "^", "=", "<", ">", "<=", ">=", "==", "<>",
	"and", "or", "not", "(", ")", "[", "]", ",", "."
]

func _init() -> void:
	super._init()
	set_keywords(EQUATION_KEYWORDS)

func _handle_operator() -> Token:
	var start_pos = _position
	var start_col = _column
	var c = _current_char()
	
	# Handle equation-specific operators
	var two_char = c + _peek_next()
	if two_char in ["<=", ">=", "==", "<>"]:
		_position += 2
		_column += 2
		return Token.new(TokenType.OPERATOR, two_char, _line, start_col, start_pos)
	
	# Handle single-character operators and punctuation
	_position += 1
	_column += 1
	
	var type = TokenType.OPERATOR if c in EQUATION_OPERATORS else TokenType.PUNCTUATION
	return Token.new(type, c, _line, start_col, start_pos)

# Override to handle equation-specific number formats
func _handle_number() -> Token:
	var start_pos = _position
	var start_col = _column
	var value = ""
	var has_decimal = false
	var has_exponent = false
	
	# Handle sign
	if _current_char() == "-" or _current_char() == "+":
		value += _current_char()
		_position += 1
		_column += 1
	
	while _position < _length:
		var c = _current_char()
		
		# Handle decimal point
		if c == "." and not has_decimal and not has_exponent:
			has_decimal = true
			value += c
			_position += 1
			_column += 1
			continue
		
		# Handle scientific notation
		if (c == "e" or c == "E") and not has_exponent:
			has_exponent = true
			value += c
			_position += 1
			_column += 1
			
			# Handle exponent sign
			if _position < _length and (_peek_next() == "+" or _peek_next() == "-"):
				value += _peek_next()
				_position += 1
				_column += 1
			continue
		
		if not c.is_valid_int():
			break
		
		value += c
		_position += 1
		_column += 1
	
	return Token.new(TokenType.NUMBER, value, _line, start_col, start_pos)

# Override to handle equation-specific identifiers
func _handle_identifier() -> Token:
	var token = super._handle_identifier()
	
	# Check for derivative operator
	if token.value == "der" and _current_char() == "(":
		# Handle derivative notation
		pass
	
	return token 