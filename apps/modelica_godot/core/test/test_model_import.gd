@tool
extends SceneTree

const MOParser = preload("../parser/mo_parser.gd")
const PackageLoader = preload("../loader/package_loader.gd")
const ModelManager = preload("../model_manager.gd")

var tests_run := 0
var tests_passed := 0
var parser: MOParser
var package_loader: PackageLoader
var model_manager: ModelManager
var test_msl_path: String

func _init():
	print("\nRunning Model Import Tests...")
	_run_all_tests()
	print("\nTests completed: %d/%d passed" % [tests_passed, tests_run])
	quit()

func _run_all_tests() -> void:
	_test_package_loading()
	_test_msl_loading()
	_test_component_loading()

func _start_test(test_name: String) -> void:
	print("\nRunning test: " + test_name)
	tests_run += 1
	
	# Reset package loader for each test
	package_loader = PackageLoader.new()
	add_child(package_loader)

func _assert(condition: bool, message: String) -> void:
	if condition:
		tests_passed += 1
		print("  ✓ " + message)
	else:
		print("  ✗ " + message)
		push_error("Test failed: " + message)

func _test_package_loading() -> void:
	_start_test("Package Loading")
	
	# Test loading a package
	var result = package_loader.load_package("res://test_models/TestPackage")
	_assert(result, "Package loaded successfully")
	
	# Test package metadata
	var metadata = package_loader.get_package_metadata("TestPackage")
	_assert(not metadata.is_empty(), "Package metadata retrieved")
	_assert(metadata.get("name", "") == "TestPackage", "Package name matches")
	
	# Test component loading
	var components = package_loader.get_package_components("TestPackage")
	_assert(not components.is_empty(), "Package components loaded")
	_assert(components.has("TestModel"), "Test model found in package")

func _test_msl_loading() -> void:
	_start_test("MSL Loading")
	
	# Test MSL path setting
	package_loader.set_msl_path("res://test_models/MSL")
	_assert(package_loader.has_msl(), "MSL path set")
	
	# Test MSL loading
	var result = package_loader.load_msl()
	_assert(result, "MSL loaded successfully")
	
	# Test MSL components
	var components = package_loader.get_package_components("Modelica")
	_assert(not components.is_empty(), "MSL components loaded")

func _test_component_loading() -> void:
	_start_test("Component Loading")
	
	# Load a test package
	package_loader.load_package("res://test_models/TestPackage")
	
	# Test component resolution
	var component = package_loader.resolve_type("TestModel", "TestPackage")
	_assert(not component.is_empty(), "Component resolved in package")
	_assert(component.get("name", "") == "TestModel", "Component name matches")
	
	# Test component from MSL
	package_loader.set_msl_path("res://test_models/MSL")
	package_loader.load_msl()
	var msl_component = package_loader.resolve_type("Resistor", "Modelica.Electrical.Analog.Basic")
	_assert(not msl_component.is_empty(), "MSL component resolved")

func _setup() -> void:
	print("Setting up test environment...")
	
	# Initialize components
	parser = MOParser.new()
	model_manager = ModelManager.new()
	
	# Add to scene tree
	root.add_child(model_manager)
	
	# Set up test paths
	test_msl_path = ProjectSettings.globalize_path("res://apps/modelica_godot/components")

func test_model_import() -> void:
	print("\nTesting model import functionality...")
	
	# Test package loading
	assert_true(package_loader.load_package(test_msl_path), "Package loaded successfully")
	
	# Test mechanical package
	assert_true(package_loader.has_package("Mechanical"), "Mechanical package exists")
	
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