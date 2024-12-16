class_name SolarPowerPlant
extends BaseFacility

@export var power_output: float = 1000.0  # kW
@export var solar_irradiance: float = 1.0  # kW/m² (1.0 is approx. Earth's average)
@export var panel_area: float = 100.0  # m²

func _init():
	pass
	
