class_name UIBaseFacility
extends UISimulationNode


func _init():
	# Set up basic GraphNode properties
	mouse_filter = MOUSE_FILTER_PASS
	resizable = true


func update_from_simulation() -> void:
	super.update_from_simulation()
	if simulation_node:
		$Parameters/Status.text = simulation_node.properties.status
		$Parameters/Efficiency.text = "Efficiency: " + str(simulation_node.properties.efficiency)

func _process(delta: float) -> void:
	update_status_display()

func update_status_display() -> void:
	# Virtual method to be implemented by child classes
	pass
