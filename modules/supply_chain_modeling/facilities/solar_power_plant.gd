extends BaseFacility
	
@export var power_output: float = 1000.0  # kW
@export var solar_irradiance: float = 1.0  # kW/m² (1.0 is approx. Earth's average)
@export var panel_area: float = 100.0  # m²

func _init():
	super._init()
	set_facility_properties("SolarPowerPlant", "Solar power generation facility", "producer")
	efficiency = 0.20  # 20% efficiency is typical for solar panels

func process_resources(delta: float) -> void:
	if status != "Running":
		return
		
	# Calculate actual power output based on conditions
	var actual_output = power_output * efficiency * solar_irradiance * delta
	
	# Implementation will depend on how power distribution is handled

func update_status_display() -> void:
	var label = $Label
	if label:
		var current_output = power_output * efficiency * solar_irradiance
		label.text = "Output: %.1f kW" % current_output

func set_solar_irradiance(new_irradiance: float) -> void:
	solar_irradiance = new_irradiance
	update_status_display()

func set_panel_area(new_area: float) -> void:
	panel_area = new_area
	power_output = panel_area * 1.0  # Assuming 1kW/m�� standard conditions
	update_status_display() 
