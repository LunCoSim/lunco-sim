@tool
extends SceneTree

const ModelManager = preload("../model_manager.gd")
const MOParser = preload("../mo_parser.gd")
const PackageManager = preload("../package_manager.gd")
const WorkspaceConfig = preload("../workspace_config.gd")

var test_root: Node
var model_manager: ModelManager
var package_manager: PackageManager
var parser: MOParser
var workspace_config: WorkspaceConfig

func _init() -> void:
	print("\nStarting Model Import Tests...")
	test_root = Node.new()
	get_root().add_child(test_root)
	_run_tests()
	quit()

func _run_tests() -> void:
	_setup()
	test_model_import()
	_teardown()
	print("Tests completed.")

func _setup() -> void:
	print("Setting up test environment...")
	
	# Initialize workspace config
	workspace_config = WorkspaceConfig.new()
	workspace_config.initialize(ProjectSettings.globalize_path("res://apps/modelica_godot"))
	
	# Initialize managers
	model_manager = ModelManager.new()
	package_manager = PackageManager.new()
	parser = MOParser.new()
	
	test_root.add_child(model_manager)
	test_root.add_child(package_manager)
	test_root.add_child(parser)
	
	model_manager.initialize()
	print("Setup complete.")

func _teardown() -> void:
	print("Cleaning up...")
	if model_manager:
		model_manager.queue_free()
	if package_manager:
		package_manager.queue_free()
	if parser:
		parser.queue_free()
	if test_root:
		test_root.queue_free()
	print("Cleanup complete.")

func test_model_import() -> void:
	print("\nTesting model import functionality...")
	
	# Test package loading
	var package_path = ProjectSettings.globalize_path("res://apps/modelica_godot/components")
	assert_true(package_manager.load_package(package_path), "Package loaded successfully")
	
	# Test mechanical package
	assert_true(package_manager.has_package("Mechanical"), "Mechanical package exists")
	
	# Test model import
	var model_path = ProjectSettings.globalize_path("res://apps/modelica_godot/components/Mechanical/DampingMassTest.mo")
	var file = FileAccess.open(model_path, FileAccess.READ)
	assert_not_null(file, "Model file exists")
	
	if file:
		var content = file.get_as_text()
		file.close()
		
		# Parse model
		var model_data = parser.parse_text(content)
		assert_not_null(model_data, "Model parsed successfully")
		assert_false(model_data.is_empty(), "Model data not empty")
		
		if not model_data.is_empty():
			# Test model manager integration
			model_manager._add_model_to_tree(model_data)
			assert_true(model_manager.has_model("DampingMassTest"), "Model added to manager")
			
			# Test component dependencies
			var components = ["Mass", "Damper", "Fixed"]
			for component in components:
				assert_true(model_manager.has_component(component), "Component " + component + " available")
			
			# Test model parameters
			var params = model_manager.get_model_parameters(model_data)
			assert_not_null(params, "Model parameters exist")
			assert_true(params.has("x0"), "Has x0 parameter")
			assert_true(params.has("v0"), "Has v0 parameter")
			
			# Test experiment settings
			var settings = model_manager.get_experiment_settings(model_data)
			assert_not_null(settings, "Experiment settings exist")
			assert_eq(settings.get("StartTime"), 0.0, "Correct start time")
			assert_eq(settings.get("StopTime"), 10.0, "Correct stop time")
			assert_eq(settings.get("Interval"), 0.1, "Correct interval")
	
	print("Model import tests completed successfully")

func assert_true(condition: bool, message: String) -> void:
	if not condition:
		push_error("Assertion failed: " + message)
		return
	print("  ✓ " + message)

func assert_false(condition: bool, message: String) -> void:
	if condition:
		push_error("Assertion failed: " + message)
		return
	print("  ✓ " + message)

func assert_not_null(value, message: String) -> void:
	if value == null:
		push_error("Assertion failed: " + message + " (value is null)")
		return
	print("  ✓ " + message)

func assert_eq(actual, expected, message: String) -> void:
	if actual != expected:
		push_error("Assertion failed: " + message + "\nExpected: " + str(expected) + "\nActual: " + str(actual))
		return
	print("  ✓ " + message)	