extends Control

# Path to the Modelica core scripts
const PackageManager = preload("res://apps/modelica/core/package_manager.gd")
const Parser = preload("res://apps/modelica/core/parser.gd")
const ErrorSystem = preload("res://apps/modelica/core/error_system.gd")
const ASTNode = preload("res://apps/modelica/core/ast_node.gd")

# Custom components
const ModelicaSyntaxHighlighter = preload("res://apps/modelica-ui/scripts/ui/modelica_syntax_highlighter.gd")
const ModelicaSimulator = preload("res://apps/modelica-ui/scripts/core/modelica_simulator.gd")

# UI elements
@onready var file_tree = $MainLayout/FilePanel/FileTree
@onready var code_editor = $MainLayout/WorkArea/EditorPanel/CodeEditor
@onready var file_name_label = $MainLayout/WorkArea/EditorPanel/EditorToolbar/FileNameLabel
@onready var results_table = $MainLayout/WorkArea/SimulationPanel/ResultsTabContainer/Table/ResultsTable
@onready var load_file_dialog = $LoadFileDialog
@onready var save_file_dialog = $SaveFileDialog
@onready var new_file_dialog = $NewFileDialog
@onready var export_csv_dialog = $ExportCSVDialog

# Simulation parameters
@onready var start_time_input = $MainLayout/WorkArea/SimulationPanel/SimulationToolbar/StartTimeInput
@onready var end_time_input = $MainLayout/WorkArea/SimulationPanel/SimulationToolbar/EndTimeInput
@onready var step_size_input = $MainLayout/WorkArea/SimulationPanel/SimulationToolbar/StepSizeInput

# Core components
var package_manager = null
var parser = null
var simulator = null
var syntax_highlighter = null
var error_manager = null

# State tracking
var current_file_path = ""
var loaded_files = []
var simulation_results = []
var file_tree_root = null
var current_ast = null
var edit_timer = null
var parsing_in_progress = false

func _ready():
	# Initialize core components
	package_manager = PackageManager.new()
	parser = Parser.new()
	simulator = ModelicaSimulator.new()
	syntax_highlighter = ModelicaSyntaxHighlighter.new()
	error_manager = ErrorSystem.create_error_manager()
	
	# Set up edit timer for delayed parsing
	edit_timer = Timer.new()
	edit_timer.one_shot = true
	edit_timer.wait_time = 0.5  # 500ms delay for parsing after typing stops
	edit_timer.timeout.connect(_on_edit_timer_timeout)
	add_child(edit_timer)
	
	# Connect simulator signals
	simulator.simulation_complete.connect(_on_simulation_complete)
	simulator.simulation_error.connect(_on_simulation_error)
	
	# Set up the code editor
	code_editor.syntax_highlighter = syntax_highlighter
	code_editor.text_changed.connect(_on_code_editor_text_changed)
	
	# Add default model paths
	package_manager.add_modelica_path("res://apps/modelica/models")
	
	# Connect UI signals
	$MainLayout/FilePanel/FilesPanelHeader/LoadFileButton.pressed.connect(_on_load_button_pressed)
	$MainLayout/FilePanel/FilesPanelHeader/NewFileButton.pressed.connect(_on_new_button_pressed)
	$MainLayout/WorkArea/EditorPanel/EditorToolbar/SaveButton.pressed.connect(_on_save_button_pressed)
	$MainLayout/WorkArea/EditorPanel/EditorToolbar/RunButton.pressed.connect(_on_run_button_pressed)
	$MainLayout/WorkArea/SimulationPanel/SimulationToolbar/ExportCSVButton.pressed.connect(_on_export_csv_button_pressed)
	
	# Connect file dialogs
	load_file_dialog.file_selected.connect(_on_file_selected)
	save_file_dialog.file_selected.connect(_on_save_file_selected)
	new_file_dialog.file_selected.connect(_on_new_file_selected)
	export_csv_dialog.file_selected.connect(_on_export_csv_file_selected)
	
	# Initialize the file tree
	_setup_file_tree()
	
	# Setup the initial UI state
	_update_ui_state()

# UI signal handlers
func _on_load_button_pressed():
	load_file_dialog.popup_centered()

func _on_new_button_pressed():
	new_file_dialog.popup_centered()

func _on_save_button_pressed():
	if current_file_path.is_empty():
		save_file_dialog.popup_centered()
	else:
		_save_current_file(current_file_path)

func _on_run_button_pressed():
	if current_file_path.is_empty():
		print("No file loaded to simulate")
		return
	
	_run_simulation()

func _on_export_csv_button_pressed():
	if simulation_results.is_empty():
		print("No simulation results to export")
		return
	
	export_csv_dialog.popup_centered()

# Editor signal handlers
func _on_code_editor_text_changed():
	# Reset the timer each time the text changes
	edit_timer.stop()
	edit_timer.start()

func _on_edit_timer_timeout():
	# Parse the current text after the edit timer expires
	if not current_file_path.is_empty():
		_parse_current_text()

# Simulator signal handlers
func _on_simulation_complete(results):
	simulation_results = results
	_display_results()
	print("Simulation completed with %d time steps" % results.size())

func _on_simulation_error(message):
	print("Simulation error: ", message)
	# TODO: Add a proper error dialog

# File dialog handlers
func _on_file_selected(path):
	_load_modelica_file(path)

func _on_save_file_selected(path):
	_save_current_file(path)

func _on_new_file_selected(path):
	_create_new_file(path)

func _on_export_csv_file_selected(path):
	simulator.export_to_csv(simulation_results, path)

# File operations
func _load_modelica_file(path):
	var file_content = ""
	
	var file = FileAccess.open(path, FileAccess.READ)
	if file:
		file_content = file.get_as_text()
		file.close()
		
		# Update the editor and state
		code_editor.text = file_content
		current_file_path = path
		file_name_label.text = path.get_file()
		
		# Try to parse the file
		_parse_modelica_file(path)
		
		# Try to load dependencies
		var deps_result = package_manager.validate_and_load_model(path)
		if deps_result.success:
			print("Model loaded successfully with %d dependencies" % deps_result.dependencies.size())
			
			# Add this file and its dependencies to our loaded files list
			if not path in loaded_files:
				loaded_files.append(path)
			
			for dep in deps_result.dependencies:
				if not dep in loaded_files:
					loaded_files.append(dep)
		else:
			print("Failed to load dependencies: ", deps_result.errors)
		
		# Update the file tree
		_update_file_tree()
		
		# Update UI state
		_update_ui_state()
	else:
		print("Failed to open file: ", path)

func _save_current_file(path):
	var file = FileAccess.open(path, FileAccess.WRITE)
	if file:
		file.store_string(code_editor.text)
		file.close()
		
		print("File saved: ", path)
		current_file_path = path
		file_name_label.text = path.get_file()
		
		# Update the file tree if necessary
		if not path in loaded_files:
			loaded_files.append(path)
			_update_file_tree()
		
		# Re-parse the file
		_parse_modelica_file(path)
		
		# Update UI state
		_update_ui_state()
	else:
		print("Failed to save file: ", path)

func _create_new_file(path):
	var file = FileAccess.open(path, FileAccess.WRITE)
	if file:
		# Determine the model name from the filename
		var filename = path.get_file().get_basename()
		
		# Create a better template with proper model name
		var template = "model %s\n\t// Your model code here\nend %s;" % [filename, filename]
		file.store_string(template)
		file.close()
		
		# Load the new file
		_load_modelica_file(path)
	else:
		print("Failed to create new file: ", path)

# Parsing
func _parse_modelica_file(path):
	if parsing_in_progress:
		return
		
	parsing_in_progress = true
	current_ast = parser.parse_file(path)
	
	if current_ast == null:
		print("Warning: Failed to parse model file. Some features may not work correctly.")
		var errors = parser.get_errors()
		if errors and errors.size() > 0:
			_show_parse_errors(errors)
	else:
		# Clear any error markers
		_clear_parse_errors()
		
		# If successful, analyze the model structure
		_analyze_model_structure(current_ast)
	
	parsing_in_progress = false

func _parse_current_text():
	if parsing_in_progress or current_file_path.is_empty():
		return
		
	# Create a temporary file
	var temp_file_path = "user://temp_modelica.mo"
	var file = FileAccess.open(temp_file_path, FileAccess.WRITE)
	if file:
		file.store_string(code_editor.text)
		file.close()
		
		# Parse the temporary file
		parsing_in_progress = true
		var ast = parser.parse_file(temp_file_path)
		
		if ast == null:
			var errors = parser.get_errors()
			if errors and errors.size() > 0:
				_show_parse_errors(errors)
		else:
			# Clear any error markers
			_clear_parse_errors()
			current_ast = ast
			
			# If successful, analyze the model structure
			_analyze_model_structure(current_ast)
		
		# Delete temporary file
		var dir = DirAccess.open("user://")
		if dir:
			dir.remove(temp_file_path)
		
		parsing_in_progress = false

func _show_parse_errors(errors):
	for error in errors:
		print("Parse error: " + error.message + " at line " + str(error.line))
		
		# Add visual indicator in the editor for the error
		if error.has("line") and error.line >= 1:
			code_editor.set_line_background_color(error.line - 1, Color(0.7, 0.2, 0.2, 0.3))
			code_editor.set_line_gutter_icon(error.line - 1, 0, get_theme_icon("Error", "EditorIcons"))

func _clear_parse_errors():
	# Clear all error markers
	for i in range(code_editor.get_line_count()):
		code_editor.set_line_background_color(i, Color(0, 0, 0, 0))
		code_editor.set_line_gutter_icon(i, 0, null)

func _analyze_model_structure(ast):
	# Analyze the model structure for enhanced features
	# This could include extracting variables, equations, etc.
	if ast == null:
		return
		
	print("Model analysis: " + ast.qualified_name)
	
	# Extract model information that could be useful for the UI
	# For example: variables, parameters, equations, etc.

# Simulation
func _run_simulation():
	if current_file_path.is_empty():
		return
	
	print("Running simulation for: ", current_file_path)
	
	# Make sure we save any unsaved changes first
	_save_current_file(current_file_path)
	
	# If we don't have a parsed AST, try to parse the file again
	if current_ast == null:
		_parse_modelica_file(current_file_path)
		if current_ast == null:
			print("Failed to parse model file")
			return
	
	# Set up the simulation parameters
	var start_time = start_time_input.value
	var end_time = end_time_input.value
	var step_size = step_size_input.value
	
	# Set up the model
	var setup_result = simulator.setup_model(current_ast, start_time, end_time, step_size)
	if not setup_result.success:
		print("Failed to set up simulation: ", setup_result.error)
		return
	
	# Run the simulation
	simulator.run_simulation(setup_result)
	
	# Update UI state
	_update_ui_state()

# Results handling
func _display_results():
	# Clear the current table
	results_table.clear()
	
	if simulation_results.is_empty():
		return
	
	# Get variable names
	var variables = simulator.get_result_variables(simulation_results)
	
	# Set up the columns
	results_table.set_column_title(0, "Time")
	var col_index = 1
	for var_name in variables:
		if col_index < results_table.columns:
			results_table.set_column_title(col_index, var_name)
		else:
			# We need to add more columns
			results_table.columns = col_index + 1
			results_table.set_column_title(col_index, var_name)
		col_index += 1
	
	# Add the data rows
	var root = results_table.create_item()
	for result in simulation_results:
		var item = results_table.create_item(root)
		item.set_text(0, str(result.time))
		col_index = 1
		for var_name in variables:
			item.set_text(col_index, str(result[var_name]))
			col_index += 1
	
	# Update UI state
	_update_ui_state()

# File tree
func _setup_file_tree():
	file_tree.clear()
	file_tree_root = file_tree.create_item()
	file_tree_root.set_text(0, "Loaded Files")
	
	file_tree.item_selected.connect(_on_file_tree_item_selected)

func _update_file_tree():
	# Clear existing items except root
	var children = file_tree_root.get_children()
	while children:
		children.free()
		children = file_tree_root.get_children()
	
	# Add loaded files
	for file_path in loaded_files:
		var item = file_tree.create_item(file_tree_root)
		item.set_text(0, file_path.get_file())
		item.set_metadata(0, file_path)

func _on_file_tree_item_selected():
	var selected = file_tree.get_selected()
	if selected and selected != file_tree_root:
		var path = selected.get_metadata(0)
		if path != current_file_path:
			_load_modelica_file(path)

# UI state management
func _update_ui_state():
	# Enable/disable buttons based on current state
	$MainLayout/WorkArea/EditorPanel/EditorToolbar/SaveButton.disabled = current_file_path.is_empty()
	$MainLayout/WorkArea/EditorPanel/EditorToolbar/RunButton.disabled = current_file_path.is_empty()
	$MainLayout/WorkArea/SimulationPanel/SimulationToolbar/ExportCSVButton.disabled = simulation_results.is_empty() 
