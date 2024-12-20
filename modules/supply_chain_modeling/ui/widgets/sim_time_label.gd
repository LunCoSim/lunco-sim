extends Label

func _process(delta: float) -> void:
	update_sim_time_label()

func update_sim_time_label() -> void:
	text = "Sim Time: " + str(round(%Simulation.get_simulation_time_scaled())) + " minutes"
