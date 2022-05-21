extends KinematicBody

# How fast the player moves in meters per second.
export var speed = 14.1
# The downward acceleration when in the air, in meters per second squared.
export var fall_acceleration = 1.6

var velocity = Vector3.ZERO
var direction = Vector3.ZERO

var speed_up = 100


func _physics_process(delta):
	
	var throttle = 0
	
	if Input.is_action_pressed("yaw_right"):
		direction.x += 1
	if Input.is_action_pressed("yaw_left"):
		direction.x -= 1
	if Input.is_action_pressed("pitch_up"):
		direction.z += 1
	if Input.is_action_pressed("pitch_down"):
		direction.z -= 1
	
	if Input.is_action_pressed("throttle"):
		throttle = -1
		$Pivot/Exhause.visible = true
	else:
		$Pivot/Exhause.visible = false
	
	if direction != Vector3.ZERO:
		direction = direction.normalized()
		$Pivot.look_at(translation + direction, Vector3.UP)

	velocity.x += (throttle*direction.x * speed)*delta
	velocity.z += (throttle*direction.z * speed)*delta
	velocity.y += (throttle*direction.y * speed - fall_acceleration) * delta
	
	velocity = move_and_slide(velocity, Vector3.UP)
