extends Node

# This script acts as a proxy for the actual ModelicaUI scene
# It redirects method calls and signals to the real scene which is inside the SubViewport

# Reference to the actual ModelicaUI scene
var actual_modelica_ui = null
var pending_methods = []

# Signal to use when the proxy is ready
signal proxy_ready

func _ready():
	print("ModelicaUI Proxy initializing...")
	
	# Keep trying to find the actual ModelicaUI until we find it
	find_actual_modelica_ui()
	
	# If we don't find it immediately, start a timer to keep trying
	if actual_modelica_ui == null:
		var timer = Timer.new()
		timer.wait_time = 0.1
		timer.one_shot = false
		timer.timeout.connect(find_actual_modelica_ui)
		add_child(timer)
		timer.start()

func find_actual_modelica_ui():
	# Find the actual ModelicaUI scene in the SubViewport
	var modelica_displays = get_tree().get_nodes_in_group("modelica_display")
	if modelica_displays.size() > 0:
		var display = modelica_displays[0]
		if display.has_node("SubViewport/ModelicaUI"):
			actual_modelica_ui = display.get_node("SubViewport/ModelicaUI")
			print("ModelicaUI proxy connected to actual scene")
			
			emit_signal("proxy_ready")
			
			# Process any pending methods
			for method_call in pending_methods:
				if actual_modelica_ui.has_method(method_call.method):
					actual_modelica_ui.callv(method_call.method, method_call.args)
			pending_methods.clear()
			
			# Remove timer if it exists
			for child in get_children():
				if child is Timer:
					child.queue_free()
		else:
			print("Looking for ModelicaUI scene, not found yet...")
	else:
		print("No modelica_display nodes found yet, retrying...")

# Override _get to forward property access to the actual scene
func _get(property):
	if actual_modelica_ui:
		return actual_modelica_ui.get(property)
	return null

# Override _set to forward property setting to the actual scene
func _set(property, value):
	if actual_modelica_ui:
		return actual_modelica_ui.set(property, value)
	return false

# Catch-all method to forward any method calls to the actual scene
func _call_method(method, args):
	if actual_modelica_ui and actual_modelica_ui.has_method(method):
		return actual_modelica_ui.callv(method, args)
	else:
		# Store the method call for later when actual_modelica_ui is available
		pending_methods.append({"method": method, "args": args})
		print("Method '" + method + "' queued for later execution")
	return null

# === Proxied methods ===

# Get the current modelica file being edited
func get_current_file():
	return _call_method("get_current_file", [])

# Run a simulation with the current file
func run_simulation(start_time = 0.0, end_time = 10.0, step_size = 0.01):
	return _call_method("run_simulation", [start_time, end_time, step_size])

# Export simulation results to CSV
func export_to_csv(file_path):
	return _call_method("export_to_csv", [file_path])

# Load a Modelica file
func load_file(file_path):
	return _call_method("load_file", [file_path])

# Save the current file
func save_file(file_path = ""):
	return _call_method("save_file", [file_path]) 