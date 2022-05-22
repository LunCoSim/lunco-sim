extends Camera

#see this link about interpolation:
# https://docs.godotengine.org/en/3.1/tutorials/math/interpolation.html

export var camera_distance = 60.0
export var camera_distance_y = 30.0

const FOLLOW_SPEED = 4.0

onready var player = $"../Spacecraft"

var tTarget = Transform()
var tPrev

func _physics_process(delta):
	tTarget.origin = player.global_transform.origin + (player.global_transform.basis.z * player.Z_FRONT * -1 * camera_distance) + (player.global_transform.basis.y * player.Z_FRONT * camera_distance_y)
		
	tTarget = tTarget.looking_at(player.global_transform.origin, player.global_transform.basis.y)  #Vector3(0,1,0))
	
	global_transform = global_transform.interpolate_with(tTarget,delta * 4.0)
