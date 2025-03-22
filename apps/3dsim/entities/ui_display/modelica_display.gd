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
var is_visible = true

func _ready():
	# Add to group for easy identification
	add_to_group("modelica_display")
	
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
	box_shape.size = Vector3(40, 30, 0.1)
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

func _on_focus_changed(control):
	# This is called when focus changes in the GUI
	pass

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
func _on_area_3d_input_event(camera, event, position, normal, shape_idx):
	if not input_enabled:
		return
	
	# Mark this event as handled to prevent it from affecting avatar height
	if event is InputEventMouseButton:
		get_viewport().set_input_as_handled()
		
		# Update dragging state
		if event.button_index == MOUSE_BUTTON_LEFT:
			mouse_button_pressed = event.pressed
			is_dragging = event.pressed
		
		# Convert 3D position to 2D viewport coordinates
		var viewport_size = $SubViewport.size
		var mesh_size = Vector2(40, 30)  # Size of our quad mesh
		
		# Calculate normalized position on the mesh (0-1)
		var local_position = position - global_position
		var local_2d_position = Vector2(
			(local_position.x / mesh_size.x + 0.5), 
			(0.5 - local_position.y / mesh_size.y)
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
func _handle_mouse_motion(position):
	var viewport_size = $SubViewport.size
	var mesh_size = Vector2(40, 30)
	var local_position = position - global_position
	var local_2d_position = Vector2(
		(local_position.x / mesh_size.x + 0.5), 
		(0.5 - local_position.y / mesh_size.y)
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

# Global input handler to catch mouse release outside the area and handle scroll wheel
func _input(event):
	if not input_enabled:
		return
	
	# Handle mouse wheel events when mouse is over display
	if mouse_over_display and event is InputEventMouseButton and (event.button_index == MOUSE_BUTTON_WHEEL_UP or event.button_index == MOUSE_BUTTON_WHEEL_DOWN):
		# Get mouse position and convert to viewport coordinates
		var mouse_pos = get_viewport().get_mouse_position()
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
			# Convert position to viewport coordinates
			var viewport_size = $SubViewport.size
			var mesh_size = Vector2(40, 30)
			var local_position = result.position - global_position
			var local_2d_position = Vector2(
				(local_position.x / mesh_size.x + 0.5), 
				(0.5 - local_position.y / mesh_size.y)
			)
			var viewport_position = Vector2(
				local_2d_position.x * viewport_size.x,
				local_2d_position.y * viewport_size.y
			)
			
			# Create scroll event for viewport
			var viewport_event = InputEventMouseButton.new()
			viewport_event.button_index = event.button_index
			viewport_event.pressed = event.pressed
			viewport_event.position = viewport_position
			viewport_event.global_position = viewport_position
			
			# Forward to viewport
			$SubViewport.push_input(viewport_event)
			
			# Stop event propagation to prevent avatar height change
			get_viewport().set_input_as_handled()
	
	# Handle mouse motion during drag even when outside the area
	if is_dragging and mouse_button_pressed and event is InputEventMouseMotion:
		# We don't need to check if we're over the display - if we're dragging
		# we want to continue processing mouse motion events
		var viewport_event = InputEventMouseMotion.new()
		viewport_event.position = last_click_position
		viewport_event.global_position = last_click_position
		viewport_event.relative = event.relative * 0.75  # Reduce movement speed to match cursor better
		viewport_event.button_mask = MOUSE_BUTTON_MASK_LEFT
		
		# Forward to viewport
		$SubViewport.push_input(viewport_event)
		
		# Stop event propagation
		get_viewport().set_input_as_handled()
	
	# Handle mouse button release
	if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT and not event.pressed:
		if is_dragging:
			is_dragging = false
			mouse_button_pressed = false

# Toggle visibility of the display
func toggle_display():
	is_visible = !is_visible
	visible = is_visible
	input_enabled = is_visible 