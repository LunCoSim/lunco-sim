class_name LCComponent
extends LCSpaceSystem

# Physical properties
@export var mass: float = 1.0
@export var power_consumption: float = 0.0
@export var power_production: float = 0.0

# Attachment
# We use markers to define where OTHER components can attach to THIS component
@export var attachment_nodes: Array[Node3D] = []

func _ready():
	super._ready()
	# Auto-find attachment nodes if not manually assigned
	if attachment_nodes.is_empty():
		for child in get_children():
			if child is Marker3D and child.name.begins_with("Attach"):
				attachment_nodes.append(child)

# XTCE Interface (Wrappers around LCSpaceSystem's dictionaries)
func get_telemetry() -> Dictionary:
	# In a real implementation, this would sample values from the component's logic
	return Telemetry

func execute_command(cmd_name: String, args: Array):
	if Commands.has(cmd_name):
		# Dispatch to specific method if it exists
		if has_method("cmd_" + cmd_name):
			callv("cmd_" + cmd_name, args)
		else:
			print("Command defined but no handler: ", cmd_name)
