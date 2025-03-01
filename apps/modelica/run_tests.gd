#!/usr/bin/env -S godot --headless --script
extends SceneTree

const TestRunner = preload("res://apps/modelica/core/testing/test_runner.gd")

func _init():
	print("Modelica Test Runner")
	
	# Process command line arguments
	var args = OS.get_cmdline_args()
	var test_dirs = []
	var filter = ""
	var i = 0
	
	while i < args.size():
		var arg = args[i]
		
		if arg == "--script":
			i += 1  # Skip the script path
		elif arg == "--dir" or arg == "-d":
			i += 1
			if i < args.size():
				test_dirs.append("res://" + args[i])
		elif arg == "--filter" or arg == "-f":
			i += 1
			if i < args.size():
				filter = args[i]
		elif arg == "--help" or arg == "-h":
			_print_usage()
			quit(0)
		i += 1
	
	# If no test directories specified, use default
	if test_dirs.empty():
		test_dirs = ["res://apps/modelica/tests"]
	
	# Run tests
	var runner = TestRunner.new()
	var exit_code = runner.run_tests(test_dirs)
	
	# Exit with number of failed tests as exit code
	quit(exit_code)

func _print_usage():
	print("Usage: godot --headless --script run_tests.gd [options]")
	print("Options:")
	print("  --dir, -d <directory>    Test directory to run (can specify multiple)")
	print("  --filter, -f <pattern>   Only run tests matching this pattern")
	print("  --help, -h               Show this help message") 