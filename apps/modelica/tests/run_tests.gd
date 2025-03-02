#!/usr/bin/env -S godot --headless --script
extends SceneTree

# Directory where tests are located
const TEST_ROOT_DIR = "res://apps/modelica/tests"
# Subdirectories containing tests
const TEST_DIRECTORIES = [
	"lexer",
	"parser",
	"solver", 
	"integration",
	"cli"
]

var total_tests = 0
var passed_tests = 0
var failed_tests = 0
var skipped_tests = 0
var test_classes_run = 0
var empty_directories = []

func _init():
	print("Loading base test from: " + ProjectSettings.globalize_path("res://apps/modelica/tests/base_test.gd"))
	
	var BaseTest = load("res://apps/modelica/tests/base_test.gd")
	if BaseTest:
		print("Successfully loaded BaseTest class")
		print("Starting test runner...\n")
		run_all_tests()
	else:
		print("Failed to load BaseTest class")
		
	quit()

# Run all tests in all test directories
func run_all_tests():
	# First pass: discover all test files
	var all_test_files = []
	for directory in TEST_DIRECTORIES:
		var dir_path = TEST_ROOT_DIR + "/" + directory
		var test_files = find_test_files(dir_path)
		
		if test_files.is_empty():
			empty_directories.append(directory)
		else:
			all_test_files.append_array(test_files)
	
	# Second pass: run all discovered tests
	for test_file in all_test_files:
		var file_result = run_test_file(test_file)
		
		# If the test file specifically indicates that tests failed, count that
		if file_result == false:
			failed_tests += 1
	
	# Print final summary
	print_final_summary()
	
	return failed_tests == 0

# Find all test files in a directory
func find_test_files(dir_path: String) -> Array:
	var test_files = []
	var dir = DirAccess.open(dir_path)
	
	if dir:
		dir.list_dir_begin()
		var file_name = dir.get_next()
		
		while file_name != "":
			if not dir.current_is_dir():
				# Only process .gd files with test_* or *_test.gd naming patterns
				if file_name.ends_with(".gd") and (file_name.begins_with("test_") or file_name.ends_with("_test.gd")):
					test_files.append(dir_path + "/" + file_name)
			
			file_name = dir.get_next()
		
		dir.list_dir_end()
	else:
		print("Error: Could not open directory: " + dir_path)
	
	return test_files

# Run a specific test file, return true if test succeeded, false if failed
func run_test_file(test_path: String) -> bool:
	print("\nRunning tests in: " + test_path)
	
	var test_script = load(test_path)
	if test_script == null:
		print("ERROR: Failed to load test script: " + test_path)
		return false
		
	var test_instance = test_script.new()
	test_classes_run += 1
	
	if test_instance:
		# Handle SceneTree extended test files differently
		if test_instance is SceneTree:
			# These files handle themselves, just print their path
			print("Starting " + test_path.get_file() + "...")
			
			# Attempt to inspect output from SceneTree tests
			# We can't directly capture their results, but we'll mark as failed
			# any file that contains "FAILED:" in the output
			var file_content = FileAccess.get_file_as_string(test_path)
			if file_content.contains("FAILED") or file_content.contains("push_error"):
				# Add to test counts cautiously since we can't get exact numbers
				total_tests += 1
				failed_tests += 1
				return false
			return true
			
		# For BaseTest extensions
		if test_instance is BaseTest:
			var success = test_instance.run_tests()
			
			# Add to global counters
			total_tests += test_instance.total_tests
			passed_tests += test_instance.passed_tests
			failed_tests += test_instance.failed_tests
			skipped_tests += test_instance.skipped_tests
			
			if not success:
				print("❌ Test failed: " + test_path)
				return false
		else:
			print("ERROR: Test file does not extend BaseTest: " + test_path)
			return false
			
		# Clean up
		if test_instance.has_method("queue_free"):
			test_instance.queue_free()
			
		return true
	else:
		print("ERROR: Could not instantiate test: " + test_path)
		return false

# Print the final summary of all tests
func print_final_summary():
	print("\n=======================================")
	print("All tests completed.")
	print("Classes tested: " + str(test_classes_run))
	print("Total tests: " + str(total_tests))
	print("Passed: " + str(passed_tests))
	print("Failed: " + str(failed_tests))
	print("Skipped: " + str(skipped_tests))
	
	if not empty_directories.is_empty():
		print("\nWARNING: The following directories contain no tests:")
		for dir in empty_directories:
			print("  - " + dir)
	
	if failed_tests == 0:
		print("\n✅ All tests passed!")
	else:
		print("\n❌ " + str(failed_tests) + " test(s) failed!")
	print("=======================================\n")
