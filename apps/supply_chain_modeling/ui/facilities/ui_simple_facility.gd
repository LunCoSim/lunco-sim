class_name UISimpleFacility
extends UISimulationNode

func _ready():
	super._ready()
	# Do not override slots here. Let the scene definition control them.
	
func update_status_display() -> void:
	# Optional: Update status label if it exists
	var status_label = find_child("Status", true, false)
	if status_label and simulation_node:
		if "status" in simulation_node:
			status_label.text = "Status: " + str(simulation_node.status)
