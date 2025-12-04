extends Node

const VIEWER_SCENE_PATH = "res://apps/supply_chain_modeling/ui/simple_graph_viewer.tscn"

enum DisplayMode {
	HIDDEN,           # Not shown
	FLOATING_3D,      # Floating near entity in 3D space
	FULLSCREEN        # Fullscreen overlay window
}

var viewer_3d: Node3D = null  # 3D floating display
var viewer_fullscreen: Control = null  # Fullscreen overlay
var viewer_instance_3d: Control = null  # Graph viewer for 3D
var viewer_instance_fullscreen: Control = null  # Graph viewer for fullscreen
var current_target: Node3D = null
var current_mode: DisplayMode = DisplayMode.HIDDEN
var floating_enabled: bool = true  # Can be toggled with 'G'

func _ready():
	# Connect to BuilderManager
	var builder_manager = get_node_or_null("/root/BuilderManager")
	if builder_manager:
		builder_manager.entity_selected.connect(_on_entity_selected)
	else:
		push_warning("FloatingScreenManager: BuilderManager not found")

func _input(event):
	if not current_target:
		return
	
	# Toggle floating display with 'G'
	if event is InputEventKey and event.pressed and not event.echo:
		if event.keycode == KEY_G and not event.shift_pressed:
			toggle_floating_display()
			get_viewport().set_input_as_handled()
		
		# Open fullscreen with Shift+G
		elif event.keycode == KEY_G and event.shift_pressed:
			show_fullscreen()
			get_viewport().set_input_as_handled()
		
		# Close fullscreen with Esc
		elif event.keycode == KEY_ESCAPE and current_mode == DisplayMode.FULLSCREEN:
			hide_fullscreen()
			get_viewport().set_input_as_handled()

func _on_entity_selected(entity):
	if entity == current_target:
		return
		
	current_target = entity
	
	if entity and (entity is LCVehicle or entity.get("solver_graph")):
		if floating_enabled:
			_show_floating_display(entity)
	else:
		_hide_all_displays()

func toggle_floating_display():
	floating_enabled = !floating_enabled
	if floating_enabled and current_target:
		_show_floating_display(current_target)
	else:
		_hide_floating_display()
	print("FloatingScreenManager: Floating display ", "enabled" if floating_enabled else "disabled")

func show_fullscreen():
	if not current_target or not current_target.get("solver_graph"):
		return
	
	_hide_floating_display()
	
	if not viewer_fullscreen:
		_create_fullscreen_viewer()
	
	if viewer_fullscreen and viewer_instance_fullscreen:
		viewer_fullscreen.visible = true
		viewer_instance_fullscreen.set_graph(current_target.solver_graph)
		current_mode = DisplayMode.FULLSCREEN
		
		# Pause game or capture input
		Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
		
		print("FloatingScreenManager: Fullscreen mode activated")

func hide_fullscreen():
	if viewer_fullscreen:
		viewer_fullscreen.visible = false
	current_mode = DisplayMode.HIDDEN
	
	# Restore floating if enabled
	if floating_enabled and current_target:
		_show_floating_display(current_target)
	
	print("FloatingScreenManager: Fullscreen mode closed")

func _show_floating_display(entity):
	if not viewer_3d:
		_create_3d_viewer()
	
	if viewer_3d and viewer_instance_3d:
		viewer_3d.visible = true
		current_mode = DisplayMode.FLOATING_3D
		
		# Position relative to entity
		viewer_3d.global_position = entity.global_position + Vector3(5.0, 3.0, 0.0)
		
		# Set graph
		if entity.get("solver_graph"):
			viewer_instance_3d.set_graph(entity.solver_graph)
			print("FloatingScreenManager: Showing floating graph for ", entity.name, " with ", entity.solver_graph.nodes.size(), " nodes")

func _hide_floating_display():
	if viewer_3d:
		viewer_3d.visible = false
	if current_mode == DisplayMode.FLOATING_3D:
		current_mode = DisplayMode.HIDDEN

func _hide_all_displays():
	_hide_floating_display()
	hide_fullscreen()
	current_target = null

func _create_3d_viewer():
	# Create a 3D node with SubViewport to display the 2D graph viewer
	viewer_3d = Node3D.new()
	viewer_3d.name = "FloatingGraphViewer"
	add_child(viewer_3d)
	
	# Create SubViewport (larger resolution for better quality)
	var viewport = SubViewport.new()
	viewport.size = Vector2i(2048, 1536)
	viewport.transparent_bg = true
	viewer_3d.add_child(viewport)
	
	# Load and add the graph viewer to the viewport
	var viewer_scene = load(VIEWER_SCENE_PATH)
	if viewer_scene:
		viewer_instance_3d = viewer_scene.instantiate()
		viewport.add_child(viewer_instance_3d)
	else:
		push_error("FloatingScreenManager: Could not load simple_graph_viewer.tscn")
		return
	
	# Create a mesh to display the viewport texture (larger size)
	var mesh_instance = MeshInstance3D.new()
	var quad_mesh = QuadMesh.new()
	quad_mesh.size = Vector2(20, 15)
	mesh_instance.mesh = quad_mesh
	viewer_3d.add_child(mesh_instance)
	
	# Create material with viewport texture
	var material = StandardMaterial3D.new()
	material.albedo_texture = viewport.get_texture()
	material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	mesh_instance.material_override = material

func _create_fullscreen_viewer():
	# Create fullscreen overlay
	viewer_fullscreen = Control.new()
	viewer_fullscreen.name = "FullscreenGraphViewer"
	viewer_fullscreen.set_anchors_preset(Control.PRESET_FULL_RECT)
	viewer_fullscreen.mouse_filter = Control.MOUSE_FILTER_STOP  # Capture all input
	get_tree().root.add_child(viewer_fullscreen)
	
	# Semi-transparent background
	var bg = ColorRect.new()
	bg.color = Color(0, 0, 0, 0.8)
	bg.set_anchors_preset(Control.PRESET_FULL_RECT)
	viewer_fullscreen.add_child(bg)
	
	# Title bar
	var title_bar = PanelContainer.new()
	title_bar.set_anchors_preset(Control.PRESET_TOP_WIDE)
	title_bar.offset_bottom = 50
	viewer_fullscreen.add_child(title_bar)
	
	var title_hbox = HBoxContainer.new()
	title_bar.add_child(title_hbox)
	
	var title_label = Label.new()
	title_label.text = "Resource Network Inspector"
	title_label.add_theme_font_size_override("font_size", 24)
	title_hbox.add_child(title_label)
	
	title_hbox.add_child(Control.new())  # Spacer
	title_hbox.get_child(-1).size_flags_horizontal = Control.SIZE_EXPAND_FILL
	
	# Close button
	var close_btn = Button.new()
	close_btn.text = "✕ Close (Esc)"
	close_btn.pressed.connect(hide_fullscreen)
	title_hbox.add_child(close_btn)
	
	# Graph viewer container
	var viewer_container = MarginContainer.new()
	viewer_container.set_anchors_preset(Control.PRESET_FULL_RECT)
	viewer_container.offset_top = 60
	viewer_container.offset_left = 20
	viewer_container.offset_right = -20
	viewer_container.offset_bottom = -20
	viewer_fullscreen.add_child(viewer_container)
	
	# Load graph viewer
	var viewer_scene = load(VIEWER_SCENE_PATH)
	if viewer_scene:
		viewer_instance_fullscreen = viewer_scene.instantiate()
		viewer_container.add_child(viewer_instance_fullscreen)
	else:
		push_error("FloatingScreenManager: Could not load simple_graph_viewer.tscn for fullscreen")

func _process(delta):
	if viewer_3d and viewer_3d.visible and current_target:
		# Keep position relative to target
		viewer_3d.global_position = current_target.global_position + Vector3(5.0, 3.0, 0.0)
		
		# Make it face the camera
		var camera = get_viewport().get_camera_3d()
		if camera:
			viewer_3d.look_at(camera.global_position, Vector3.UP)
			viewer_3d.rotate_y(PI)  # Flip to face camera
