extends Resource
class_name LCLogEndpoint

var level_mask: int = 31 # Default to ALL (LogLevel.ALL)

func log_message(message: String, is_error: bool) -> void:
	pass

func log_error(function: String, file: String, line: int, code: String, 
			rationale: String, editor_notify: bool, error_type: int, 
			script_backtraces: Array) -> void:
	pass

func flush() -> void:
	pass

func should_log(level: int) -> bool:
	return (level & level_mask) != 0
