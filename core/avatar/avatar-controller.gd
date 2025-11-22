class_name LCAvatarController
extends LCController

# Movement state
@export var direction: = Vector3(0.0, 0.0, 0.0)
@export var camera_basis: = Basis.IDENTITY
@export var speed: = 10

var _parent: Node3D
var enabled: bool = true

func _ready():
	_parent = get_parent()

func _process(delta):
	if not enabled or not _parent:
		return
	
	# Apply movement based on current direction and speed
	_parent.position += camera_basis * direction * delta * speed

# Input methods called by AvatarInputAdapter
func set_direction(new_direction: Vector3):
	direction = new_direction

func set_speed(new_speed: float):
	speed = new_speed
