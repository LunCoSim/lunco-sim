extends Node3D

# Path to the modelica UI scene
const MODELICA_SCENE_PATH = "res://apps/modelica-ui/scenes/modelica_main.tscn"

# State variables
var modelica_scene = null
var input_enabled = true
var last_click_position = Vector2()
var is_dragging = false
var mouse_button_pressed = false
var mouse_over_display = false
var is_display_visible = true
var has_keyboard_focus = false
var mesh_size = Vector2(50, 38)  # Will be updated from actual mesh in _ready()

# Helper function to get the actual mesh size
func _get_mesh_size() -> Vector2:
	if $DisplayMesh and $DisplayMesh.mesh:
		var mesh = $DisplayMesh.mesh
		if mesh is QuadMesh:
			return mesh.size
		elif mesh is PlaneMesh:
			return mesh.size
	# Fallback to default size if mesh not found
	return Vector2(50, 38)

func _ready():
	# Add to group for easy identification
	add_to_group("modelica_display")
	
	# Get the actual mesh size from the DisplayMesh
	mesh_size = _get_mesh_size()
	print("ModelicaUI: Mesh size detected as: ", mesh_size)
	
	# First, add a reference to the scene at root level for scripts that use absolute paths
	# This needs to happen BEFORE loading the modelica scene so other nodes can find it
	if not get_node_or_null("/root/ModelicaUI"):
		var root_ref = Node.new()
		root_ref.name = "ModelicaUI"
		root_ref.set_script(load("res://apps/3dsim/entities/ui_display/modelica_proxy.gd"))
		call_deferred("_add_root_reference", root_ref)
	
	# Load the modelica scene into the SubViewport (deferred)
	modelica_scene = load(MODELICA_SCENE_PATH).instantiate()
	modelica_scene.name = "ModelicaUI"  # Set the name to match what other scripts are looking for
	call_deferred("_add_modelica_scene", modelica_scene)
		
	# Set up mesh to display the viewport texture
	var viewport_texture = $SubViewport.get_texture()
	var material = $DisplayMesh.get_surface_override_material(0)
	material.albedo_texture = viewport_texture
	
	# Set up collision shape to match mesh for interaction
	var collision_shape = $Area3D/CollisionShape3D
	var box_shape = BoxShape3D.new()
	box_shape.size = Vector3(mesh_size.x, mesh_size.y, 0.1)
	collision_shape.shape = box_shape
	print("ModelicaUI: Collision shape updated to size: ", box_shape.size)
	
	# Make sure the Area3D is on the correct collision layer (2)
	$Area3D.collision_layer = 2
	$Area3D.collision_mask = 0  # We don't need the area to detect collisions
	
	# Make sure the SubViewport receives input events
	$SubViewport.handle_input_locally = true
	$SubViewport.gui_disable_input = false
	
	# Connect to global mouse events for better drag handling
	get_viewport().connect("gui_focus_changed", _on_focus_changed)
	
	# Connect to mouse enter/exit events
	$Area3D.mouse_entered.connect(_on_mouse_entered)
	$Area3D.mouse_exited.connect(_on_mouse_exited)

func _process(_delta):
	# Continuously send mouse motion events when dragging is active
	if is_dragging and mouse_button_pressed and input_enabled:
		# Get the mouse position in viewport space
		var mouse_pos = get_viewport().get_mouse_position()
		var camera = get_viewport().get_camera_3d()
		
		# Cast a ray from the camera to the mouse position
		var from = camera.project_ray_origin(mouse_pos)
		var to = from + camera.project_ray_normal(mouse_pos) * 1000
		
		var space_state = get_world_3d().direct_space_state
		var query = PhysicsRayQueryParameters3D.create(from, to)
		query.collide_with_areas = true
		query.collision_mask = 2  # Match the Area3D collision layer
		
		var result = space_state.intersect_ray(query)
		if result and result.collider == $Area3D:
			# Convert 3D position to 2D viewport coordinates
			_handle_mouse_motion(result.position)
		else:
			# If ray doesn't hit our display but we're still dragging,
			# use the last position with updated relative movement
			var viewport_event = InputEventMouseMotion.new()
			viewport_event.position = last_click_position
			viewport_event.global_position = last_click_position
			viewport_event.relative = mouse_pos - get_viewport().get_mouse_position()
			viewport_event.button_mask = MOUSE_BUTTON_MASK_LEFT
			$SubViewport.push_input(viewport_event)

	# Debug direct key detection is no longer needed since we're handling keys properly through receive_keyboard_input
	# This dramatically reduces console spam

func _on_focus_changed(control):
	# This is called when focus changes in the GUI
	# If a control in our SubViewport gained focus, we should capture keyboard events
	if control and is_instance_valid(control) and control.is_inside_tree():
		var parent_viewport = control.get_viewport()
		if parent_viewport == $SubViewport:
			has_keyboard_focus = true
			print("Modelica UI gained keyboard focus")
		else:
			has_keyboard_focus = false

# Helper method to add the root reference in a deferred way
func _add_root_reference(ref_node):
	get_tree().root.add_child(ref_node)
	print("Added ModelicaUI proxy to root")

# Helper method to add the modelica scene in a deferred way
func _add_modelica_scene(scene):
	$SubViewport.add_child(scene)
	print("Added modelica scene to SubViewport")

# Mouse enter/exit event handlers
func _on_mouse_entered():
	mouse_over_display = true

func _on_mouse_exited():
	mouse_over_display = false

# Handle 3D area input and translate to 2D viewport input
func _on_area_3d_input_event(_camera, event, mouse_position, _normal, _shape_idx):
	if not input_enabled:
		return
	
	# Mark this event as handled to prevent it from affecting avatar height
	if event is InputEventMouseButton:
		get_viewport().set_input_as_handled()
		
		# Handle mouse button click to activate display
		if event.button_index == MOUSE_BUTTON_LEFT and event.pressed:
			# When clicking on the modelica display, ensure it's active and has keyboard focus
			has_keyboard_focus = true
			mouse_button_pressed = true
			is_dragging = true
			
			# Make sure the display is visible
			if !is_display_visible:
				is_display_visible = true
				visible = true
			
			# Activate the controls directly and set focus immediately
			print("ModelicaUI: Area3D received direct click - activating input focus")
			_activate_direct_input_focus()
			_direct_set_focus()  # Call directly without deferring
			
			# Notify UI display manager that we've been clicked on and need to be active
			var controller = _find_display_controller()
			if controller and controller.ui_display_manager:
				print("ModelicaUI: Found display controller, activating UI")
				controller.ui_display_manager.on_modelica_display_clicked()
			else:
				print("ModelicaUI: Could not find display controller or UI manager")
				# Try to directly find the avatar and its UI display manager
				var avatar = get_tree().get_first_node_in_group("avatar")
				if avatar and avatar.has_node("UiDisplayManager"):
					print("ModelicaUI: Found avatar directly, activating UI")
					var ui_manager = avatar.get_node("UiDisplayManager")
					ui_manager.on_modelica_display_clicked()
				else:
					print("ModelicaUI: Could not find avatar or its UI manager")
			
			# Print focus state for debugging
			print("ModelicaUI gained keyboard focus from click")
		
		# Convert 3D position to 2D viewport coordinates
		var viewport_size = $SubViewport.size
		
		# Convert mouse position from global space to local space (accounts for rotation)
		var local_position = to_local(mouse_position)
		
		# The mesh is centered at origin, so local_position ranges from:
		# X: -mesh_size.x/2 to +mesh_size.x/2
		# Y: -mesh_size.y/2 to +mesh_size.y/2
		# Z: should be near 0 (on the surface)
		
		# Normalize to 0-1 range
		var local_2d_position = Vector2(
			(local_position.x / mesh_size.x) + 0.5,
			0.5 - (local_position.y / mesh_size.y)
		)
		
		# Convert to viewport coordinates
		var viewport_position = Vector2(
			local_2d_position.x * viewport_size.x,
			local_2d_position.y * viewport_size.y
		)
		
		# Create a new event for the viewport
		var viewport_event = InputEventMouseButton.new()
		viewport_event.button_index = event.button_index
		viewport_event.pressed = event.pressed
		viewport_event.position = viewport_position
		viewport_event.global_position = viewport_position
		viewport_event.double_click = event.double_click if event.has_method("double_click") else false
		
		# Send event to the viewport
		$SubViewport.push_input(viewport_event)
		
		# Only update last click position when pressing down
		if event.pressed:
			last_click_position = viewport_position
		
		# Debug
		print("Mouse button event forwarded to viewport at position: ", viewport_position)
	
	elif event is InputEventMouseMotion:
		get_viewport().set_input_as_handled()
		_handle_mouse_motion(mouse_position)

# Handle mouse motion events
func _handle_mouse_motion(mouse_position):
	var viewport_size = $SubViewport.size
	
	# Convert mouse position from global space to local space (accounts for rotation)
	var local_position = to_local(mouse_position)
	
	# Normalize to 0-1 range
	var local_2d_position = Vector2(
		(local_position.x / mesh_size.x) + 0.5,
		0.5 - (local_position.y / mesh_size.y)
	)
	
	var viewport_position = Vector2(
		local_2d_position.x * viewport_size.x,
		local_2d_position.y * viewport_size.y
	)
	
	var viewport_event = InputEventMouseMotion.new()
	viewport_event.position = viewport_position
	viewport_event.global_position = viewport_position
	
	# Calculate relative motion since last position
	if last_click_position != Vector2.ZERO:
		# Apply a smaller relative movement to match cursor movement more precisely
		viewport_event.relative = (viewport_position - last_click_position) * 0.75
	
	if mouse_button_pressed:
		viewport_event.button_mask = MOUSE_BUTTON_MASK_LEFT
	
	$SubViewport.push_input(viewport_event)
	last_click_position = viewport_position

# Find the display controller that contains the UiDisplayManager
func _find_display_controller():
	var controllers = get_tree().get_nodes_in_group("display_controller")
	if controllers.size() > 0:
		print("ModelicaUI: Found display controller: ", controllers[0])
		return controllers[0]
	
	print("ModelicaUI: No display controllers found in 'display_controller' group")
	return null

# Direct focus setting (without deferring)
func _direct_set_focus():
	if not is_instance_valid(modelica_scene):
		return
	
	# Priority-based focus selection (similar to _find_and_set_focus)
	
	# 1. First try to find the code editor (highest priority)
	var code_editors = []
	_find_specific_controls(modelica_scene, code_editors, ["CodeEdit", "TextEdit"], false)
	if code_editors.size() > 0:
		var editor = code_editors[0]
		print("ModelicaUI: Direct focus to code editor: ", editor.name)
		editor.grab_focus()
		# Force update the editor's focus state
		if editor.has_method("set_caret_line"):
			editor.set_caret_line(editor.get_caret_line())
		return
	
	# 2. Then try text fields (second priority)
	var text_fields = []
	_find_specific_controls(modelica_scene, text_fields, ["LineEdit", "SpinBox"], false)
	if text_fields.size() > 0:
		var text_field = text_fields[0]
		print("ModelicaUI: Direct focus to text field: ", text_field.name)
		text_field.grab_focus()
		return
		
	# 3. Then try buttons (third priority)
	var buttons = []
	_find_specific_controls(modelica_scene, buttons, ["Button"], false)
	if buttons.size() > 0:
		var button = buttons[0]
		print("ModelicaUI: Direct focus to button: ", button.name)
		button.grab_focus()
		return
		
	# 4. Finally try any focusable control
	var focusable_controls = []
	_find_focusable_controls(modelica_scene, focusable_controls, false)
	if focusable_controls.size() > 0:
		print("ModelicaUI: Direct focus to ", focusable_controls[0].name)
		focusable_controls[0].grab_focus()
		return
	
	print("ModelicaUI: No suitable controls found for direct focus")

# Special method to directly activate input focus bypassing the normal flow
func _activate_direct_input_focus():
	# This will send some fake input events to the SubViewport to ensure it's properly activated
	
	# First, try a text input event to activate any text fields
	var text_event = InputEventKey.new()
	text_event.pressed = true
	text_event.keycode = KEY_A  # Letter A as test
	text_event.unicode = 65     # ASCII for A
	$SubViewport.push_input(text_event)
	
	# Also create a mouse click in the center of the viewport
	var viewport_size = $SubViewport.size
	var click_event = InputEventMouseButton.new()
	click_event.button_index = MOUSE_BUTTON_LEFT
	click_event.pressed = true
	click_event.position = Vector2(viewport_size.x / 2, viewport_size.y / 2)
	$SubViewport.push_input(click_event)
	
	print("ModelicaUI: Sent direct activation events to SubViewport")

# New function to handle keyboard input from avatar
func receive_keyboard_input(event: InputEvent) -> bool:
	if not input_enabled or not is_display_visible:
		return false
		
	# Always process keyboard input regardless of has_keyboard_focus
	if event is InputEventKey:
		# Debug only major events with specific keys for performance
		if event.pressed and not event.is_echo() and event.keycode in [KEY_ENTER, KEY_ESCAPE, KEY_TAB]:
			print("ModelicaUI receiving key: ", event.keycode)
		
		# Make sure the SubViewport is set up to handle input
		$SubViewport.handle_input_locally = true
		$SubViewport.gui_disable_input = false
		
		# First, check if any control is actually focused in the UI
		var has_focused_control = false
		if is_instance_valid(modelica_scene):
			var focused_control = _get_focused_control(modelica_scene)
			has_focused_control = focused_control != null
		
		# If no control has focus and this is a typing key (not a navigation/system key),
		# try to set focus before accepting the event
		if event.pressed and not has_focused_control:
			var is_text_key = event.keycode > KEY_SPACE and event.keycode < KEY_ESCAPE
			if is_text_key:
				# If this is a typing key but no control has focus, set focus before accepting
				_find_and_set_focus()
				# Re-check if focus succeeded
				var focused_control = _get_focused_control(modelica_scene)
				if focused_control == null:
					# If we still don't have focus, don't handle the event
					return false
		
		# Create a copy of the keyboard event to forward to the viewport
		var viewport_event = InputEventKey.new()
		viewport_event.keycode = event.keycode
		viewport_event.physical_keycode = event.physical_keycode
		viewport_event.unicode = event.unicode
		viewport_event.echo = event.echo
		viewport_event.pressed = event.pressed
		viewport_event.alt_pressed = event.alt_pressed
		viewport_event.shift_pressed = event.shift_pressed
		viewport_event.ctrl_pressed = event.ctrl_pressed
		viewport_event.meta_pressed = event.meta_pressed
		
		# Send to viewport
		$SubViewport.push_input(viewport_event)
		
		# Return true to indicate we handled this event
		return true
	
	return false

# Helper method to find the currently focused control
func _get_focused_control(node: Node) -> Control:
	if node is Control and node.has_focus():
		return node
	
	for child in node.get_children():
		var focused = _get_focused_control(child)
		if focused != null:
			return focused
	
	return null

# New function to handle mouse input from avatar
func receive_mouse_input(event: InputEvent) -> bool:
	if not input_enabled or not is_display_visible:
		return false
		
	if event is InputEventMouseButton:
		# Get mouse position and convert to viewport coordinates
		var mouse_pos = event.position
		var camera = get_viewport().get_camera_3d()
		
		# Cast a ray to find the intersection with display
		var from = camera.project_ray_origin(mouse_pos)
		var to = from + camera.project_ray_normal(mouse_pos) * 1000
		
		var space_state = get_world_3d().direct_space_state
		var query = PhysicsRayQueryParameters3D.create(from, to)
		query.collide_with_areas = true
		query.collision_mask = 2  # Match the Area3D collision layer
		
		var result = space_state.intersect_ray(query)
		if result and result.collider == $Area3D:
			# Update dragging state
			if event.button_index == MOUSE_BUTTON_LEFT:
				mouse_button_pressed = event.pressed
				is_dragging = event.pressed
				
				# Set keyboard focus when clicking on the display
				if event.pressed:
					has_keyboard_focus = true
			
			# Calculate viewport coordinates
			var viewport_size = $SubViewport.size
			
			# Convert from global space to local space (accounts for rotation)
			var local_position = to_local(result.position)
			
			# Normalize to 0-1 range
			var local_2d_position = Vector2(
				(local_position.x / mesh_size.x) + 0.5,
				0.5 - (local_position.y / mesh_size.y)
			)
			
			var viewport_position = Vector2(
				local_2d_position.x * viewport_size.x,
				local_2d_position.y * viewport_size.y
			)
			
			# Create event for viewport
			var viewport_event = InputEventMouseButton.new()
			viewport_event.button_index = event.button_index
			viewport_event.pressed = event.pressed
			viewport_event.position = viewport_position
			viewport_event.global_position = viewport_position
			
			# Forward to viewport
			$SubViewport.push_input(viewport_event)
			
			# Update last click position
			if event.pressed:
				last_click_position = viewport_position
				
			return true
	
	# Handle mouse motion during drag even when outside the area
	if is_dragging and mouse_button_pressed and event is InputEventMouseMotion:
		var viewport_event = InputEventMouseMotion.new()
		viewport_event.position = last_click_position
		viewport_event.global_position = last_click_position
		viewport_event.relative = event.relative * 0.75  # Reduce movement speed to match cursor better
		viewport_event.button_mask = MOUSE_BUTTON_MASK_LEFT
		
		# Forward to viewport
		$SubViewport.push_input(viewport_event)
		return true
	
	# Handle mouse button release
	if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT and not event.pressed:
		if is_dragging:
			is_dragging = false
			mouse_button_pressed = false
			return true
			
	return false

# New function to toggle display visibility
func toggle_display():
	is_display_visible = !is_display_visible
	visible = is_display_visible
	input_enabled = is_display_visible
	
	# If becoming visible, try to set focus
	if is_display_visible:
		has_keyboard_focus = true
		# Try to find any Controls in the SubViewport and set focus to the first one
		call_deferred("_find_and_set_focus")
	else:
		# Reset focus when hiding the display
		has_keyboard_focus = false
	
	return is_display_visible

# New function to just release focus without affecting visibility
func release_focus():
	has_keyboard_focus = false
	
	# Find and release focus from any currently focused control
	if is_instance_valid(modelica_scene):
		var focused_control = _get_focused_control(modelica_scene)
		if focused_control and focused_control.has_focus():
			focused_control.release_focus()
			return true
	
	return false

# Helper method to find focusable controls in the SubViewport and set focus
func _find_and_set_focus():
	if not is_instance_valid(modelica_scene):
		return
		
	if not input_enabled or not has_keyboard_focus:
		return
	
	# Skip printing the entire scene tree for performance reasons
	# print_scene_tree(modelica_scene)
	
	# Priority-based focus selection
	
	# 1. First try to find the code editor (highest priority)
	var code_editors = []
	_find_specific_controls(modelica_scene, code_editors, ["CodeEdit", "TextEdit"], false)
	if code_editors.size() > 0:
		var editor = code_editors[0]
		print("ModelicaUI: Setting focus to code editor: ", editor.name)
		editor.grab_focus()
		# Force update the editor's focus state
		if editor.has_method("set_caret_line"):
			editor.set_caret_line(editor.get_caret_line())
		return
	
	# 2. Then try text fields (second priority)
	var text_fields = []
	_find_specific_controls(modelica_scene, text_fields, ["LineEdit", "SpinBox"], false)
	if text_fields.size() > 0:
		var text_field = text_fields[0]
		print("ModelicaUI: Setting focus to text field: ", text_field.name)
		text_field.grab_focus()
		return
		
	# 3. Then try buttons (third priority)
	var buttons = []
	_find_specific_controls(modelica_scene, buttons, ["Button"], false)
	if buttons.size() > 0:
		var button = buttons[0]
		print("ModelicaUI: Setting focus to button: ", button.name)
		button.grab_focus()
		return
	
	# 4. Finally, try any other focusable control
	var focusable_controls = []
	_find_focusable_controls(modelica_scene, focusable_controls, false)
	
	if focusable_controls.size() > 0:
		print("ModelicaUI: Setting focus to ", focusable_controls[0].name)
		focusable_controls[0].grab_focus()
	else:
		print("ModelicaUI: No focusable controls found")

# Find specific types of controls
func _find_specific_controls(node: Node, result: Array, types: Array, debug: bool = true):
	var node_class = node.get_class()
	if types.has(node_class):
		result.append(node)
		if debug:
			print("ModelicaUI: Found control of type ", node_class, ": ", node.name)
	
	for child in node.get_children():
		_find_specific_controls(child, result, types, debug)

# Modified for better debugging
func _find_focusable_controls(node: Node, result: Array, debug: bool = true):
	if node is Control:
		if node.focus_mode != Control.FOCUS_NONE:
			result.append(node)
			if debug:
				print("ModelicaUI: Found focusable control: ", node.name, " of type ", node.get_class())
		elif debug:
			print("ModelicaUI: Found non-focusable control: ", node.name, " of type ", node.get_class())
	
	for child in node.get_children():
		_find_focusable_controls(child, result, debug)

# Debug function to print the scene tree
func print_scene_tree(node, indent=""):
	print(indent + node.name + " (" + node.get_class() + ")")
	
	if node is Control:
		print(indent + "  Focus mode: " + str(node.focus_mode))
		
	for child in node.get_children():
		print_scene_tree(child, indent + "  ") 
