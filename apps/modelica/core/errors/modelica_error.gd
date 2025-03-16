@tool
class_name ModelicaError
extends RefCounted

enum Severity {INFO, WARNING, ERROR, FATAL}
enum Category {
    SYNTAX,           # Syntax errors
    TYPE,             # Type checking errors
    SEMANTIC,         # Semantic errors
    VARIABLE,         # Variable-related errors (undefined, already defined)
    EQUATION,         # Equation errors
    SOLVER,           # Numerical solver errors
    SYSTEM            # System/internal errors
}

var message: String
var severity: Severity
var category: Category
var location: Dictionary = {} # {file, line, column}
var context: Dictionary = {}  # Additional information

func _init(msg: String, cat: Category, sev: Severity = Severity.ERROR, loc = null, ctx = null):
    message = msg
    category = cat
    severity = sev
    if loc:
        location = loc
    if ctx:
        context = ctx
        
func get_error_string() -> String:
    var result = "%s: %s" % [Severity.keys()[severity], message]
    if not location.is_empty():
        result += " at %s:%s:%s" % [location.file, location.line, location.column]
    return result 