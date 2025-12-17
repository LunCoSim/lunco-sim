extends SceneTree

## Test for Floating Screen Manager

func _initialize():
	print("\n=== Testing Floating Screen Manager ===\n")
	
	# Create root node
	var root = Node3D.new()
	root.name = "Root"
	get_root().add_child(root)
	
	# Mock BuilderManager
	var builder_manager = Node.new()
	builder_manager.name = "BuilderManager"
	builder_manager.add_user_signal("entity_selected", [{"name": "entity", "type": TYPE_OBJECT}])
	get_root().add_child(builder_manager)
	
	# Create FloatingScreenManager
	var manager_script = load("res://apps/3dsim/managers/floating_screen_manager.gd")
	var manager = manager_script.new()
	root.add_child(manager)
	
	# Create a dummy vehicle with solver graph
	var vehicle = Node3D.new()
	vehicle.name = "TestVehicle"
	vehicle.set_script(load("res://core/base/vehicle.gd"))
	root.add_child(vehicle)
	
	# Initialize vehicle (creates solver graph)
	vehicle._ready()
	
	print("Vehicle solver graph: ", vehicle.solver_graph)
	
	# Simulate selection
	print("Selecting vehicle...")
	builder_manager.emit_signal("entity_selected", vehicle)
	
	# Check if display was created
	await create_timer(0.1).timeout
	
	if manager.display_instance:
		print("✅ Display instance created")
		print("Display visible: ", manager.display_instance.visible)
		print("Display position: ", manager.display_instance.global_position)
	else:
		print("❌ Display instance NOT created")
	
	# Simulate deselection
	print("Deselecting...")
	builder_manager.emit_signal("entity_selected", null)
	
	if manager.display_instance and not manager.display_instance.visible:
		print("✅ Display hidden on deselect")
	else:
		print("❌ Display NOT hidden on deselect")
	
	# Cleanup
	root.queue_free()
	builder_manager.queue_free()
	quit(0)
