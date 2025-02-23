@tool
extends SceneTree

const MOParser = preload("../mo_parser.gd")
const PackageManager = preload("../package_manager.gd")
const ModelManager = preload("../model_manager.gd")

var parser: MOParser
var package_manager: PackageManager
var model_manager: ModelManager
var test_msl_path: String

func _init() -> void:
	print("\nStarting Model Import Tests...")
	_setup()
	test_model_import()
	_cleanup()
	quit()

func _setup() -> void:
	print("Setting up test environment...")
	
	# Initialize components
	parser = MOParser.new()
	package_manager = PackageManager.new()
	model_manager = ModelManager.new()
	
	# Add to scene tree
	root.add_child(package_manager)
	root.add_child(model_manager)
	
	# Set up test paths
	test_msl_path = ProjectSettings.globalize_path("res://apps/modelica_godot/components")

func test_model_import() -> void:
	print("\nTesting model import functionality...")
	
	# Test package loading
	assert_true(package_manager.load_package(test_msl_path), "Package loaded successfully")
	
	# Test mechanical package
	assert_true(package_manager.has_package("Mechanical"), "Mechanical package exists")
	
	# Test model import
	var model_path = test_msl_path.path_join("Mechanical/DampingMassTest.mo")
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

func _cleanup() -> void:
	print("\nCleaning up...")
	if is_instance_valid(package_manager):
		package_manager.queue_free()
	if is_instance_valid(model_manager):
		model_manager.queue_free()
	if is_instance_valid(parser):
		parser.free()
	print("Cleanup complete.")
	print("Tests completed.")

# Test helper functions
func assert_true(condition: bool, message: String = "") -> void:
	if not condition:
		push_error("Assertion failed: " + message)

func assert_false(condition: bool, message: String = "") -> void:
	if condition:
		push_error("Assertion failed: " + message)

func assert_eq(a: Variant, b: Variant, message: String = "") -> void:
	if a != b:
		push_error("Assertion failed: " + message + " (Expected " + str(b) + ", got " + str(a) + ")")

func assert_not_null(value: Variant, message: String = "") -> void:
	if value == null:
		push_error("Assertion failed: " + message + " (Got null)")	