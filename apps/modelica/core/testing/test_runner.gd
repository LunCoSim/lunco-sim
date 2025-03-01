class_name TestRunner
extends Node

const BaseTest = preload("res://apps/modelica/core/testing/base_test.gd")

# Statistics across all test suites
var total_test_suites: int = 0
var passed_test_suites: int = 0
var failed_test_suites: int = 0
var total_tests: int = 0
var passed_tests: int = 0
var failed_tests: int = 0

# List of test suite scripts
var test_suites = []

func _init():
	pass

# Run all tests in the given directories
func run_tests(test_dirs: Array = ["res://apps/modelica/tests"]) -> int:
	print("\n===== MODELICA TEST RUNNER =====\n")
	print("Discovering tests...")
	
	# Discover test files
	for dir_path in test_dirs:
		discover_tests(dir_path)
	
	total_test_suites = test_suites.size()
	print("Found " + str(total_test_suites) + " test suites\n")
	
	# Run test suites
	for test_script in test_suites:
		run_test_suite(test_script)
	
	# Print overall summary
	print_summary()
	
	return failed_tests

# Recursively discover test files in directories
func discover_tests(dir_path: String):
	var dir = DirAccess.open(dir_path)
	if not dir:
		push_error("Failed to open directory: " + dir_path)
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	
	while file_name != "":
		if dir.current_is_dir():
			# Recursively search subdirectories
			if not file_name in [".", ".."]:
				discover_tests(dir_path.path_join(file_name))
		else:
			# Check if it's a test file
			if file_name.begins_with("test_") and file_name.ends_with(".gd"):
				var script_path = dir_path.path_join(file_name)
				var script = load(script_path)
				
				# Check if it's a valid test script (extends BaseTest)
				if script and script.new() is BaseTest:
					test_suites.append(script)
		
		file_name = dir.get_next()
	
	dir.list_dir_end()

# Run a single test suite
func run_test_suite(test_script):
	var test_instance = test_script.new()
	
	print("Running test suite: " + test_instance.test_class)
	var success = test_instance.run_tests()
	
	# Update statistics
	if success:
		passed_test_suites += 1
	else:
		failed_test_suites += 1
	
	total_tests += test_instance.total_tests
	passed_tests += test_instance.passed_tests
	failed_tests += test_instance.failed_tests
	
	print("\n")

# Print summary of all test results
func print_summary():
	print("===== TEST SUMMARY =====")
	print("Test suites: " + str(total_test_suites))
	print("  Passed: " + str(passed_test_suites))
	print("  Failed: " + str(failed_test_suites))
	print("\nTotal tests: " + str(total_tests))
	print("  Passed: " + str(passed_tests))
	print("  Failed: " + str(failed_tests))
	
	if failed_tests == 0:
		print("\nALL TESTS PASSED!\n")
	else:
		print("\nSOME TESTS FAILED!\n")

# Entry point when run directly
func _run():
	var exit_code = run_tests()
	OS.exit(exit_code) 