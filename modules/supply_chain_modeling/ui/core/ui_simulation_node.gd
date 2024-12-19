class_name UISimulationNode
extends GraphNode

var simulation_node: SimulationNode

func _init():
	pass

func _ready() -> void:
	set_physics_process(false) # UI doesn't need physics processing

func update_from_simulation() -> void:
	if simulation_node:
		# Update UI based on simulation state
		pass

func _process(delta: float) -> void:
	update_status_display()

func update_status_display() -> void:
	pass
