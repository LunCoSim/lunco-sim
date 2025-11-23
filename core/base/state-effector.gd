class_name LCStateEffector
extends LCComponent

## Base class for components that contribute state (mass, inertia, power) to a vehicle.
##
## Implements the "State Effector" concept from Basilisk.
## These components are passive and provide property contributions when queried.

func _ready():
	super._ready()

## Returns the mass of this component in kg.
## Override this if mass is dynamic (e.g. fuel tank).
func get_mass_contribution() -> float:
	return mass

## Returns the inertia tensor contribution of this component.
## Currently returns zero (point mass approximation).
## Override for accurate physics.
func get_inertia_contribution() -> Vector3:
	return Vector3.ZERO

## Returns the center of mass offset relative to the vehicle origin.
## By default, uses the component's local position.
func get_center_of_mass_offset() -> Vector3:
	return position

## Returns power consumption in Watts.
func get_power_consumption() -> float:
	return power_consumption

## Returns power production in Watts.
func get_power_production() -> float:
	return power_production
