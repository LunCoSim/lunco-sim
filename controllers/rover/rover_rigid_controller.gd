class_name LCRoverRigidController
extends LCController

@export var fl_wheel: LCWheelRigid
@export var fr_wheel: LCWheelRigid
@export var bl_wheel: LCWheelRigid
@export var br_wheel: LCWheelRigid

@export var steering_angle: float = 30.0

@onready var rover: RigidBody3D = get_parent()
func _ready():
	# Force wake up the rover and wheels
	rover.sleeping = false
	rover.can_sleep = false
	if fl_wheel: fl_wheel.sleeping = false; fl_wheel.can_sleep = false
	if fr_wheel: fr_wheel.sleeping = false; fr_wheel.can_sleep = false
	if bl_wheel: bl_wheel.sleeping = false; bl_wheel.can_sleep = false
	if br_wheel: br_wheel.sleeping = false; br_wheel.can_sleep = false

# Internal state
var motor_input := 0.0
var steering_input := 0.0
var brake_input := 0.0


func _physics_process(delta: float):
	# Differential Steering Logic mirrored from LCRoverController
	var left_drive = -motor_input - steering_input
	var right_drive = -motor_input + steering_input

	# Apply to wheels
	if fl_wheel: 
		fl_wheel.set_drive(left_drive)
	if bl_wheel: 
		bl_wheel.set_drive(left_drive)

	if fr_wheel: 
		fr_wheel.set_drive(right_drive)
	if br_wheel: 
		br_wheel.set_drive(right_drive)

	# DEBUG: Print inputs and torque application
	if abs(motor_input) > 0.01 or abs(steering_input) > 0.01:
		print("RoverRigidController: Throttle=", motor_input, " Steer=", steering_input, " LeftDrive=", left_drive, " RightDrive=", right_drive)
		if fl_wheel:
			print("  WheelFL: MotorInput=", left_drive)

	# Apply brake
	if fl_wheel: fl_wheel.set_brake(brake_input)
	if bl_wheel: bl_wheel.set_brake(brake_input)
	if fr_wheel: fr_wheel.set_brake(brake_input)
	if br_wheel: br_wheel.set_brake(brake_input)


# Control API for Avatar/Command system
func set_motor(value: float):
	motor_input = clamp(value, -1.0, 1.0)

func set_steering(value: float):
	steering_input = clamp(value, -1.0, 1.0)

func set_brake(value: float):
	brake_input = clamp(value, 0.0, 1.0)

# Command metadata
func get_command_metadata() -> Dictionary:
	return {
		"SET_MOTOR": {
			"description": "Set main motor power (-1.0 to 1.0)",
			"arguments": [{"name": "value", "type": "float", "default": 0.0}]
		},
		"SET_STEERING": {
			"description": "Set steering angle (-1.0 to 1.0)",
			"arguments": [{"name": "value", "type": "float", "default": 0.0}]
		},
		"SET_BRAKE": {
			"description": "Set brake force (0.0 to 1.0)",
			"arguments": [{"name": "value", "type": "float", "default": 0.0}]
		}
	}

# Commands for LCCommandExecutor
func cmd_set_motor(value: float = 0.0):
	set_motor(value)

func cmd_set_steering(value: float = 0.0):
	set_steering(value)

func cmd_set_brake(value: float = 0.0):
	set_brake(value)
