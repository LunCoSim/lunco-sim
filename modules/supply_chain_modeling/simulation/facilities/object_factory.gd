class_name ObjectFactory
extends BaseFacility

# Input/output rates
@export var o2_input_rate: float = 1.0  # units/minute
@export var h2_input_rate: float = 2.0  # units/minute
@export var power_input_rate: float = 100.0  # kW
@export var h2o_output_rate: float = 1.0  # units/minute
@export var power_consumption: float = 100.0  # kW

# Current resource amounts
@export var o2_stored: float = 0.0
@export var h2_stored: float = 0.0
@export var power_available: float = 0.0

func _init():
	pass
	
