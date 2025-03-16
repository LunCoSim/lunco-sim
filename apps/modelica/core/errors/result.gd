@tool
class_name Result
extends RefCounted

var success: bool
var value = null
var error: ModelicaError = null

# Constructor for successful result
static func ok(result_value = null) -> Result:
    var instance = Result.new()
    instance.success = true
    instance.value = result_value
    return instance

# Constructor for error result
static func err(error_obj: ModelicaError) -> Result:
    var instance = Result.new()
    instance.success = false
    instance.error = error_obj
    return instance

# Helper to create error with message
static func create_error(msg: String, category: ModelicaError.Category, 
                severity: ModelicaError.Severity = ModelicaError.Severity.ERROR,
                location = null, context = null) -> Result:
    var error_obj = ModelicaError.new(msg, category, severity, location, context)
    return err(error_obj)

# Protected constructor - use static methods ok() and err() instead
func _init():
    pass

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
func get_error() -> ModelicaError:
    return error

# Get string representation
func get_result_string() -> String:
    if success:
        return "Result.ok(%s)" % [str(value)]
    else:
        return "Result.err(%s)" % [error.get_error_string()] 