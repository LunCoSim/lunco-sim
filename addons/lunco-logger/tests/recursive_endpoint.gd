extends LCLogEndpoint

var _logger: Node
func _init(logger_node: Node):
	_logger = logger_node
func log_message(message: String, _is_error: bool, _is_engine: bool = false) -> void:
	if message.contains("RECURSION_STOP"): return
	# This print() will be caught by EngineLoggerImpl -> log_raw() -> push_log()
	# But log_raw() has a recursion guard.
	print("Recursive engine log")
	# This is a direct call
	# But _log_variadic() has a recursion guard.
	if is_instance_valid(_logger):
		_logger.call("info", "RECURSION_STOP: Direct recursive call")
