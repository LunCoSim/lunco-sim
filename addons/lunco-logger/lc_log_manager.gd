extends Node

const SETTING_PATH = "lunco/logger/path"
const SETTING_MAX_FILES = "lunco/logger/max_files"
const SETTING_STDOUT_LEVEL = "lunco/logger/stdout_level"
const SETTING_FILE_LEVEL = "lunco/logger/file_level"

enum LogLevel {
	DEBUG = 1,
	INFO = 2,
	WARN = 4,
	ERROR = 8,
	FATAL = 16,
	ALL = 31,
	NONE = 0
}

var _endpoints: Array[LCLogEndpoint] = []
var _queue: Array = []
var _mutex: Mutex
var _semaphore: Semaphore
var _exit_thread: bool = false
var _thread_task_id: int = -1

# Thread-local storage to prevent recursion
# Note: In Godot 4, we use set_meta on the thread if possible, 
# but for the worker thread we can just use a simple flag since it's one task.
var _is_logging_thread: bool = false

class EngineLoggerImpl extends Logger:
	var _manager_ref: WeakRef
	
	func _init(manager: Node):
		_manager_ref = weakref(manager)

	func _log_message(message: String, is_error: bool) -> void:
		var manager = _manager_ref.get_ref()
		if manager:
			# If this is the manager's own worker thread, ignore to prevent recursion
			if manager._is_logging_thread:
				return
			var level = LogLevel.ERROR if is_error else LogLevel.INFO
			manager.log_raw(message, level)

	func _log_error(function: String, file: String, line: int, code: String, 
					rationale: String, editor_notify: bool, error_type: int, 
					script_backtraces: Array[ScriptBacktrace]) -> void:
		var manager = _manager_ref.get_ref()
		if manager:
			if manager._is_logging_thread:
				return
			var level = LogLevel.ERROR if error_type == 0 else LogLevel.WARN
			manager.push_error_log(level, function, file, line, code, rationale, editor_notify, error_type, script_backtraces)

func _enter_tree() -> void:
	_setup_settings()
	_mutex = Mutex.new()
	_semaphore = Semaphore.new()
	
	_update_from_settings()
	ProjectSettings.settings_changed.connect(_on_settings_changed)
	
	_thread_task_id = WorkerThreadPool.add_task(_worker_loop)
	
	OS.add_logger(EngineLoggerImpl.new(self))

func _exit_tree() -> void:
	_exit_thread = true
	_semaphore.post()
	if _thread_task_id != -1:
		WorkerThreadPool.wait_for_task_completion(_thread_task_id)
	
	flush()

func _on_settings_changed():
	_update_from_settings()

func _update_from_settings():
	var path = ProjectSettings.get_setting(SETTING_PATH, "user://logs")
	var max_files = ProjectSettings.get_setting(SETTING_MAX_FILES, 5)
	var stdout_level = ProjectSettings.get_setting(SETTING_STDOUT_LEVEL, LogLevel.ALL)
	var file_level = ProjectSettings.get_setting(SETTING_FILE_LEVEL, LogLevel.ALL)
	
	_mutex.lock()
	var file_endpoint: LCLogEndpoint = null
	var console_endpoint: LCLogEndpoint = null
	
	for e in _endpoints:
		var script_path = e.get_script().get_path()
		if script_path.ends_with("lc_log_file_endpoint.gd"):
			file_endpoint = e
		elif script_path.ends_with("lc_log_console_endpoint.gd"):
			console_endpoint = e
	
	if not file_endpoint:
		file_endpoint = load("res://addons/lunco-logger/lc_log_file_endpoint.gd").new(path, max_files)
		_endpoints.append(file_endpoint)
	else:
		file_endpoint.call("update_configuration", path, max_files)
	file_endpoint.level_mask = file_level
	
	if not console_endpoint:
		console_endpoint = load("res://addons/lunco-logger/lc_log_console_endpoint.gd").new()
		_endpoints.append(console_endpoint)
	console_endpoint.level_mask = stdout_level
	
	_mutex.unlock()

func _setup_settings():
	_set_setting_default(SETTING_PATH, "user://logs")
	_set_setting_default(SETTING_MAX_FILES, 5)
	_set_setting_default(SETTING_STDOUT_LEVEL, LogLevel.ALL)
	_set_setting_default(SETTING_FILE_LEVEL, LogLevel.ALL)

func _set_setting_default(name: String, value: Variant):
	if not ProjectSettings.has_setting(name):
		ProjectSettings.set_setting(name, value)
	ProjectSettings.set_initial_value(name, value)
	ProjectSettings.set_as_basic(name, true)

# High-level API with VARIADIC arguments
func debug(...args): _log_variadic(LogLevel.DEBUG, args)
func info(...args): _log_variadic(LogLevel.INFO, args)
func warn(...args): _log_variadic(LogLevel.WARN, args)
func error(...args): _log_variadic(LogLevel.ERROR, args)
func fatal(...args): _log_variadic(LogLevel.FATAL, args)

func _log_variadic(level: int, args: Array):
	var parts = []
	for a in args:
		if typeof(a) == TYPE_DICTIONARY or typeof(a) == TYPE_ARRAY:
			parts.append(JSON.stringify(a))
		else:
			parts.append(str(a))
	
	var message = "".join(parts)
	var time = Time.get_time_string_from_system()
	var level_names = LogLevel.keys()
	var level_values = LogLevel.values()
	var level_idx = level_values.find(level)
	var level_str = level_names[level_idx] if level_idx != -1 else "UNKNOWN"
	var formatted = "[%s][%s] %s" % [time, level_str, message]
	
	push_log(formatted, level)

# For engine redirection
func log_raw(message: String, level: int):
	push_log(message, level)

func push_log(message: String, level: int):
	_mutex.lock()
	_queue.push_back({ "type": "msg", "text": message, "level": level })
	_mutex.unlock()
	_semaphore.post()

func push_error_log(level, function, file, line, code, rationale, editor_notify, error_type, script_backtraces):
	_mutex.lock()
	_queue.push_back({
		"type": "err",
		"level": level,
		"function": function,
		"file": file,
		"line": line,
		"code": code,
		"rationale": rationale,
		"editor_notify": editor_notify,
		"error_type": error_type,
		"backtraces": script_backtraces
	})
	_mutex.unlock()
	_semaphore.post()

func add_endpoint(endpoint: LCLogEndpoint):
	_mutex.lock()
	_endpoints.append(endpoint)
	_mutex.unlock()

func remove_endpoint(endpoint: LCLogEndpoint):
	_mutex.lock()
	_endpoints.erase(endpoint)
	_mutex.unlock()

func clear_endpoints():
	_mutex.lock()
	_endpoints.clear()
	_mutex.unlock()

func flush():
	_mutex.lock()
	var logs = _queue.duplicate()
	_queue.clear()
	_mutex.unlock()
	_process_logs(logs)
	
	_mutex.lock()
	for endpoint in _endpoints:
		endpoint.flush()
	_mutex.unlock()

func _process_logs(logs: Array):
	_is_logging_thread = true
	_mutex.lock()
	var current_endpoints = _endpoints.duplicate()
	_mutex.unlock()
	
	for item in logs:
		for endpoint in current_endpoints:
			if not endpoint.should_log(item.level):
				continue
				
			if item.type == "msg":
				endpoint.log_message(item.text, item.level >= LogLevel.ERROR)
			else:
				endpoint.log_error(item.function, item.file, item.line, item.code, 
								item.rationale, item.editor_notify, item.error_type, item.backtraces)
	_is_logging_thread = false

func _worker_loop():
	while not _exit_thread:
		_semaphore.wait()
		if _exit_thread: break
		
		_mutex.lock()
		var logs = _queue.duplicate()
		_queue.clear()
		_mutex.unlock()
		
		_process_logs(logs)
		
		_mutex.lock()
		for endpoint in _endpoints:
			endpoint.flush()
		_mutex.unlock()
