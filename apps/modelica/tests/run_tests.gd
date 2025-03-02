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
	# First, discover all test files
	var all_test_files = []
	for directory in TEST_DIRECTORIES:
		var dir_path = TEST_ROOT_DIR + "/" + directory
		var test_files = find_test_files(dir_path)
		
		if test_files.is_empty():
			empty_directories.append(directory)
		else:
			all_test_files.append_array(test_files)
	
	# Group tests by type to better manage resources
	var scene_tree_tests = []
	var base_tests = []
	
	# Identify test types
	for test_file in all_test_files:
		var file_path = ProjectSettings.globalize_path(test_file)
		if file_is_scene_tree_test(test_file):
			scene_tree_tests.append(test_file)
		else:
			base_tests.append(test_file)
	
	# Run regular BaseTest tests first (these are more reliable)
	for test_file in base_tests:
		run_base_test(test_file)
	
	# Run SceneTree tests afterward
	for test_file in scene_tree_tests:
		run_scene_tree_test(test_file)
	
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

# Determine if a test file is a SceneTree test by examining its content
func file_is_scene_tree_test(file_path: String) -> bool:
	var file = FileAccess.open(file_path, FileAccess.READ)
	if file:
		var content = file.get_as_text()
		file.close()
		
		# Check if the file directly extends SceneTree
		return content.contains("extends SceneTree") and not content.contains("extends \"res://apps/modelica/tests/base_test.gd\"")
	
	return false

# Run a regular BaseTest test
func run_base_test(test_path: String) -> bool:
	print("\nRunning BaseTest in: " + test_path)
	
	var test_script = load(test_path)
	if test_script == null:
		print("ERROR: Failed to load test script: " + test_path)
		return false
		
	var test_instance = test_script.new()
	test_classes_run += 1
	
	if test_instance:
		# Check if it extends BaseTest using a different approach
		var BaseTest = load("res://apps/modelica/tests/base_test.gd")
		if BaseTest != null:
			# Try to run the test - will catch errors if not compatible
			if test_instance.has_method("run_tests"):
				var success = test_instance.run_tests()
				
				# Check if the instance has the expected properties from BaseTest
				if "total_tests" in test_instance and "passed_tests" in test_instance:
					# Add to global counters
					total_tests += test_instance.total_tests
					passed_tests += test_instance.passed_tests
					failed_tests += test_instance.failed_tests
					skipped_tests += test_instance.skipped_tests
					
					if not success:
						print("❌ FAILED: " + test_path + " (Failed tests: " + str(test_instance.failed_tests) + ")")
						if test_instance.has_method("get_failed_test_names") and test_instance.get_failed_test_names():
							print("   Failed tests: " + str(test_instance.get_failed_test_names()))
						return false
					else:
						print("✅ PASSED: " + test_path + " (Tests: " + str(test_instance.total_tests) + ")")
				else:
					print("ERROR: Test does not have required BaseTest properties: " + test_path)
					return false
			else:
				print("ERROR: Test file does not have run_tests method: " + test_path)
				return false
		else:
			print("ERROR: Could not load BaseTest class")
			return false
			
		# Clean up
		if test_instance.has_method("queue_free"):
			test_instance.queue_free()
			
		return true
	else:
		print("ERROR: Could not instantiate test: " + test_path)
		return false

# Run a SceneTree test in a smarter way
func run_scene_tree_test(test_path: String) -> bool:
	print("\nRunning SceneTree test in: " + test_path)
	
	# Get the absolute path for this test
	var abs_test_path = ProjectSettings.globalize_path(test_path)
	
	# Create a unique temporary directory for this test run to ensure isolation
	var timestamp = Time.get_unix_time_from_system()
	var tmp_dir = "/tmp/godot_test_" + str(timestamp).md5_text()
	
	# Create a shell command to ensure the directory exists
	OS.execute("mkdir", ["-p", tmp_dir], [], true)
	
	# Arguments for Godot with optimizations for test execution
	var args = [
		"--headless",                    # No UI needed
		"--script", abs_test_path,       # The test to run
		"--test-suite-mode",             # Special flag for our tests
		"--no-window",                   # Ensure no window is created 
		"--path", ProjectSettings.globalize_path("res://"),  # Ensure proper path
		"--quiet"                        # Reduce noise
	]
	
	# Improve isolation by using a dedicated user directory
	args.append("--userdir")
	args.append(tmp_dir)
	
	# Run the test and capture output
	var output = []
	var exit_code = OS.execute("godot", args, output, true)
	
	# Analyze the output
	var output_str = output[0] if output.size() > 0 else ""
	
	# Count this as a test
	total_tests += 1
	test_classes_run += 1
	
	# Clean up resources
	OS.execute("rm", ["-rf", tmp_dir], [], true)
	
	# Determine test success/failure
	if exit_code != 0 or output_str.contains("❌ FAILED") or output_str.contains("push_error") or output_str.contains("ERROR:") or output_str.contains("FAILED:"):
		failed_tests += 1
		print("❌ FAILED: " + test_path)
		print("Output: " + output_str.substr(0, min(output_str.length(), 500)) + "...")
		return false
	else:
		# Look for explicit PASSED indicator in the test output
		if output_str.contains("✅") or output_str.contains("PASSED") or output_str.contains("All tests passed"):
			passed_tests += 1
			print("✅ PASSED: " + test_path)
			return true
		else:
			# Some tests might not explicitly output success indicators
			# If there were no errors, we'll assume success
			passed_tests += 1
			print("✅ PASSED (implicit): " + test_path)  
			return true

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
