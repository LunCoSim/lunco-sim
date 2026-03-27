extends Node

const SETTING_PATH = "lunco/logger/path"
const SETTING_MAX_FILES = "lunco/logger/max_files"
const SETTING_STDOUT_LEVEL = "lunco/logger/stdout_level"
const SETTING_FILE_LEVEL = "lunco/logger/file_level"
const SETTING_SHOW_CALLER = "lunco/logger/show_caller"

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
var _show_caller: bool = true
var _internal_logging_threads_mutex: Mutex
var _internal_logging_threads: Dictionary = {}
var _global_class_list: Array = []

func _set_internal_logging(value: bool):
	if _internal_logging_threads_mutex == null: return
	var tid = OS.get_thread_caller_id()
	_internal_logging_threads_mutex.lock()
	if value:
		_internal_logging_threads[tid] = true
	else:
		_internal_logging_threads.erase(tid)
	_internal_logging_threads_mutex.unlock()

func _is_internal_logging() -> bool:
	if _internal_logging_threads_mutex == null: return false
	var tid = OS.get_thread_caller_id()
	_internal_logging_threads_mutex.lock()
	var result = _internal_logging_threads.has(tid)
	_internal_logging_threads_mutex.unlock()
	return result

class EngineLoggerImpl extends Logger:
	var _manager_ref: WeakRef
	
	func _init(manager: Node):
		_manager_ref = weakref(manager)

	func _log_message(message: String, is_error: bool) -> void:
		var manager = _manager_ref.get_ref()
		if is_instance_valid(manager):
			# If this is the manager's own worker thread, ignore to prevent recursion
			if manager._is_internal_logging():
				return
			var level = LogLevel.ERROR if is_error else LogLevel.INFO
			manager.log_raw(message, level)

	func _log_error(function: String, file: String, line: int, code: String, 
					rationale: String, editor_notify: bool, error_type: int, 
					script_backtraces: Array[ScriptBacktrace]) -> void:
		var manager = _manager_ref.get_ref()
		if is_instance_valid(manager):
			if manager._is_internal_logging():
				return
			var level = LogLevel.ERROR if error_type == 0 else LogLevel.WARN
			manager.push_error_log(level, function, file, line, code, rationale, editor_notify, error_type, script_backtraces, true)

func _init():
	_mutex = Mutex.new()
	_semaphore = Semaphore.new()
	_internal_logging_threads_mutex = Mutex.new()

func _enter_tree() -> void:
	_setup_settings()
	
	_update_from_settings()
	ProjectSettings.settings_changed.connect(_on_settings_changed)
	
	_thread_task_id = WorkerThreadPool.add_task(_worker_loop)
	
	_global_class_list = ProjectSettings.get_global_class_list()
	
	OS.add_logger(EngineLoggerImpl.new(self))

func _exit_tree() -> void:
	_exit_thread = true
	if _semaphore: _semaphore.post()
	if _thread_task_id != -1:
		WorkerThreadPool.wait_for_task_completion(_thread_task_id)
	
	flush()
	
	_mutex = null
	_semaphore = null
	_internal_logging_threads_mutex = null

func _on_settings_changed():
	_update_from_settings()

func _update_from_settings():
	if _mutex == null: return
	var path = ProjectSettings.get_setting(SETTING_PATH, "user://logs")
	var max_files = ProjectSettings.get_setting(SETTING_MAX_FILES, 5)
	var stdout_level = ProjectSettings.get_setting(SETTING_STDOUT_LEVEL, LogLevel.ALL)
	var file_level = ProjectSettings.get_setting(SETTING_FILE_LEVEL, LogLevel.ALL)
	_show_caller = ProjectSettings.get_setting(SETTING_SHOW_CALLER, true)
	
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
	_set_setting_default(SETTING_SHOW_CALLER, true)

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
	# Recursion check: if this thread is already logging, ignore to prevent infinite loops
	if _is_internal_logging():
		return

	_set_internal_logging(true)

	var parts = []
	for a in args:
		if typeof(a) == TYPE_DICTIONARY or typeof(a) == TYPE_ARRAY:
			parts.append(JSON.stringify(a))
		else:
			parts.append(str(a))

	var message = "".join(parts).strip_edges()
	if message.is_empty():
		_set_internal_logging(false)
		return

	var time = _get_timestamp()
	var level_names = LogLevel.keys()
	var level_values = LogLevel.values()
	var level_idx = level_values.find(level)
	var level_str = level_names[level_idx] if level_idx != -1 else "UNKNOWN"
	
	var caller_info = _get_caller_info()
	var formatted = _format_log(time, level_str, caller_info, message)

	push_log(formatted, level, false)
	_set_internal_logging(false)

func _get_timestamp() -> String:
	var time_dict = Time.get_time_dict_from_system()
	var msec = int(Time.get_unix_time_from_system() * 1000) % 1000
	return "%02d:%02d:%02d.%03d" % [time_dict.hour, time_dict.minute, time_dict.second, msec]

func _get_caller_info() -> Dictionary:
	if not _show_caller:
		return {}
		
	var stack = get_stack()
	if stack.is_empty():
		return {}
		
	var caller_frame = {}
	
	# Skip stack frames that are inside the logger manager
	var manager_script = get_script().get_path()
	for i in range(stack.size()):
		var s = stack[i]
		if s.source != manager_script:
			caller_frame = s
			break
	
	if not caller_frame.is_empty():
		var source = caller_frame.source
		var function = caller_frame.function
		var line = caller_frame.line
		
		# Handle in-memory scripts (gdscript://...)
		var file = source.get_file()
		if file.is_empty() and source.begins_with("gdscript://"):
			file = source.replace("gdscript://", "mem://")
		
		# Try to find class_name
		var class_name_str = ""
		for c in _global_class_list:
			if c.path == source:
				class_name_str = c.class
				break
		
		var display_name = class_name_str if not class_name_str.is_empty() else file
		return {
			"display_name": display_name,
			"function": function,
			"line": line
		}
		
	return {}

func _format_log(time: String, level_str: String, caller_info: Dictionary, message: String) -> String:
	if caller_info.is_empty():
		return "[%s][%s] %s" % [time, level_str, message]
	else:
		return "[%s][%s][%s:%s:%d] %s" % [
			time, 
			level_str, 
			caller_info.display_name, 
			caller_info.function, 
			caller_info.line, 
			message
		]

# For engine redirection
func log_raw(message: String, level: int):
	if _is_internal_logging() or _mutex == null:
		return
		
	var stripped = message.strip_edges()
	if stripped.is_empty():
		return

	_set_internal_logging(true)
	
	var time = _get_timestamp()
	var level_names = LogLevel.keys()
	var level_values = LogLevel.values()
	var level_idx = level_values.find(level)
	var level_str = level_names[level_idx] if level_idx != -1 else "UNKNOWN"
	
	var caller_info = _get_caller_info()
	var formatted = _format_log(time, level_str, caller_info, stripped)
	
	push_log(formatted, level, true)
	_set_internal_logging(false)

func push_log(message: String, level: int, is_engine: bool = false):
	if _mutex == null: return
	_mutex.lock()
	_queue.push_back({ "type": "msg", "text": message, "level": level, "is_engine": is_engine })
	_mutex.unlock()
	_semaphore.post()

func push_error_log(level, function, file, line, code, rationale, editor_notify, error_type, script_backtraces, is_engine: bool = true):
	if _mutex == null: return
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
		"backtraces": script_backtraces,
		"is_engine": is_engine
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
	_set_internal_logging(true)
	_mutex.lock()
	var current_endpoints = _endpoints.duplicate()
	_mutex.unlock()
	
	for item in logs:
		for endpoint in current_endpoints:
			if not endpoint.should_log(item.level):
				continue
				
			if item.type == "msg":
				endpoint.log_message(item.text, item.level >= LogLevel.ERROR, item.is_engine)
			else:
				endpoint.log_error(item.function, item.file, item.line, item.code, 
								item.rationale, item.editor_notify, item.error_type, item.backtraces, item.is_engine)
	_set_internal_logging(false)

func _worker_loop():
	while not _exit_thread:
		if _semaphore == null: break
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
