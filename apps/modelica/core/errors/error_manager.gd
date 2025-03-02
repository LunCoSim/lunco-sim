@tool
class_name ErrorManager
extends RefCounted

signal error_reported(error)

var errors: Array = []
var warnings: Array = []
var stop_on_error: bool = true

func report_error(error: ModelicaError) -> void:
    if error.severity >= ModelicaError.Severity.ERROR:
        errors.append(error)
    elif error.severity == ModelicaError.Severity.WARNING:
        warnings.append(error)
    
    emit_signal("error_reported", error)
    
    # Log to console for development
    print(error.to_string())
    
    if stop_on_error and error.severity >= ModelicaError.Severity.FATAL:
        # For fatal errors, we could potentially throw or halt execution
        push_error("FATAL ERROR: " + error.to_string())

func report(msg: String, category: ModelicaError.Category, 
           severity: ModelicaError.Severity = ModelicaError.Severity.ERROR, 
           location = null, context = null) -> ModelicaError:
    var error = ModelicaError.new(msg, category, severity, location, context)
    report_error(error)
    return error

func has_errors() -> bool:
    return errors.size() > 0
    
func has_warnings() -> bool:
    return warnings.size() > 0

func get_errors() -> Array:
    return errors

func get_warnings() -> Array:
    return warnings

func clear() -> void:
    errors.clear()
    warnings.clear()
    
func get_error_count() -> int:
    return errors.size()
    
func get_warning_count() -> int:
    return warnings.size()

# Helper methods for common error types
func report_variable_error(msg: String, severity: ModelicaError.Severity = ModelicaError.Severity.ERROR, 
                          location = null, context = null) -> ModelicaError:
    return report(msg, ModelicaError.Category.VARIABLE, severity, location, context)
    
func report_syntax_error(msg: String, severity: ModelicaError.Severity = ModelicaError.Severity.ERROR, 
                         location = null, context = null) -> ModelicaError:
    return report(msg, ModelicaError.Category.SYNTAX, severity, location, context)
    
func report_equation_error(msg: String, severity: ModelicaError.Severity = ModelicaError.Severity.ERROR, 
                          location = null, context = null) -> ModelicaError:
    return report(msg, ModelicaError.Category.EQUATION, severity, location, context)
    
func report_solver_error(msg: String, severity: ModelicaError.Severity = ModelicaError.Severity.ERROR, 
                         location = null, context = null) -> ModelicaError:
    return report(msg, ModelicaError.Category.SOLVER, severity, location, context) 