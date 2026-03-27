extends LCLogEndpoint
class_name LCLogConsoleEndpoint

func log_message(message: String, is_error: bool, is_engine: bool = false) -> void:
	# If this is from the engine, skip it to avoid duplication in console
	if is_engine:
		return
		
	if is_error:
		printerr(message)
	else:
		printraw(message + "\n")

func log_error(function: String, file: String, line: int, code: String, 
			rationale: String, _editor_notify: bool, error_type: int, 
			_script_backtraces: Array, is_engine: bool = false) -> void:
	# If this is from the engine, skip it to avoid duplication in console
	if is_engine:
		return
	
	var type_str = "ERROR" if error_type == 0 else "WARNING"
	var entry = "[%s] %s:%d in %s() - %s: %s" % [
		type_str, file, line, function, code, rationale
	]
	log_message(entry, error_type == 0)
