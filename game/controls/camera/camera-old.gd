extends Camera

export var lerp_speed = 3.0
export (Vector3) var offset = Vector3.ZERO

onready var target = $Spacecraft

func _physics_process(delta):
	if !target:
		return
	var target_pos = target.global_transform.translated(offset)
	global_transform = global_transform.interpolate_with(target_pos, lerp_speed * delta)
	look_at(target.global_transform.origin, Vector3.UP)
