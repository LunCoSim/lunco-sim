class_name UISolarPowerPlant
extends UIBaseFacility
	

func _process(delta: float) -> void:
	update_status_display()

func update_status_display() -> void:
	if not simulation_node is SolarPowerPlant:
		return

	var solar_power_plant = simulation_node as SolarPowerPlant	

	var label = $Label
	if label:
		var current_output = solar_power_plant.current_output
		label.text = "Output: %.1f kW" % current_output
