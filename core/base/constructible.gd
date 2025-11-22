class_name LCConstructible
extends RigidBody3D

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
	
	# Find existing components
	for child in get_children():
		if child is LCComponent:
			register_component(child)

func register_component(component: LCComponent):
	if not component in components:
		components.append(component)
		component_added.emit(component)
		
		# Reparent if not already a child (logic might vary based on attachment type)
		if component.get_parent() != self:
			# This case handles components that might be spawned separately and then attached
			# For now, we assume they are added as children
			pass
			
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
