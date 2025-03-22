class_name LCUiDisplayManager
extends Node

# References to the display nodes (will be set by the display controller)
var supply_chain_display = null
var modelica_display = null

# Track which display is currently active
var active_display = "none"  # "none", "supply_chain", or "modelica"

# Signals
signal display_activated(display_name)
signal display_deactivated(display_name)

# Initialize the manager
func _ready():
	pass

# Method to set the display references
func set_displays(supply_chain: Node, modelica: Node):
	supply_chain_display = supply_chain
	modelica_display = modelica

# Process key events for toggling displays
func process_key_event(event: InputEvent) -> bool:
	# Return true if we handled the event, false otherwise
	if not event is InputEventKey:
		return false
		
	if event.pressed and event.keycode == KEY_TAB:
		toggle_supply_chain_display()
		return true
		
	if event.pressed and event.keycode == KEY_M:
		toggle_modelica_display()
		return true
		
	# Pass keyboard events to active display when active
	if active_display != "none" and event is InputEventKey:
		return pass_keyboard_input_to_active_display(event)
		
	return false

# Process mouse events for displays
func process_mouse_event(event: InputEvent) -> bool:
	# Return true if we handled the event, false otherwise
	if active_display == "none":
		return false
	
	if active_display == "supply_chain" and supply_chain_display:
		return pass_mouse_input_to_supply_chain(event)
	elif active_display == "modelica" and modelica_display:
		return pass_mouse_input_to_modelica(event)
		
	return false

# Toggle supply chain display
func toggle_supply_chain_display():
	if active_display == "supply_chain":
		# Hide display
		if supply_chain_display:
			supply_chain_display.toggle_display()
		active_display = "none"
		emit_signal("display_deactivated", "supply_chain")
	else:
		# First hide any active display
		if active_display == "modelica" and modelica_display:
			modelica_display.toggle_display()
			emit_signal("display_deactivated", "modelica")
		
		# Then show supply chain display
		if supply_chain_display:
			supply_chain_display.toggle_display()
		active_display = "supply_chain"
		emit_signal("display_activated", "supply_chain")

# Toggle modelica display
func toggle_modelica_display():
	if active_display == "modelica":
		# Hide display
		if modelica_display:
			modelica_display.toggle_display()
		active_display = "none"
		emit_signal("display_deactivated", "modelica")
	else:
		# First hide any active display
		if active_display == "supply_chain" and supply_chain_display:
			supply_chain_display.toggle_display()
			emit_signal("display_deactivated", "supply_chain")
		
		# Then show modelica display
		if modelica_display:
			modelica_display.toggle_display()
		active_display = "modelica"
		emit_signal("display_activated", "modelica")

# Handle passing keyboard input to the active display
func pass_keyboard_input_to_active_display(event: InputEvent) -> bool:
	if active_display == "supply_chain" and supply_chain_display:
		return supply_chain_display.receive_keyboard_input(event)
	elif active_display == "modelica" and modelica_display:
		return modelica_display.receive_keyboard_input(event)
	return false

# Handle passing mouse input to the supply chain display
func pass_mouse_input_to_supply_chain(event: InputEvent) -> bool:
	if supply_chain_display:
		return supply_chain_display.receive_mouse_input(event)
	return false

# Handle passing mouse input to the modelica display
func pass_mouse_input_to_modelica(event: InputEvent) -> bool:
	if modelica_display:
		return modelica_display.receive_mouse_input(event)
	return false

# Check if a display is active
func is_display_active() -> bool:
	return active_display != "none"

# Get the name of the active display
func get_active_display() -> String:
	return active_display 