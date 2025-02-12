extends Node

# Test results tracking
var total_tests := 0
var passed_tests := 0
var failed_tests := []

# Test components
var model_manager: ModelManager
var test_msl_path: String

func _ready():
	print("\nStarting Model Loading Tests...")
	call_deferred("start_tests")

func start_tests():
	await get_tree().create_timer(0.1).timeout  # Give time for scene setup
	await run_all_tests()
	print_results()
	
func run_all_tests():
	# Setup
	await setup_test_environment()
	await get_tree().create_timer(0.1).timeout  # Wait for setup to complete
	
	# Run tests
	await test_model_manager_initialization()
	await test_basic_package_loading()
	await test_blocks_interface_loading()
	await test_model_parameters()
	await test_model_equations()
	await test_model_components()
	await test_model_hierarchy()
	
	# Cleanup
	cleanup_test_environment()

func setup_test_environment():
	print("\nSetting up test environment...")
	model_manager = ModelManager.new()
	add_child(model_manager)
	await get_tree().create_timer(0.2).timeout  # Wait for initialization
	
	# Get the MSL path
	var project_root = ProjectSettings.globalize_path("res://")
	test_msl_path = project_root.path_join("apps/modelica_godot/MSL")
	
	# Verify MSL directory exists
	assert_true(DirAccess.dir_exists_absolute(test_msl_path), "MSL directory should exist")

func cleanup_test_environment():
	if model_manager:
		model_manager.queue_free()
		model_manager = null

# Test Cases
func test_model_manager_initialization():
	print("\nTest: Model Manager Initialization")
	assert_not_null(model_manager, "ModelManager should be created")
	await get_tree().create_timer(0.1).timeout  # Wait for initialization
	assert_not_null(model_manager.equation_system, "EquationSystem should be created")
	await get_tree().create_timer(0.1).timeout

func test_basic_package_loading():
	print("\nTest: Basic Package Loading")
	var done = false
	
	# Load main Modelica package
	var package_path = test_msl_path.path_join("Modelica/package.mo")
	assert_true(FileAccess.file_exists(package_path), "Package file should exist")
	
	# Connect to signal before loading
	if model_manager:
		model_manager.models_loaded.connect(func(): done = true, CONNECT_ONE_SHOT)
		model_manager.load_msl_directory(test_msl_path)
		await wait_for_loading(done)
		
		# Check if models were loaded
		var models = model_manager._models
		assert_true(models.size() > 0, "Should have loaded at least one model")
		assert_true(models.has(package_path), "Main Modelica package should be loaded")
		
		# Verify package content
		if models.has(package_path):
			var package_data = models[package_path]
			assert_true(package_data["type"] == "package", "Should be a package type")
			assert_true(package_data["name"] == "Modelica", "Should be named Modelica")

func test_blocks_interface_loading():
	print("\nTest: Blocks Interface Loading")
	var done = false
	
	# Load Blocks.Interfaces package
	var interfaces_path = test_msl_path.path_join("Modelica/Blocks/Interfaces.mo")
	assert_true(FileAccess.file_exists(interfaces_path), "Interfaces file should exist")
	
	if model_manager:
		model_manager.models_loaded.connect(func(): done = true, CONNECT_ONE_SHOT)
		model_manager.load_msl_directory(test_msl_path.path_join("Modelica/Blocks"))
		await wait_for_loading(done)
		
		# Check if interfaces were loaded
		var models = model_manager._models
		assert_true(models.has(interfaces_path), "Interfaces should be loaded")
		
		if models.has(interfaces_path):
			var interface_data = models[interfaces_path]
			print("Interface data:", interface_data)
			assert_true(interface_data.has("components"), "Should have components")
			assert_true(interface_data.has("parameters"), "Should have parameters")

func test_model_parameters():
	print("\nTest: Model Parameters")
	var done = false
	
	# Load a model with parameters (e.g., FirstOrder)
	var model_path = test_msl_path.path_join("Modelica/Blocks/Continuous.mo")
	assert_true(FileAccess.file_exists(model_path), "Model file should exist")
	
	if model_manager:
		model_manager.models_loaded.connect(func(): done = true, CONNECT_ONE_SHOT)
		model_manager.load_msl_directory(test_msl_path.path_join("Modelica/Blocks"))
		await wait_for_loading(done)
		
		# Check parameters
		var models = model_manager._models
		if models.has(model_path):
			var model_data = models[model_path]
			assert_true(model_data.has("parameters"), "Should have parameters array")
			print("Model parameters:", model_data["parameters"])

func test_model_equations():
	print("\nTest: Model Equations")
	var done = false
	
	# Load a model with equations
	var model_path = test_msl_path.path_join("Modelica/Blocks/Continuous.mo")
	assert_true(FileAccess.file_exists(model_path), "Model file should exist")
	
	if model_manager:
		model_manager.models_loaded.connect(func(): done = true, CONNECT_ONE_SHOT)
		model_manager.load_msl_directory(test_msl_path.path_join("Modelica/Blocks"))
		await wait_for_loading(done)
		
		# Check equations
		var models = model_manager._models
		if models.has(model_path):
			var model_data = models[model_path]
			assert_true(model_data.has("equations"), "Should have equations array")
			print("Model equations:", model_data["equations"])

func test_model_components():
	print("\nTest: Model Components")
	var done = false
	
	# Load a model with components
	var model_path = test_msl_path.path_join("Modelica/Blocks/Math.mo")
	assert_true(FileAccess.file_exists(model_path), "Model file should exist")
	
	if model_manager:
		model_manager.models_loaded.connect(func(): done = true, CONNECT_ONE_SHOT)
		model_manager.load_msl_directory(test_msl_path.path_join("Modelica/Blocks"))
		await wait_for_loading(done)
		
		# Check components
		var models = model_manager._models
		if models.has(model_path):
			var model_data = models[model_path]
			assert_true(model_data.has("components"), "Should have components array")
			print("Model components:", model_data["components"])

func test_model_hierarchy():
	print("\nTest: Model Hierarchy")
	var done = false
	
	if model_manager:
		model_manager.models_loaded.connect(func(): done = true, CONNECT_ONE_SHOT)
		model_manager.load_msl_directory(test_msl_path)
		await wait_for_loading(done)
		
		var model_tree = model_manager._model_tree
		
		# Check basic structure
		assert_true(model_tree.has("Modelica"), "Should have Modelica package")
		if model_tree.has("Modelica"):
			var modelica_pkg = model_tree["Modelica"]
			assert_true(modelica_pkg is Dictionary, "Modelica package should be a dictionary")
			assert_true(modelica_pkg.has("Blocks"), "Should have Blocks package")
			
			print("\nModel Tree Structure:")
			print_model_tree(model_tree)
			
			# Check specific packages
			var blocks = modelica_pkg["Blocks"]
			assert_true(blocks.has("Math"), "Should have Math package")
			assert_true(blocks.has("Continuous"), "Should have Continuous package")
			assert_true(blocks.has("Interfaces"), "Should have Interfaces package")

# Helper functions
func wait_for_loading(done_flag: bool) -> void:
	var timeout = 30.0  # 30 seconds timeout
	var time_waited = 0.0
	while not done_flag and time_waited < timeout:
		await get_tree().create_timer(0.1).timeout
		time_waited += 0.1
	assert_true(done_flag, "Loading should complete within timeout")

func print_model_tree(tree: Dictionary, indent: String = ""):
	for key in tree.keys():
		print(indent + key)
		if tree[key] is Dictionary:
			print_model_tree(tree[key], indent + "  ")

# Assertion helpers
func assert_true(condition: bool, message: String) -> void:
	total_tests += 1
	if condition:
		passed_tests += 1
		print("✓ " + message)
	else:
		failed_tests.append(message)
		print("✗ " + message)

func assert_not_null(value: Variant, message: String) -> void:
	assert_true(value != null, message)

# Results reporting
func print_results():
	print("\nTest Results:")
	print("-------------")
	print("Total Tests: ", total_tests)
	print("Passed: ", passed_tests)
	print("Failed: ", total_tests - passed_tests)
	
	if failed_tests.size() > 0:
		print("\nFailed Tests:")
		for failure in failed_tests:
			print("- " + failure) 