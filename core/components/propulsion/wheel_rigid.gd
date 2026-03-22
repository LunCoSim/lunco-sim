class_name LCWheelRigid
extends RigidBody3D

@export var max_torque: float = 200.0
@export var brake_torque: float = 400.0
@export var radius: float = 0.3

var drive_input: float = 0.0
var brake_input: float = 0.0

func _physics_process(delta: float):
	# Apply drive torque
	if abs(drive_input) > 0.01:
		var torque_vec = global_transform.basis.x * drive_input * max_torque
		apply_torque(torque_vec)
		print("Wheel %s: Applying torque: %s" % [name, torque_vec])
		
	# Apply brake torque
	if brake_input > 0.01:
		var ang_vel = angular_velocity.dot(global_transform.basis.x)
		# Apply opposite torque to stop rotation
		if abs(ang_vel) > 0.1:
			apply_torque(global_transform.basis.x * -sign(ang_vel) * brake_input * brake_torque)
		else:
			# Stop completely if slow enough
			angular_velocity = Vector3.ZERO

func set_drive(value: float):
	drive_input = clamp(value, -1.0, 1.0)

func set_brake(value: float):
	brake_input = clamp(value, 0.0, 1.0)
