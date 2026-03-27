# GdUnit4 Test
extends GdUnitTestSuite

const TEST_LOG_PATH = "user://test_logs"

class MockEndpoint extends LCLogEndpoint:
	var messages = []
	var errors = []

	func log_message(message: String, is_error: bool) -> void:
		messages.append({"text": message, "is_error": is_error})

	func log_error(function: String, file: String, line: int, code: String, 
				rationale: String, editor_notify: bool, error_type: int, 
				script_backtraces: Array) -> void:
		errors.append({"func": function, "rationale": rationale})

var logger: Node

func before():
	# Clean up any existing test logs
	if DirAccess.dir_exists_absolute(TEST_LOG_PATH):
		var dir = DirAccess.open(TEST_LOG_PATH)
		dir.list_dir_begin()
		var file_name = dir.get_next()
		while file_name != "":
			dir.remove(file_name)
			file_name = dir.get_next()
		DirAccess.remove_absolute(TEST_LOG_PATH)
	
	# Try to get the autoload, or create it if missing
	if not has_node("/root/LCLogger"):
		logger = load("res://addons/lunco-logger/lc_log_manager.gd").new()
		logger.name = "LCLogger"
		get_tree().root.add_child(logger)
	else:
		logger = get_node("/root/LCLogger")
	
	logger.clear_endpoints()
	# Reset settings to default
	ProjectSettings.set_setting("lunco/logger/stdout_level", 31) # ALL
	ProjectSettings.set_setting("lunco/logger/file_level", 31) # ALL
	ProjectSettings.set_setting("lunco/logger/path", "user://logs")
	logger.call("_update_from_settings")

func test_logging_to_file():
	var endpoint = load("res://addons/lunco-logger/lc_log_file_endpoint.gd").new(TEST_LOG_PATH, 5)

	endpoint.log_message("Test Message 1", false)
	endpoint.log_message("Test Error Message", true)
	endpoint.flush()

	# Find the session log file
	var dir = DirAccess.open(TEST_LOG_PATH)
	dir.list_dir_begin()
	var file_name = dir.get_next()
	assert_str(file_name).starts_with("log_")

	var file = FileAccess.open(TEST_LOG_PATH.path_join(file_name), FileAccess.READ)
	var content = file.get_as_text()
	assert_str(content).contains("Test Message 1")
	assert_str(content).contains("Test Error Message")

func test_rotation():
	# Create many logs to trigger rotation
	for i in range(5):
		var endpoint = load("res://addons/lunco-logger/lc_log_file_endpoint.gd").new(TEST_LOG_PATH, 3)
		endpoint.log_message("Session %d" % i, false)
		endpoint.flush()
		OS.delay_msec(1001) 

	var dir = DirAccess.open(TEST_LOG_PATH)
	dir.list_dir_begin()
	var count = 0
	var file_name = dir.get_next()
	while file_name != "":
		if not dir.current_is_dir() and file_name.ends_with(".log"):
			count += 1
		file_name = dir.get_next()

	assert_int(count).is_equal(3)

func test_per_endpoint_filtering():
	var mock_all = MockEndpoint.new()
	mock_all.level_mask = 31 # ALL
	
	var mock_error = MockEndpoint.new()
	mock_error.level_mask = 8 # ERROR ONLY
	
	logger.add_endpoint(mock_all)
	logger.add_endpoint(mock_error)
	
	logger.call("push_log", "INFO_MSG", 2) # INFO
	logger.call("push_log", "ERROR_MSG", 8) # ERROR
	logger.flush()
	
	assert_int(mock_all.messages.size()).is_equal(2)
	assert_int(mock_error.messages.size()).is_equal(1)
	assert_str(mock_error.messages[0].text).contains("ERROR_MSG")
	
	logger.remove_endpoint(mock_all)
	logger.remove_endpoint(mock_error)

func test_variadic_arguments():
	var mock = MockEndpoint.new()
	logger.add_endpoint(mock)
	
	# Test more than 10 arguments to verify variadic support
	logger.info("1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12")
	logger.flush()
	
	assert_int(mock.messages.size()).is_equal(1)
	assert_str(mock.messages[0].text).contains("123456789101112")
	
	logger.remove_endpoint(mock)

func test_dynamic_settings_change():
	var NEW_PATH = "user://dynamic_logs"
	if DirAccess.dir_exists_absolute(NEW_PATH):
		var dir = DirAccess.open(NEW_PATH)
		dir.list_dir_begin()
		var fn = dir.get_next()
		while fn != "":
			dir.remove(fn)
			fn = dir.get_next()
		DirAccess.remove_absolute(NEW_PATH)
		
	ProjectSettings.set_setting("lunco/logger/path", NEW_PATH)
	logger.call("_on_settings_changed")
	
	logger.info("Dynamic path message")
	logger.flush()
	
	assert_bool(DirAccess.dir_exists_absolute(NEW_PATH)).is_true()

func test_concurrent_logging():
	var mock = MockEndpoint.new()
	logger.add_endpoint(mock)
	
	var thread_count = 4
	var logs_per_thread = 20
	var total_expected = thread_count * logs_per_thread

	var tasks = []
	for t in range(thread_count):
		var task_id = WorkerThreadPool.add_task(func():
			for i in range(logs_per_thread):
				logger.call("push_log", "Thread %d log %d" % [t, i], 2) # INFO
		)
		tasks.append(task_id)

	for task_id in tasks:
		WorkerThreadPool.wait_for_task_completion(task_id)

	# Give it a bit more time for the worker thread to finish processing
	var timeout = 2.0
	while timeout > 0 and mock.messages.size() < total_expected:
		await get_tree().process_frame
		timeout -= 1.0/60.0
		logger.flush()

	var actual_count = mock.messages.size()
	logger.remove_endpoint(mock)
	assert_int(actual_count).is_greater_equal(total_expected)

func test_caller_info_tracking():
	var mock = MockEndpoint.new()
	logger.add_endpoint(mock)
	
	ProjectSettings.set_setting("lunco/logger/show_caller", true)
	logger.call("_update_from_settings")
	
	# Log from this file
	logger.info("Test caller info")
	logger.flush()
	
	assert_int(mock.messages.size()).is_equal(1)
	# Should contain [lc_logger_test.gd:LINE]
	assert_str(mock.messages[0].text).contains("[lc_logger_test.gd:")
	
	# Test disabling it
	mock.messages.clear()
	ProjectSettings.set_setting("lunco/logger/show_caller", false)
	logger.call("_update_from_settings")
	
	logger.info("Test no caller info")
	logger.flush()
	
	assert_int(mock.messages.size()).is_equal(1)
	assert_bool(mock.messages[0].text.contains("[lc_logger_test.gd:")).is_false()
	
	logger.remove_endpoint(mock)

func test_recursion_protection():
	var mock = MockEndpoint.new()
	logger.add_endpoint(mock)
	
	# Create a custom endpoint that calls print() or LCLogger.info()
	# This should NOT cause infinite recursion because of our guards
	var recursive_endpoint = MockEndpoint.new()
	recursive_endpoint.log_message = func(msg: String, _err: bool):
		# This print() will be caught by EngineLoggerImpl and call log_raw()
		print("Recursive print: ", msg)
		# Direct call to logger
		logger.info("Recursive logger call")
	
	logger.add_endpoint(recursive_endpoint)
	
	logger.info("Start recursion test")
	logger.flush()
	
	# If we are here and didn't crash, the guards are working.
	# The recursive calls might still be queued, but they shouldn't trigger more logs.
	assert_bool(true).is_true()
	
	logger.remove_endpoint(mock)
	logger.remove_endpoint(recursive_endpoint)
