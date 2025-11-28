extends Node3D

# Path to the supply chain modeling scene
const SUPPLY_CHAIN_SCENE_PATH = "res://apps/supply_chain_modeling/rsct.tscn"

# State variables
var supply_chain_scene = null
var input_enabled = true
var last_click_position = Vector2()
var is_dragging = false
var mouse_button_pressed = false
var mouse_over_display = false
var has_keyboard_focus = false
var is_display_visible = true
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
	add_to_group("supply_chain_display")
	
	# Get the actual mesh size from the DisplayMesh
	mesh_size = _get_mesh_size()
	print("RSCT: Mesh size detected as: ", mesh_size)
	
	# First, add a reference to the scene at root level for scripts that use absolute paths
	# This needs to happen BEFORE loading the supply chain scene so other nodes can find it
	if not get_node_or_null("/root/RSCT"):
		var root_ref = Node.new()
		root_ref.name = "RSCT"
		root_ref.set_script(load("res://apps/3dsim/entities/ui_display/rsct_proxy.gd"))
		call_deferred("_add_root_reference", root_ref)
	
	# Load the supply chain scene into the SubViewport (deferred)
	supply_chain_scene = load(SUPPLY_CHAIN_SCENE_PATH).instantiate()
	supply_chain_scene.name = "RSCT"  # Set the name to match what other scripts are looking for
	call_deferred("_add_supply_chain_scene", supply_chain_scene)
		
	# Set up mesh to display the viewport texture
	var viewport_texture = $SubViewport.get_texture()
	var material = $DisplayMesh.get_surface_override_material(0)
	material.albedo_texture = viewport_texture
	
	# Set up collision shape to match mesh for interaction
	var collision_shape = $Area3D/CollisionShape3D
	var box_shape = BoxShape3D.new()
	box_shape.size = Vector3(mesh_size.x, mesh_size.y, 0.1)
	collision_shape.shape = box_shape
	
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

# Called when focus changes in the GUI
func _on_focus_changed(control):
	# If a control in our SubViewport gained focus, we should capture keyboard events
	if control and is_instance_valid(control) and control.is_inside_tree():
		var parent_viewport = control.get_viewport()
		if parent_viewport == $SubViewport:
			has_keyboard_focus = true
			print("Supply Chain UI gained keyboard focus")
		else:
			has_keyboard_focus = false

# Helper method to add the root reference in a deferred way
func _add_root_reference(ref_node):
	get_tree().root.add_child(ref_node)
	print("Added RSCT proxy to root")

# Helper method to add the supply chain scene in a deferred way
func _add_supply_chain_scene(scene):
	$SubViewport.add_child(scene)
	print("Added supply chain scene to SubViewport")

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
		
		# Update dragging state
		if event.button_index == MOUSE_BUTTON_LEFT:
			mouse_button_pressed = event.pressed
			is_dragging = event.pressed
			
			# Set keyboard focus when clicking on the display
			if event.pressed:
				has_keyboard_focus = true
		
		# Convert 3D position to 2D viewport coordinates
		var viewport_size = $SubViewport.size
		
		# Convert mouse position from global space to local space (accounts for rotation)
		var local_position = to_local(mouse_position)
		
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
		_handle_mouse_motion(position)

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

# New function to handle keyboard input from avatar
func receive_keyboard_input(event: InputEvent) -> bool:
	if not input_enabled or not is_display_visible:
		return false
		
	if event is InputEventKey:
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
		return true
	
	return false

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

# Toggle visibility of the display
func toggle_display():
	is_display_visible = !is_display_visible
	visible = is_display_visible
	input_enabled = is_display_visible

	# Reset keyboard focus when hiding
	if !is_display_visible:
		has_keyboard_focus = false
