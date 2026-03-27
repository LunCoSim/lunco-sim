extends LCLogEndpoint
class_name LCLogFileEndpoint

var _base_path: String
var _max_files: int
var _file_path: String
var _file: FileAccess
var _mutex: Mutex

func _init(path: String, max_files: int):
	_mutex = Mutex.new()
	_base_path = path
	_max_files = max_files
	_initialize_file()

func _initialize_file():
	_mutex.lock()
	if _file:
		_file.close()
	
	_file_path = _get_session_path(_base_path)
	_ensure_dir(_base_path)
	_rotate_logs(_base_path, _max_files)
	_file = FileAccess.open(_file_path, FileAccess.WRITE)
	_mutex.unlock()

func update_configuration(new_path: String, new_max_files: int):
	_mutex.lock()
	var changed = (new_path != _base_path)
	_base_path = new_path
	_max_files = new_max_files
	_mutex.unlock()
	
	if changed:
		_initialize_file()

func _get_session_path(base_path: String) -> String:
	var time = Time.get_datetime_dict_from_system()
	var stamp = "%04d-%02d-%02d_%02d-%02d-%02d" % [
		time.year, time.month, time.day,
		time.hour, time.minute, time.second
	]
	return base_path.path_join("log_%s.log" % stamp)

func _ensure_dir(path: String):
	if not DirAccess.dir_exists_absolute(path):
		DirAccess.make_dir_recursive_absolute(path)

func _rotate_logs(path: String, max_files: int):
	var dir = DirAccess.open(path)
	if not dir: return
	
	dir.list_dir_begin()
	var files = []
	var file_name = dir.get_next()
	while file_name != "":
		if not dir.current_is_dir() and file_name.begins_with("log_") and file_name.ends_with(".log"):
			files.append(file_name)
		file_name = dir.get_next()
	
	files.sort()
	while files.size() >= max_files:
		var to_delete = files.pop_front()
		dir.remove(to_delete)

func log_message(message: String, _is_error: bool, _is_engine: bool = false) -> void:
	# Ensure message is not empty or just a newline
	var stripped = message.strip_edges()
	if stripped.is_empty():
		return
		
	_mutex.lock()
	if _file:
		_file.store_line(message)
	_mutex.unlock()

func log_error(function: String, file: String, line: int, code: String, 
			rationale: String, _editor_notify: bool, error_type: int, 
			_script_backtraces: Array, is_engine: bool = false) -> void:
	
	var type_str = "ERROR" if error_type == 0 else "WARNING"
	var entry = "[%s] %s:%d in %s() - %s: %s" % [
		type_str, file, line, function, code, rationale
	]
	log_message(entry, error_type == 0, is_engine)

func flush() -> void:
	_mutex.lock()
	if _file:
		_file.flush()
	_mutex.unlock()
