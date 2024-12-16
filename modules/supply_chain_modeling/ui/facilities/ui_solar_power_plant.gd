extends UIBaseFacility
	
var solar_power_plant: SolarPowerPlant	

func _init():
	super._init()
	set_facility_properties("SolarPowerPlant", "Solar power generation facility", "producer")
	solar_power_plant = SolarPowerPlant.new()
	facility.efficiency = 0.20  # 20% efficiency is typical for solar panels

func _physics_process(delta: float) -> void:
	if facility.status != "Running":
		return
		
	# Calculate actual power output based on conditions
	var actual_output = solar_power_plant.power_output * facility.efficiency * solar_power_plant.solar_irradiance * delta
	
	# Implementation will depend on how power distribution is handled

func _process(delta: float) -> void:
	update_status_display()

func update_status_display() -> void:
	var label = $Label
	if label:
		var current_output = solar_power_plant.power_output * facility.efficiency * solar_power_plant.solar_irradiance
		label.text = "Output: %.1f kW" % current_output

func set_solar_irradiance(new_irradiance: float) -> void:
	solar_power_plant.solar_irradiance = new_irradiance

func set_panel_area(new_area: float) -> void:
	solar_power_plant.panel_area = new_area
	solar_power_plant.power_output = solar_power_plant.panel_area * 1.0  # Assuming 1kW/m standard conditions
