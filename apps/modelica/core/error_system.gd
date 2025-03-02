@tool
class_name ModelicaErrorSystem
extends RefCounted

#-----------------------------------------------------------------------------
# Error types and severity levels
#-----------------------------------------------------------------------------
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

#-----------------------------------------------------------------------------
# MError Class - represents a single error
#-----------------------------------------------------------------------------
class MError:
    var message: String
    var severity: int
    var category: int
    var location: Dictionary = {} # {file, line, column}
    var context: Dictionary = {}  # Additional information

    func _init(msg: String, cat: int, sev: int = Severity.ERROR, loc = null, ctx = null):
        message = msg
        category = cat
        severity = sev
        if loc:
            location = loc
        if ctx:
            context = ctx
            
    func get_string() -> String:
        var sev_name = "UNKNOWN"
        match severity:
            Severity.INFO: sev_name = "INFO"
            Severity.WARNING: sev_name = "WARNING"
            Severity.ERROR: sev_name = "ERROR"
            Severity.FATAL: sev_name = "FATAL"
            
        var result = "%s: %s" % [sev_name, message]
        if not location.is_empty():
            result += " at %s:%s:%s" % [location.file, location.line, location.column]
        return result

#-----------------------------------------------------------------------------
# MResult Class - represents success or failure with possible value
#-----------------------------------------------------------------------------
class MResult:
    var success: bool
    var value = null
    var error = null  # MError

    # Constructor for successful result
    static func ok(result_value = null):
        var instance = MResult.new()
        instance.success = true
        instance.value = result_value
        return instance

    # Constructor for error result
    static func err(error_obj):
        var instance = MResult.new()
        instance.success = false
        instance.error = error_obj
        return instance

    # Check if result is successful
    func is_ok() -> bool:
        return success
        
    # Check if result is an error
    func is_err() -> bool:
        return not success

    # Get the value (returns null if error)
    func get_value():
        return value

    # Get the error (returns null if success)
    func get_error():
        return error

    # Get string representation
    func get_string() -> String:
        if success:
            return "Result.ok(%s)" % [str(value)]
        else:
            return "Result.err(%s)" % [error.get_string()]

#-----------------------------------------------------------------------------
# MErrorManager Class - manages collection of errors
#-----------------------------------------------------------------------------
class MErrorManager:
    var errors = []
    var warnings = []
    var stop_on_error: bool = true

    func report_error(error) -> void:
        if error.severity >= Severity.ERROR:
            errors.append(error)
        elif error.severity == Severity.WARNING:
            warnings.append(error)
        
        # Log to console for development
        print(error.get_string())
        
        if stop_on_error and error.severity >= Severity.FATAL:
            # For fatal errors, we could potentially throw or halt execution
            push_error("FATAL ERROR: " + error.get_string())

    func create_error(msg: String, category: int, 
                     severity: int = Severity.ERROR, 
                     location = null, context = null):
        var error = MError.new(msg, category, severity, location, context)
        report_error(error)
        return error

    func has_errors() -> bool:
        return errors.size() > 0
        
    func has_warnings() -> bool:
        return warnings.size() > 0

    func get_errors():
        return errors

    func get_warnings():
        return warnings

    func clear() -> void:
        errors.clear()
        warnings.clear()
        
    func get_error_count() -> int:
        return errors.size()
        
    func get_warning_count() -> int:
        return warnings.size()

    # Helper methods for common error types
    func report_variable_error(msg: String, severity: int = Severity.ERROR, 
                              location = null, context = null):
        return create_error(msg, Category.VARIABLE, severity, location, context)
        
    func report_syntax_error(msg: String, severity: int = Severity.ERROR, 
                            location = null, context = null):
        return create_error(msg, Category.SYNTAX, severity, location, context)
        
    func report_equation_error(msg: String, severity: int = Severity.ERROR, 
                              location = null, context = null):
        return create_error(msg, Category.EQUATION, severity, location, context)
        
    func report_solver_error(msg: String, severity: int = Severity.ERROR, 
                            location = null, context = null):
        return create_error(msg, Category.SOLVER, severity, location, context)

#-----------------------------------------------------------------------------
# Factory methods for easy creation
#-----------------------------------------------------------------------------

# Create a new error manager
static func create_error_manager():
    return MErrorManager.new()
    
# Create a new error
static func create_error(msg: String, category: int, 
                        severity: int = Severity.ERROR, 
                        location = null, context = null):
    return MError.new(msg, category, severity, location, context)
    
# Create a successful result
static func ok(value = null):
    return MResult.ok(value)
    
# Create an error result
static func err(error_obj):
    return MResult.err(error_obj)
    
# Helper to create error result directly from message
static func error(msg: String, category: int, 
                 severity: int = Severity.ERROR,
                 location = null, context = null):
    var error_obj = MError.new(msg, category, severity, location, context)
    return MResult.err(error_obj) 