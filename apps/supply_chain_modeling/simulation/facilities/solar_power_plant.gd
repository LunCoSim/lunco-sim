class_name SolarPowerPlant
extends BaseFacility

@export var power_output: float = 1000.0  # kW
@export var solar_irradiance: float = 1.0  # kW/m² (1.0 is approx. Earth's average)
@export var panel_area: float = 100.0  # m²

@export var current_output: float = 0.0
func _init():
	pass


func _physics_process(delta: float) -> void:
	if status != "Running":
		return
		
	# Calculate actual power output based on conditions
	current_output = power_output * efficiency * delta
	
	# Implementation will depend on how power distribution is handled

func set_solar_irradiance(new_irradiance: float) -> void:
	solar_irradiance = new_irradiance

func set_panel_area(new_area: float) -> void:
	panel_area = new_area
	power_output = panel_area * solar_irradiance  # Assuming 1kW/m standard conditions

func save_state() -> Dictionary:
	var state = super.save_state()


	return state

func load_state(state: Dictionary) -> void:
	super.load_state(state)
