class_name LCAvatarController
extends LCController

# Movement state
@export var direction: = Vector3(0.0, 0.0, 0.0)
@export var camera_basis: = Basis.IDENTITY
@export var speed: = 10

var _parent: Node3D

# Called when the node enters the scene tree for the first time.
func _ready():
	_parent = get_parent()


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	_parent.position += camera_basis*direction*delta*speed
