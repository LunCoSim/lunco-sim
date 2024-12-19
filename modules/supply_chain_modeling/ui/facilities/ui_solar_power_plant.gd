class_name UISolarPowerPlant
extends UIBaseFacility
	
var solar_power_plant: SolarPowerPlant	

func _init():
	super._init()
	# set_facility_properties("SolarPowerPlant", "Solar power generation facility", "producer")
	solar_power_plant = SolarPowerPlant.new()
	facility.efficiency = 0.20  # 20% efficiency is typical for solar panels

func _process(delta: float) -> void:
	update_status_display()

func update_status_display() -> void:
	var label = $Label
	if label:
		var current_output = solar_power_plant.current_output
		label.text = "Output: %.1f kW" % current_output
