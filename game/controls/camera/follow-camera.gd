#TBD Delete this file as it's not used anywhere
extends Camera

#see this link about interpolation:
# https://docs.godotengine.org/en/3.1/tutorials/math/interpolation.html
# Nice tutorial https://kidscancode.org/godot_recipes/3d/camera_gimbal/

export var follow_speed = 4.0

export var camera_distance = 100.0
export var camera_phi = PI/2
export var camera_theta = 0

var camera_position = Vector3.ZERO

onready var player = $"../Spacecraft"
var tTarget = Transform()

func spherical_to_cartesian(r, phi, theta):
	#print(phi, " ", theta, " phi:", sin(phi), " ", cos(phi), " theta: ", sin(theta), " ", cos(theta))
	#print(Vector3(sin(phi)*cos(theta), sin(phi)*sin(theta), cos(phi)))
	return r*Vector3(sin(phi)*cos(theta), sin(phi)*sin(theta), cos(phi))

func update_camera_position():
	camera_position = spherical_to_cartesian(camera_distance, camera_phi, camera_theta)
	#print(camera_position)	
	
func _ready():
	#Input.set_mouse_mode(Input.MOUSE_MODE_CAPTURED)
	update_camera_position()
	

func _input(event):
	if Input.is_action_pressed("rotate_camera"): #dive up
		Input.set_mouse_mode(Input.MOUSE_MODE_CAPTURED)
		if event is InputEventMouseMotion:
			var movement = event.relative
			camera_phi = camera_phi + movement.x / 10
			camera_theta = camera_theta + movement.y/10
			update_camera_position()
	else:
		Input.set_mouse_mode(Input.MOUSE_MODE_VISIBLE)
			
func _physics_process(delta):
	tTarget.origin = player.global_transform.origin + camera_position
		
	tTarget = tTarget.looking_at(player.global_transform.origin, tTarget.origin)
	global_transform = tTarget
	#global_transform = global_transform.interpolate_with(tTarget, delta * follow_speed)
