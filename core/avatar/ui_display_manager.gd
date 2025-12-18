class_name LCUiDisplayManager
extends Node

# References to the display nodes (will be set by the display controller)
var supply_chain_display = null
var modelica_display = null

# Track which display is currently active
var active_display = "none"  # "none", "supply_chain", "modelica", or "console"

# Signals
signal display_activated(display_name)
signal display_deactivated(display_name)

# Initialize the manager
func _ready():
	add_to_group("ui_display_manager")
	print("UiDisplayManager: Initialized")

# Check if any display is currently capturing input
func is_input_captured() -> bool:
	# Check if modelica display has keyboard focus
	if active_display == "modelica" and modelica_display:
		if "has_keyboard_focus" in modelica_display:
			var is_captured = modelica_display.has_keyboard_focus
			print("UiDisplayManager: Modelica has_keyboard_focus = ", is_captured)
			return is_captured
	
	# Check if supply chain display has keyboard focus
	if active_display == "supply_chain" and supply_chain_display:
		if "has_keyboard_focus" in supply_chain_display:
			var is_captured = supply_chain_display.has_keyboard_focus
			print("UiDisplayManager: Supply chain has_keyboard_focus = ", is_captured)
			return is_captured
	
	# Fallback: if a display is active, assume it's capturing input
	var fallback = active_display != "none"
	if fallback:
		# If console is active, it definitely captured input
		if active_display == "console":
			return true
		print("UiDisplayManager: Fallback - active_display = ", active_display)
	return fallback

# Method to set the display references
func set_displays(supply_chain: Node, modelica: Node):
	print("UiDisplayManager: Setting displays - Supply Chain: ", supply_chain, ", ModelicaUI: ", modelica)
	
	supply_chain_display = supply_chain
	modelica_display = modelica
	
	# Check each display and warn if null
	if supply_chain_display == null:
		push_warning("UiDisplayManager: Supply chain display reference is null")
	if modelica_display == null:
		push_warning("UiDisplayManager: ModelicaUI display reference is null")
		
	# Try to find the displays directly if they weren't provided
	if modelica_display == null:
		var displays = get_tree().get_nodes_in_group("modelica_display")
		if displays.size() > 0:
			modelica_display = displays[0]
			print("UiDisplayManager: Found ModelicaUI display directly: ", modelica_display)
	
	if supply_chain_display == null:
		var displays = get_tree().get_nodes_in_group("supply_chain_display")
		if displays.size() > 0:
			supply_chain_display = displays[0]
			print("UiDisplayManager: Found Supply Chain display directly: ", supply_chain_display)
			
	# Final status report
	print("UiDisplayManager: Displays registered - Supply Chain: ", supply_chain_display != null, 
		", ModelicaUI: ", modelica_display != null)
		
	# If we found ModelicaUI, ensure it's configured correctly
	if modelica_display != null:
		print("UiDisplayManager: Ensuring ModelicaUI is properly set up")
		# Make sure it's visible
		modelica_display.visible = true
		modelica_display.is_display_visible = true
		modelica_display.input_enabled = true

# Process key events for toggling displays and forwarding input
func process_key_event(event: InputEvent) -> bool:
	# Return true if we handled the event, false otherwise
	if not event is InputEventKey:
		return false
	
	# Only log important keys for performance reasons
	if event.pressed and not event.is_echo() and event.keycode in [KEY_ESCAPE, KEY_TAB, KEY_ENTER]:
		print("UiDisplayManager: Processing key event - Key: ", event.keycode)
		
	# Handle Escape key to close active display
	if event.pressed and event.keycode == KEY_ESCAPE:
		if active_display == "modelica":
			close_modelica_display()
			return true
		elif active_display == "supply_chain":
			toggle_supply_chain_display()
			return true
	
	# Handle TAB for supply chain display
	if event.pressed and event.keycode == KEY_TAB:
		toggle_supply_chain_display()
		return true
		
	# Pass keyboard events to active display when active
	if active_display != "none" and event is InputEventKey:
		# If console is active, capture EVERYTHING to prevent leaks to avatar/sim
		if active_display == "console":
			# Still let the console process it if it needs to, but we definitely return true
			return true
			
		var result = pass_keyboard_input_to_active_display(event)
		return result
		
	return false

# Process mouse events for displays
func process_mouse_event(event: InputEvent) -> bool:
	# Return true if we handled the event, false otherwise
	if active_display == "none":
		# Check if we clicked on any inactive displays
		if event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT:
			# We'll let the 3D area input handling take care of this
			return false
		return false
	
	if active_display == "supply_chain" and supply_chain_display:
		return pass_mouse_input_to_supply_chain(event)
	elif active_display == "modelica" and modelica_display:
		var handled = pass_mouse_input_to_modelica(event)
		
		# If the event wasn't handled by the Modelica display (i.e. clicked outside),
		# and it's a left mouse click, close the display
		if not handled and event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT:
			print("UiDisplayManager: Clicked outside Modelica display, closing")
			close_modelica_display()
			return true
			
		return handled
		
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
		print("Supply Chain Display activated")

# Close modelica display - only closes, doesn't toggle
func close_modelica_display():
	if active_display == "modelica" and modelica_display:
		# Use the dedicated release_focus function if available
		if modelica_display.has_method("release_focus"):
			modelica_display.call("release_focus")
		else:
			# Fallback: manually release focus from any focused controls
			if is_instance_valid(modelica_display.modelica_scene):
				var focused_control = null
				if modelica_display.has_method("_get_focused_control"):
					focused_control = modelica_display.call("_get_focused_control", modelica_display.modelica_scene)
				
				if focused_control and is_instance_valid(focused_control) and focused_control.has_focus():
					focused_control.release_focus()
					print("UiDisplayManager: Released focus from control in ModelicaUI")
		
		# Just release keyboard focus but keep the display visible
		modelica_display.has_keyboard_focus = false
		
		# Instead of toggling the display off completely, we just deactivate it for input purposes
		active_display = "none"
		emit_signal("display_deactivated", "modelica")
		print("Modelica Display deactivated (but still visible)")
		
		# Ensure that the UI viewport loses focus to restore normal avatar controls
		var viewport = get_viewport()
		if viewport:
			viewport.gui_release_focus()
			print("UiDisplayManager: Released focus from all UI elements in viewport")

# Toggle modelica display - kept for backward compatibility
func toggle_modelica_display():
	if active_display == "modelica":
		close_modelica_display()
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
		print("Modelica Display activated")

# Handle passing keyboard input to the active display
func pass_keyboard_input_to_active_display(event: InputEvent) -> bool:
	# Only log important actions
	if event.pressed and not event.is_echo() and event.keycode in [KEY_ESCAPE, KEY_TAB, KEY_ENTER]:
		print("UiDisplayManager: Pass key to " + active_display + ": " + str(event.keycode))
	
	if active_display == "supply_chain" and supply_chain_display:
		return supply_chain_display.receive_keyboard_input(event)
	elif active_display == "modelica" and modelica_display:
		# Always force ensure the display is visible and active
		if modelica_display.visible == false:
			modelica_display.visible = true
			modelica_display.is_display_visible = true
			modelica_display.input_enabled = true
			print("UiDisplayManager: Forced ModelicaUI to be visible")
		
		# Ensure the display has keyboard focus
		modelica_display.has_keyboard_focus = true
		
		# Special case for TAB, ENTER and directional keys which are important for UI navigation
		if event is InputEventKey and event.pressed:
			if event.keycode in [KEY_TAB, KEY_ENTER, KEY_UP, KEY_DOWN, KEY_LEFT, KEY_RIGHT]:
				# If this is a key important for UI navigation, make sure focus is set before sending
				if modelica_display.has_method("_direct_set_focus"):
					modelica_display.call("_direct_set_focus")
		
		# Forward the event
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

# Method to handle when ModelicaUI is clicked directly
func on_modelica_display_clicked():
	print("UiDisplayManager: ModelicaUI clicked, active_display was: ", active_display)
	
	# Always set modelica as active display when clicked
	active_display = "modelica"
	
	# First hide any other active display if needed
	if supply_chain_display and active_display == "supply_chain":
		supply_chain_display.toggle_display()
		emit_signal("display_deactivated", "supply_chain")
	
	emit_signal("display_activated", "modelica")
	print("UiDisplayManager: Modelica Display activated from direct click")
	
	# Ensure modelica display is visible and has focus
	if modelica_display:
		if not modelica_display.visible:
			modelica_display.visible = true
			modelica_display.is_display_visible = true
			modelica_display.input_enabled = true
			print("UiDisplayManager: Made ModelicaUI visible")
		
		# Ensure proper focus using multiple approaches
		modelica_display.has_keyboard_focus = true
		
		# Send space key event to trigger focus
		var dummy_event = InputEventKey.new()
		dummy_event.pressed = true
		dummy_event.keycode = KEY_SPACE
		dummy_event.unicode = 32  # Space character
		modelica_display.receive_keyboard_input(dummy_event)
		
		# Send key release event
		dummy_event.pressed = false
		modelica_display.receive_keyboard_input(dummy_event)
		
		print("UiDisplayManager: Sent focus triggers to ModelicaUI")
		
		# Try direct mouse click too
		var click_event = InputEventMouseButton.new()
		click_event.button_index = MOUSE_BUTTON_LEFT
		click_event.pressed = true
		click_event.position = Vector2(800, 450)  # Center of the screen
		modelica_display.receive_mouse_input(click_event)
		
		# And mouse button release
		click_event.pressed = false
		modelica_display.receive_mouse_input(click_event)
		
		print("UiDisplayManager: Sent mouse events to ModelicaUI") 
