class_name LCDynamicEffector
extends LCComponent

## Base class for components that apply forces and torques to a vehicle.
##
## Implements the "Dynamic Effector" concept from Basilisk.
## These components actively compute force/torque contributions.

func _ready():
	super._ready()

## Computes the force and torque applied by this effector.
## Returns a Dictionary with:
## - "force": Vector3 (global frame)
## - "torque": Vector3 (global frame)
## - "position": Vector3 (application point in global frame)
func compute_force_torque(delta: float) -> Dictionary:
	return {}

## Helper to convert local force to global
func local_to_global_force(local_force: Vector3) -> Vector3:
	return global_transform.basis * local_force

## Helper to convert local torque to global
func local_to_global_torque(local_torque: Vector3) -> Vector3:
	return global_transform.basis * local_torque
