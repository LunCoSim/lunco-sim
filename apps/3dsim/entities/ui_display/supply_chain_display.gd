extends Node3D

# Path to the supply chain modeling scene
const SUPPLY_CHAIN_SCENE_PATH = "res://apps/supply_chain_modeling/rsct.tscn"

# State variables
var supply_chain_scene = null
var input_enabled = true
var last_click_position = Vector2()

func _ready():
	# Add to group for easy identification
	add_to_group("supply_chain_display")
	
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
	box_shape.size = Vector3(4, 3, 0.1) # Increased size
	collision_shape.shape = box_shape
	
	# Make sure the SubViewport receives input events
	$SubViewport.handle_input_locally = true
	$SubViewport.gui_disable_input = false

# Helper method to add the root reference in a deferred way
func _add_root_reference(ref_node):
	get_tree().root.add_child(ref_node)
	print("Added RSCT proxy to root")

# Helper method to add the supply chain scene in a deferred way
func _add_supply_chain_scene(scene):
	$SubViewport.add_child(scene)
	print("Added supply chain scene to SubViewport")

# Handle 3D area input and translate to 2D viewport input
func _on_area_3d_input_event(camera, event, position, normal, shape_idx):
	if not input_enabled:
		return
		
	if event is InputEventMouseButton:
		# Convert 3D position to 2D viewport coordinates
		var viewport_size = $SubViewport.size
		var mesh_size = Vector2(4, 3)  # Size of our quad mesh - INCREASED
		
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
		
		# Send event to the viewport
		$SubViewport.push_input(viewport_event)
		last_click_position = viewport_position
		
		# Debug
		print("Mouse button event forwarded to viewport at position: ", viewport_position)
	
	elif event is InputEventMouseMotion and last_click_position != Vector2():
		# Similar conversion for motion events
		var viewport_size = $SubViewport.size
		var mesh_size = Vector2(4, 3) # INCREASED size
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
		
		$SubViewport.push_input(viewport_event)

# Toggle the display on/off
func toggle_display():
	visible = !visible
	input_enabled = visible

# Set the display state
func set_display_state(state):
	visible = state
	input_enabled = state 
