class_name LCConstructible
extends VehicleBody3D

# Signals
signal component_added(component)
signal component_removed(component)

# List of attached components
var components: Array[LCComponent] = []

# Network synchronizer for the whole assembly
@export var synchronizer: MultiplayerSynchronizer

func _ready():
	# Ensure we are in the constructible group
	add_to_group("Constructibles")
	
	# Add a basic rover controller so the entity can be controlled
	# We'll use the existing rover controller as a template
	var controller_scene = load("res://controllers/rover/rover-controller.tscn")
	if controller_scene:
		var controller = controller_scene.instantiate()
		controller.name = "Controller"
		add_child(controller)
		print("Constructible: Added rover controller")
	else:
		push_warning("Constructible: Could not load rover controller")
	
	# Find existing components
	for child in get_children():
		if child is LCComponent:
			register_component(child)
	
	# VehicleBody3D works fine as a RigidBody without wheels. 
	# User will add wheels manually via Builder.

func register_component(component: LCComponent):
	if not component in components:
		components.append(component)
		component_added.emit(component)
		
		print("Constructible: Registering component: ", component.name, " at position: ", component.position)
		
		# Reparent if not already a child (logic might vary based on attachment type)
		if component.get_parent() != self:
			# This case handles components that might be spawned separately and then attached
			# For now, we assume they are added as children
			pass
		
		# IMPORTANT: VehicleBody3D requires specific nodes as direct children:
		# - CollisionShape3D for collision detection
		# - VehicleWheel3D for wheel physics
		# Components have these as children, so we need to reparent them
		for child in component.get_children():
			if child is CollisionShape3D:
				print("Constructible: Reparenting collision shape from component to VehicleBody3D")
				# Store the global transform
				var global_trans = child.global_transform
				# Reparent to the VehicleBody3D
				child.reparent(self)
				# Restore global position
				child.global_transform = global_trans
			elif child is VehicleWheel3D:
				print("Constructible: Reparenting wheel from component to VehicleBody3D")
				# Store the global transform
				var global_trans = child.global_transform
				# Reparent to the VehicleBody3D
				child.reparent(self)
				# Restore global position
				child.global_transform = global_trans
		
		print("Constructible: Component registered. Total components: ", components.size())
			
		# Recalculate mass/CoM
		recalculate_physics()

func unregister_component(component: LCComponent):
	if component in components:
		components.erase(component)
		component_removed.emit(component)
		recalculate_physics()

func recalculate_physics():
	# Simple mass aggregation
	var total_mass = 1.0 # Base mass
	
	for comp in components:
		total_mass += comp.mass
		
	mass = total_mass
	# TODO: Calculate Center of Mass properly based on component positions

# XTCE Aggregation
func get_telemetry_data() -> Dictionary:
	var data = {}
	for comp in components:
		var comp_data = comp.get_telemetry()
		# Merge with some prefix or structure
		data[comp.name] = comp_data
	return data

func execute_command(component_name: String, command_name: String, args: Array):
	for comp in components:
		if comp.name == component_name:
			comp.execute_command(command_name, args)
			return
