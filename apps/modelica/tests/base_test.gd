class_name BaseTest
extends Node

# Test metadata
var test_name: String = ""
var test_class: String = ""
var test_description: String = ""

# Test statistics
var total_tests: int = 0
var passed_tests: int = 0
var failed_tests: int = 0
var skipped_tests: int = 0

# Test results
var results = []
var current_test_name = ""

# Setup and teardown flags
var _setup_done: bool = false
var _teardown_needed: bool = false

# Constructor
func _init():
	test_class = get_script().resource_path.get_file().get_basename()

# Virtual methods that subclasses should override
func setup():
	# Setup code to run before each test
	pass

func teardown():
	# Teardown code to run after each test
	pass

func before_all():
	# Called once before all tests
	pass

func after_all():
	# Called once after all tests
	pass

# Test discovery and execution
func run_tests():
	print("\n=== Running tests for " + test_class + " ===")
	
	# Run setup for all tests
	before_all()
	
	# Find all test methods (starting with "test_")
	var test_methods = []
	for method in get_method_list():
		var method_name = method["name"]
		if method_name.begins_with("test_") and method["args"].size() == 0:
			test_methods.append(method_name)
	
	total_tests = test_methods.size()
	
	# Run each test
	for test_method in test_methods:
		run_single_test(test_method)
	
	# Run cleanup for all tests
	after_all()
	
	# Print summary
	print_summary()
	
	return failed_tests == 0

# Run a single test
func run_single_test(test_method: String):
	current_test_name = test_method
	var result = {
		"name": test_method,
		"passed": false,
		"error": "",
		"execution_time": 0
	}
	
	print("  Running " + test_method + "...")
	
	# Setup
	_setup_done = false
	_teardown_needed = false
	
	var start_time = Time.get_ticks_msec()
	
	# Run the test with error handling
	try:
		# Setup
		setup()
		_setup_done = true
		_teardown_needed = true
		
		# Run the test
		call(test_method)
		
		# Test passed if we got here
		result.passed = true
		passed_tests += 1
		
	except var e:
		# Test failed
		result.passed = false
		result.error = str(e)
		failed_tests += 1
		print("    FAILED: " + str(e))
	finally:
		# Teardown if setup was completed
		if _teardown_needed:
			teardown()
	
	var end_time = Time.get_ticks_msec()
	result.execution_time = end_time - start_time
	
	results.append(result)

# Print test summary
func print_summary():
	print("\n=== Test Summary for " + test_class + " ===")
	print("  Total tests: " + str(total_tests))
	print("  Passed: " + str(passed_tests))
	print("  Failed: " + str(failed_tests))
	print("  Skipped: " + str(skipped_tests))
	
	if failed_tests > 0:
		print("\nFailed tests:")
		for result in results:
			if not result.passed:
				print("  " + result.name + ": " + result.error)
	
	print("\n")

# Assertion methods
func assert_true(condition: bool, message: String = ""):
	if not condition:
		var error_msg = "Assertion failed: Expected true"
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_false(condition: bool, message: String = ""):
	if condition:
		var error_msg = "Assertion failed: Expected false"
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_equal(actual, expected, message: String = ""):
	if actual != expected:
		var error_msg = "Assertion failed: Expected " + str(expected) + " but got " + str(actual)
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_not_equal(actual, expected, message: String = ""):
	if actual == expected:
		var error_msg = "Assertion failed: Expected value to be different from " + str(expected)
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_almost_equal(actual: float, expected: float, tolerance: float = 0.0001, message: String = ""):
	if abs(actual - expected) > tolerance:
		var error_msg = "Assertion failed: Expected " + str(expected) + " but got " + str(actual) + " (tolerance: " + str(tolerance) + ")"
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_null(value, message: String = ""):
	if value != null:
		var error_msg = "Assertion failed: Expected null but got " + str(value)
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_not_null(value, message: String = ""):
	if value == null:
		var error_msg = "Assertion failed: Expected non-null value"
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_has(container, value, message: String = ""):
	if value not in container:
		var error_msg = "Assertion failed: Expected container to include " + str(value)
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_does_not_have(container, value, message: String = ""):
	if value in container:
		var error_msg = "Assertion failed: Expected container to not include " + str(value)
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_has_method(obj: Object, method_name: String, message: String = ""):
	if not obj.has_method(method_name):
		var error_msg = "Assertion failed: Expected object to have method '" + method_name + "'"
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

func assert_throws(callback: Callable, message: String = ""):
	var threw = false
	try:
		callback.call()
	except:
		threw = true
	
	if not threw:
		var error_msg = "Assertion failed: Expected function to throw an error"
		if message:
			error_msg += " - " + message
		_fail_test(error_msg)

# Helper to fail a test
func _fail_test(error_message: String):
	push_error(error_message)
	assert(false, error_message)  # This will throw an error and fail the test

# Static method to run all tests in the tests directory
static func run_all_tests():
	print("Starting test runner...")
	var test_dir = "res://apps/modelica/tests"
	var failed_tests = 0
	
	# Find and run all test files
	failed_tests += _run_tests_in_directory(test_dir + "/lexer")
	failed_tests += _run_tests_in_directory(test_dir + "/parser")
	failed_tests += _run_tests_in_directory(test_dir + "/solver")
	failed_tests += _run_tests_in_directory(test_dir + "/integration")
	failed_tests += _run_tests_in_directory(test_dir + "/cli")
	
	# Print final summary
	print("\n=======================================")
	print("All tests completed.")
	if failed_tests == 0:
		print("✅ All tests passed!")
	else:
		print("❌ " + str(failed_tests) + " test(s) failed!")
	print("=======================================\n")
	
	return failed_tests == 0

# Helper method to run all tests in a directory
static func _run_tests_in_directory(dir_path: String) -> int:
	print("\nRunning tests in: " + dir_path)
	var dir = DirAccess.open(dir_path)
	var failed_tests = 0
	
	if dir:
		dir.list_dir_begin()
		var file_name = dir.get_next()
		
		while file_name != "":
			if file_name.ends_with("_test.gd") or file_name.begins_with("test_"):
				var test_script = load(dir_path + "/" + file_name)
				if test_script:
					var test_instance = test_script.new()
					if test_instance is BaseTest:
						if not test_instance.run_tests():
							failed_tests += 1
					test_instance.queue_free()
			file_name = dir.get_next()
	else:
		print("Error: Could not open directory: " + dir_path)
	
	return failed_tests 