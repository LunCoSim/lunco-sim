#This code is based on this game: 
extends RigidBody

const Z_FRONT = 1 #in this game the front side is towards negative Z
const THRUST = 50
const THRUST_TURN = 200
const THRUST_ROLL = 50


onready var exhause = $Exhause

var wasThrust = false #Visible exhause

# damping: see linear and angular damping parameters

func _physics_process(delta):
	if Input.is_action_pressed("throttle"):
		add_central_force(transform.basis.z * Z_FRONT * THRUST)
		if !wasThrust:
			wasThrust = true
	else:
		if wasThrust:
			wasThrust=false

	exhause.emitting = wasThrust	
	
	if Input.is_action_pressed("pitch_up"): #dive up
		add_torque(global_transform.basis.x * -THRUST_TURN * Z_FRONT)
	if Input.is_action_pressed("pitch_down"): #dive down
		add_torque(global_transform.basis.x * THRUST_TURN * Z_FRONT)
	if Input.is_action_pressed("yaw_left"):
		add_torque(global_transform.basis.y * THRUST_TURN * Z_FRONT)
	if Input.is_action_pressed("yaw_right"):
		add_torque(global_transform.basis.y * -THRUST_TURN * Z_FRONT)
	if Input.is_action_pressed("roll_ccw"):
		add_torque(global_transform.basis.z * -THRUST_ROLL * Z_FRONT)
	if Input.is_action_pressed("roll_cw"):
		add_torque(global_transform.basis.z * THRUST_ROLL * Z_FRONT)

