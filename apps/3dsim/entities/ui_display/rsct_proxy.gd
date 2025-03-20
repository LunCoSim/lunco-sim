extends Node

# This script acts as a proxy for the actual RSCT scene
# It redirects method calls and signals to the real scene which is inside the SubViewport

# Reference to the actual RSCT scene
var actual_rsct = null
var pending_methods = []

# Properties to proxy directly
var Utils = null
var Web3Interface = null
var simulation = null

# Signal to use when the proxy is ready
signal proxy_ready

func _ready():
	print("RSCT Proxy initializing...")
	
	# Keep trying to find the actual RSCT until we find it
	find_actual_rsct()
	
	# If we don't find it immediately, start a timer to keep trying
	if actual_rsct == null:
		var timer = Timer.new()
		timer.wait_time = 0.1
		timer.one_shot = false
		timer.timeout.connect(find_actual_rsct)
		add_child(timer)
		timer.start()

func find_actual_rsct():
	# Find the actual RSCT scene in the SubViewport
	var supply_chain_displays = get_tree().get_nodes_in_group("supply_chain_display")
	if supply_chain_displays.size() > 0:
		var display = supply_chain_displays[0]
		if display.has_node("SubViewport/RSCT"):
			actual_rsct = display.get_node("SubViewport/RSCT")
			print("RSCT proxy connected to actual scene")
			
			# Set up direct property references
			if actual_rsct.has_node("Simulation"):
				simulation = actual_rsct.get_node("Simulation")
			
			Utils = actual_rsct.Utils
			Web3Interface = actual_rsct.Web3Interface
			
			emit_signal("proxy_ready")
			
			# Process any pending methods
			for method_call in pending_methods:
				if actual_rsct.has_method(method_call.method):
					actual_rsct.callv(method_call.method, method_call.args)
			pending_methods.clear()
			
			# Remove timer if it exists
			for child in get_children():
				if child is Timer:
					child.queue_free()
		else:
			print("Looking for RSCT scene, not found yet...")
	else:
		print("No supply_chain_display nodes found yet, retrying...")

# Override _get to forward property access to the actual scene
func _get(property):
	# Check if we have a local reference first
	if property == "Utils" and Utils != null:
		return Utils
	elif property == "Web3Interface" and Web3Interface != null:
		return Web3Interface
	elif property == "simulation" and simulation != null:
		return simulation
	
	# Otherwise forward to the actual scene
	if actual_rsct:
		return actual_rsct.get(property)
	return null

# Override _set to forward property setting to the actual scene
func _set(property, value):
	if actual_rsct:
		return actual_rsct.set(property, value)
	return false

# Catch-all method to forward any method calls to the actual scene
func _call_method(method, args):
	if actual_rsct and actual_rsct.has_method(method):
		return actual_rsct.callv(method, args)
	else:
		# Store the method call for later when actual_rsct is available
		pending_methods.append({"method": method, "args": args})
		print("Method '" + method + "' queued for later execution")
	return null

# === Proxied methods for wallet_connect_button.gd and other scripts ===

# Web3 interface methods
func connect_wallet():
	return _call_method("connect_wallet", [])

func get_wallet_interface():
	return Web3Interface

# Simulation access
func get_simulation():
	return simulation

# Graph access
func get_graph():
	return _call_method("get_graph", [])

# Node management methods needed by the facilities menu
func add_node_from_path(path, position = Vector2.ZERO):
	return _call_method("add_node_from_path", [path, position])

# For class map access
func initialize_class_map(path):
	if Utils:
		return Utils.initialize_class_map(path)
	return null 