extends "../base_test.gd"

# Mock CLI class for testing
class MockCLI:
	extends Node
	
	var args = []
	var model_name = ""
	var output_format = "csv"
	var output_file = ""
	var simulation_start = 0.0
	var simulation_stop = 10.0
	var simulation_step = 0.01
	var help_called = false
	
	func _init(test_args: Array):
		args = test_args
		_process_args()
	
	func _process_args():
		var i = 0
		while i < args.size():
			var arg = args[i]
			
			if arg == "--script":
				i += 1  # Skip the script path
			elif arg == "--format" or arg == "-f":
				i += 1
				if i < args.size():
					output_format = args[i].to_lower()
			elif arg == "--output" or arg == "-o":
				i += 1
				if i < args.size():
					output_file = args[i]
			elif arg == "--start" or arg == "-s":
				i += 1
				if i < args.size():
					simulation_start = float(args[i])
			elif arg == "--stop" or arg == "-e":
				i += 1
				if i < args.size():
					simulation_stop = float(args[i])
			elif arg == "--step" or arg == "-dt":
				i += 1
				if i < args.size():
					simulation_step = float(args[i])
			elif arg == "--help" or arg == "-h":
				help_called = true
			elif not arg.begins_with("--") and not arg.begins_with("-"):
				model_name = arg
			i += 1

func test_basic_args():
	# Test with just a model name
	var cli = MockCLI.new(["SpringMassDamper"])
	
	assert_equal(cli.model_name, "SpringMassDamper", "Model name should be parsed correctly")
	assert_equal(cli.output_format, "csv", "Default output format should be csv")
	assert_equal(cli.output_file, "", "Default output file should be empty")
	assert_equal(cli.simulation_start, 0.0, "Default start time should be 0.0")
	assert_equal(cli.simulation_stop, 10.0, "Default stop time should be 10.0")
	assert_equal(cli.simulation_step, 0.01, "Default step size should be 0.01")

func test_format_option():
	# Test with format option
	var cli = MockCLI.new(["--format", "json", "SpringMassDamper"])
	
	assert_equal(cli.model_name, "SpringMassDamper", "Model name should be parsed correctly")
	assert_equal(cli.output_format, "json", "Output format should be json")
	
	# Test with short option
	cli = MockCLI.new(["-f", "csv", "SpringMassDamper"])
	
	assert_equal(cli.output_format, "csv", "Output format should be csv")

func test_output_option():
	# Test with output file option
	var cli = MockCLI.new(["--output", "results.csv", "SpringMassDamper"])
	
	assert_equal(cli.model_name, "SpringMassDamper", "Model name should be parsed correctly")
	assert_equal(cli.output_file, "results.csv", "Output file should be set correctly")
	
	# Test with short option
	cli = MockCLI.new(["-o", "results.json", "SpringMassDamper"])
	
	assert_equal(cli.output_file, "results.json", "Output file should be set correctly")

func test_time_options():
	# Test with time options
	var cli = MockCLI.new([
		"--start", "1.0",
		"--stop", "20.0",
		"--step", "0.05",
		"SpringMassDamper"
	])
	
	assert_equal(cli.model_name, "SpringMassDamper", "Model name should be parsed correctly")
	assert_equal(cli.simulation_start, 1.0, "Start time should be set correctly")
	assert_equal(cli.simulation_stop, 20.0, "Stop time should be set correctly")
	assert_equal(cli.simulation_step, 0.05, "Step size should be set correctly")
	
	# Test with short options
	cli = MockCLI.new([
		"-s", "2.0",
		"-e", "30.0",
		"-dt", "0.1",
		"SpringMassDamper"
	])
	
	assert_equal(cli.simulation_start, 2.0, "Start time should be set correctly")
	assert_equal(cli.simulation_stop, 30.0, "Stop time should be set correctly")
	assert_equal(cli.simulation_step, 0.1, "Step size should be set correctly")

func test_help_option():
	# Test with help option
	var cli = MockCLI.new(["--help"])
	
	assert_true(cli.help_called, "Help should be called")
	assert_equal(cli.model_name, "", "Model name should be empty")
	
	# Test with short option
	cli = MockCLI.new(["-h"])
	
	assert_true(cli.help_called, "Help should be called")

func test_mixed_options():
	# Test with mixed options
	var cli = MockCLI.new([
		"--format", "json",
		"--output", "results.json",
		"--start", "5.0",
		"SpringMassDamper"
	])
	
	assert_equal(cli.model_name, "SpringMassDamper", "Model name should be parsed correctly")
	assert_equal(cli.output_format, "json", "Output format should be json")
	assert_equal(cli.output_file, "results.json", "Output file should be set correctly")
	assert_equal(cli.simulation_start, 5.0, "Start time should be set correctly")
	assert_equal(cli.simulation_stop, 10.0, "Stop time should be unchanged")
	assert_equal(cli.simulation_step, 0.01, "Step size should be unchanged")

func test_script_path_handling():
	# Test with script path (should be ignored)
	var cli = MockCLI.new([
		"--script", "apps/modelica/cli.gd",
		"SpringMassDamper"
	])
	
	assert_equal(cli.model_name, "SpringMassDamper", "Model name should be parsed correctly") 