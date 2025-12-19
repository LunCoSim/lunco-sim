class_name LCISRUProcessor
extends LCProcessEffector

## In-Situ Resource Utilization Processor
##
## Extracts oxygen from lunar regolith using hydrogen reduction.
## Operates at high temperature (1000°C+).

@export var operating_temperature: float = 1273.15  # 1000°C in Kelvin
@export var current_temperature: float = 293.15  # Current temp

func _ready():
	super._ready()
	recipe_id = "regolith_reduction"
	_load_recipe()

func _physics_process(delta: float):
	# Heat up when active
	if is_active and current_temperature < operating_temperature:
		current_temperature += delta * 10.0  # Heat up rate
	elif not is_active and current_temperature > 293.15:
		current_temperature -= delta * 5.0  # Cool down rate
	
	current_temperature = clamp(current_temperature, 293.15, operating_temperature + 100.0)
	
	# Efficiency based on temperature
	if current_temperature >= operating_temperature:
		current_efficiency = 1.0
	else:
		current_efficiency = current_temperature / operating_temperature * 0.5  # Reduced efficiency when cold
	
	super._physics_process(delta)

func get_status() -> String:
	if current_temperature < operating_temperature * 0.9:
		return "Heating Up"
	return super.get_status()
