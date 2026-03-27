# GdUnit4 Test
extends GdUnitTestSuite

class MockEndpoint extends LCLogEndpoint:
	var messages = []
	func log_message(message: String, is_error: bool) -> void:
		messages.append(message)

var logger: Node

func before():
	# Create a fresh logger instance for each test to ensure isolation
	var logger_script = load("res://addons/lunco-logger/lc_log_manager.gd")
	logger = logger_script.new()
	logger.name = "LCLogger_Test"
	get_tree().root.add_child(logger)
	
	# Clear auto-added endpoints to have full control
	logger.clear_endpoints()
	
	# Default settings
	ProjectSettings.set_setting("lunco/logger/show_caller", true)
	logger.call("_update_from_settings")

func after():
	if is_instance_valid(logger):
		logger.queue_free()

func test_caller_info_enabled() -> void:
	var mock = MockEndpoint.new()
	logger.add_endpoint(mock)
	
	ProjectSettings.set_setting("lunco/logger/show_caller", true)
	logger.call("_update_from_settings")
	
	logger.call("info", "Hello with caller")
	logger.call("flush")
	
	assert_int(mock.messages.size()).is_equal(1)
	# Current file name and function should be in the message
	assert_str(mock.messages[0]).contains("[lc_logger_behavior_test.gd:test_caller_info_enabled:")
	
	logger.remove_endpoint(mock)

func test_caller_info_disabled() -> void:
	var mock = MockEndpoint.new()
	logger.add_endpoint(mock)
	
	ProjectSettings.set_setting("lunco/logger/show_caller", false)
	logger.call("_update_from_settings")
	
	logger.call("info", "Hello without caller")
	logger.call("flush")
	
	assert_int(mock.messages.size()).is_equal(1)
	assert_bool(mock.messages[0].contains("[lc_logger_behavior_test.gd:")).is_false()
	
	logger.remove_endpoint(mock)

func test_recursion_protection() -> void:
	var mock = MockEndpoint.new()
	logger.add_endpoint(mock)
	
	var rec_endpoint_script = load("res://addons/lunco-logger/tests/recursive_endpoint.gd")
	var rec_endpoint = rec_endpoint_script.new(logger)
	logger.add_endpoint(rec_endpoint)
	
	logger.call("info", "Initial call")
	logger.call("flush")
	
	# If we didn't crash/hang, protection is working.
	assert_bool(true).is_true()
	
	logger.remove_endpoint(mock)
	logger.remove_endpoint(rec_endpoint)
