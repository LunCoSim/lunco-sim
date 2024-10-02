@tool
# Solar power station facility that generates electricity based on its area
class_name LCSolarPowerStationFacility
extends LCFacilityBlank

@export var efficiency: float = 0.2  # Solar panel efficiency (typical range: 0.15 to 0.22)
@export var solar_irradiance: float = 1.0  # kW/m² (1.0 is approx. Earth's average)

var energy_output: float = 0.0  # kW

func _ready():
	super._ready()
	calculate_energy_output()

func calculate_energy_output():
	var area = size.x * size.z  # Assuming the facility is flat and the y-axis is height
	energy_output = area * efficiency * solar_irradiance

func _process(delta):
	pass
	# Here you would implement the actual energy generation
	# For example, you might have a global resource manager:
	# ResourceManager.add_electricity(energy_output * delta / 3600)  # Convert from kW to kWh

func set_size(new_size: Vector3):
	super.set_size(new_size)
	calculate_energy_output()

func set_efficiency(new_efficiency: float):
	efficiency = new_efficiency
	calculate_energy_output()

func set_solar_irradiance(new_irradiance: float):
	solar_irradiance = new_irradiance
	calculate_energy_output()

func get_energy_output() -> float:
	return energy_output
